//! The audio thread. It owns the output stream and plays one track at a time;
//! the UI talks to it only through `PlayerCommand`s and `PlayerEvent`s.

use super::position::PlaybackPosition;
use super::stream::StreamSource;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use rodio::buffer::SamplesBuffer;
use rodio::source::UniformSourceIterator;
use rodio::{OutputStream, Sink};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub fn clamp_volume(volume: f32) -> f32 {
    if volume.is_finite() { volume.clamp(0.0, 1.0) } else { 1.0 }
}

/// Track ids tie asynchronous events to the file that caused them, so a late
/// event from the previous file is ignored instead of acting on the new one.
pub enum PlayerCommand {
    /// Play `path` with this position and track id, paused at the start.
    Load(PathBuf, Arc<PlaybackPosition>, u64),
    Play,
    Pause,
    Stop,
    /// Seek to a fraction of the track, then play if the flag is set.
    Seek(f64, bool),
    SetVolume(f32),
    /// Sent by the audio callback when a segment (the second id) of a track runs out.
    Ended(u64, u64),
}

#[derive(Debug, Clone)]
pub enum PlayerEvent {
    Ended(u64),
    /// A track's peaks are built; the waveform only needs redrawing.
    WaveformPeaksReady,
    DeviceUnavailable,
    FileFailed(u64, String),
}

/// A handle to the audio thread.
#[derive(Debug, Clone)]
pub struct PlayerWorker {
    commands: UnboundedSender<PlayerCommand>,
    events: UnboundedSender<PlayerEvent>,
}

impl PlayerWorker {
    /// Starts the audio thread. `is_playing` is kept current by the thread;
    /// `looping` is read when a track ends.
    pub fn spawn(
        is_playing: Arc<AtomicBool>,
        looping: Arc<AtomicBool>,
        volume: f32,
    ) -> (Self, UnboundedReceiver<PlayerEvent>) {
        let (commands, command_receiver) = unbounded();
        let (events, event_receiver) = unbounded();
        let handle = Self { commands, events };
        let thread_handle = handle.clone();
        std::thread::spawn(move || {
            let stream = match rodio::OutputStreamBuilder::open_default_stream() {
                Ok(stream) => stream,
                Err(err) => {
                    eprintln!("Audio output unavailable: {err}");
                    is_playing.store(false, Ordering::SeqCst);
                    thread_handle.emit(PlayerEvent::DeviceUnavailable);
                    return;
                }
            };
            let mut worker = AudioWorker {
                handle: thread_handle,
                stream,
                volume: clamp_volume(volume),
                sink: None,
                track: None,
                segment: 0,
                offset: 0.0,
                is_playing,
                looping,
            };
            for command in futures::executor::block_on_stream(command_receiver) {
                worker.run_command(command);
            }
        });
        (handle, event_receiver)
    }

    pub fn send(&self, command: PlayerCommand) {
        if let Err(err) = self.commands.unbounded_send(command) {
            eprintln!("Player command failed: {err:?}");
        }
    }

    /// Reports an event as if the audio thread had, e.g. from a peak builder.
    pub fn emit(&self, event: PlayerEvent) {
        let _ = self.events.unbounded_send(event);
    }
}

/// The loaded track on the audio thread.
struct Track {
    path: PathBuf,
    position: Arc<PlaybackPosition>,
    id: u64,
}

/// Owns the output stream. Every start, seek, and loop builds a fresh `Sink`
/// and drops the previous one, which stops it without blocking: no queue to
/// drain, and an old segment's end-of-track callback never fires.
struct AudioWorker {
    handle: PlayerWorker,
    stream: OutputStream,
    volume: f32,
    sink: Option<Sink>,
    track: Option<Track>,
    /// Counts started segments; a late end event from an earlier segment of
    /// the same track (before a seek or restart) is ignored.
    segment: u64,
    /// Where to resume, as a fraction of the track.
    offset: f64,
    is_playing: Arc<AtomicBool>,
    looping: Arc<AtomicBool>,
}

impl AudioWorker {
    fn set_playing(&self, playing: bool) {
        self.is_playing.store(playing, Ordering::SeqCst);
    }

    /// Start a new segment of the current track at `offset`.
    fn start(&mut self, offset: f64, play: bool) {
        self.sink = None;
        let Some(track) = &self.track else {
            self.set_playing(false);
            return;
        };
        self.offset = offset.clamp(0.0, 1.0);
        let (channels, sample_rate) = (self.stream.config().channel_count(), self.stream.config().sample_rate());

        let sink = Sink::connect_new(self.stream.mixer());
        sink.set_volume(self.volume);
        if !play {
            sink.pause();
        }
        // Tags the rodio queue at the device rate (its default filler is 44100 Hz).
        if channels > 0 && sample_rate > 0 {
            sink.append(SamplesBuffer::new(channels, sample_rate, vec![0.0; usize::from(channels)]));
        }
        self.segment += 1;
        let (id, segment, handle) = (track.id, self.segment, self.handle.clone());
        let on_end = Box::new(move || handle.send(PlayerCommand::Ended(id, segment)));
        match StreamSource::open(&track.path, self.offset, Arc::clone(&track.position), on_end) {
            Ok(source) => sink.append(UniformSourceIterator::new(source, channels, sample_rate)),
            Err(err) => {
                self.set_playing(false);
                self.handle.emit(PlayerEvent::FileFailed(track.id, err));
                return;
            }
        }
        self.sink = Some(sink);
        self.set_playing(play);
    }

    fn run_command(&mut self, command: PlayerCommand) {
        match command {
            PlayerCommand::Load(path, position, id) => {
                self.track = Some(Track { path, position, id });
                self.start(0.0, false);
            }
            PlayerCommand::Play => {
                let Some(track) = &self.track else {
                    return self.set_playing(false);
                };
                let total = track.position.total_frames();
                if self.sink.as_ref().is_none_or(Sink::empty) || playback_exhausted(self.offset, total) {
                    let restart = playback_exhausted(track.position.progress(), total);
                    self.start(if restart { 0.0 } else { self.offset }, true);
                } else if let Some(sink) = &self.sink {
                    sink.play();
                    self.set_playing(true);
                }
            }
            PlayerCommand::Pause => {
                if let Some(track) = &self.track {
                    self.offset = track.position.progress();
                }
                if let Some(sink) = &self.sink {
                    sink.pause();
                }
                self.set_playing(false);
            }
            PlayerCommand::Stop => {
                self.sink = None;
                self.offset = 0.0;
                if let Some(track) = &self.track {
                    track.position.set_frame(0);
                }
                self.set_playing(false);
            }
            PlayerCommand::Seek(progress, resume) => self.start(progress, resume),
            PlayerCommand::Ended(id, segment) => {
                if segment != self.segment || self.track.as_ref().is_none_or(|track| track.id != id) {
                    return;
                }
                if self.looping.load(Ordering::Acquire) {
                    self.start(0.0, true);
                } else {
                    self.sink = None;
                    self.set_playing(false);
                    self.handle.emit(PlayerEvent::Ended(id));
                }
            }
            PlayerCommand::SetVolume(volume) => {
                self.volume = clamp_volume(volume);
                if let Some(sink) = &self.sink {
                    sink.set_volume(self.volume);
                }
            }
        }
    }
}

/// Whether resuming at `offset` would play nothing, so play should restart.
fn playback_exhausted(offset: f64, total_frames: u64) -> bool {
    if !offset.is_finite() || offset >= 1.0 {
        return true;
    }
    total_frames > 0 && (offset.clamp(0.0, 1.0) * total_frames as f64).round() as u64 >= total_frames
}

#[cfg(test)]
mod tests {
    use super::playback_exhausted;

    #[test]
    fn play_restarts_when_resuming_would_play_nothing() {
        for (offset, exhausted) in
            [(1.0, true), (f64::NAN, true), (0.999, true), (0.0, false), (0.5, false), (0.99, false)]
        {
            assert_eq!(playback_exhausted(offset, 100), exhausted, "offset {offset}");
        }
    }
}
