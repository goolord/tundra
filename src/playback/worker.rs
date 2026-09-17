//! The audio thread. It owns the output stream and plays one track at a time;
//! the UI talks to it only through `PlayerCommand`s and `PlayerEvent`s.

use super::callback::Callback;
use super::position::PlaybackPosition;
use super::stream::append_stream;
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use rodio::buffer::SamplesBuffer;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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
    /// Sent by the audio callback when `segment` of track `track` runs out.
    Ended { track: u64, segment: u64 },
}

#[derive(Debug, Clone)]
pub enum PlayerEvent {
    Ended(u64),
    Looped(u64),
    WaveformPeaksReady(u64),
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
        std::thread::spawn(move || run(thread_handle, command_receiver, is_playing, looping, clamp_volume(volume)));
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

    /// A handle with no thread behind it; commands wait in the returned receiver.
    #[cfg(test)]
    pub fn detached() -> (Self, UnboundedReceiver<PlayerCommand>) {
        let (commands, command_receiver) = unbounded();
        let (events, _) = unbounded();
        (Self { commands, events }, command_receiver)
    }
}

#[derive(Clone, Copy)]
struct OutputFormat {
    channels: u16,
    sample_rate: u32,
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
    stream: rodio::OutputStream,
    output: OutputFormat,
    volume: f32,
    sink: Option<rodio::Sink>,
    track: Option<Track>,
    /// Counts started segments; a late end event from an earlier segment of
    /// the same track (before a seek or restart) is ignored.
    segment: u64,
    /// Where to resume, as a fraction of the track.
    offset: f64,
    is_playing: Arc<AtomicBool>,
    looping: Arc<AtomicBool>,
}

fn run(
    handle: PlayerWorker,
    commands: UnboundedReceiver<PlayerCommand>,
    is_playing: Arc<AtomicBool>,
    looping: Arc<AtomicBool>,
    volume: f32,
) {
    let stream = match rodio::OutputStreamBuilder::open_default_stream() {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("Audio output unavailable: {err}");
            is_playing.store(false, Ordering::SeqCst);
            handle.emit(PlayerEvent::DeviceUnavailable);
            return;
        }
    };
    let output = OutputFormat {
        channels: stream.config().channel_count(),
        sample_rate: stream.config().sample_rate(),
    };
    let mut worker = AudioWorker {
        handle,
        stream,
        output,
        volume,
        sink: None,
        track: None,
        segment: 0,
        offset: 0.0,
        is_playing,
        looping,
    };
    for command in futures::executor::block_on_stream(commands) {
        worker.run_command(command);
    }
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

        let sink = rodio::Sink::connect_new(self.stream.mixer());
        sink.set_volume(self.volume);
        if !play {
            sink.pause();
        }
        prime_output_queue(&sink, self.output);
        if let Err(err) = append_stream(
            &sink,
            &track.path,
            self.offset,
            track.position.total_frames(),
            Some(Arc::clone(&track.position)),
            self.output.channels,
            self.output.sample_rate,
        ) {
            self.set_playing(false);
            self.handle.emit(PlayerEvent::FileFailed(track.id, err));
            return;
        }

        self.segment += 1;
        let (id, segment, handle) = (track.id, self.segment, self.handle.clone());
        sink.append(Callback::new(
            move || handle.send(PlayerCommand::Ended { track: id, segment }),
            self.output.sample_rate,
        ));
        self.sink = Some(sink);
        self.set_playing(play);
    }

    fn run_command(&mut self, command: PlayerCommand) {
        match command {
            PlayerCommand::Load(path, position, id) => {
                position.reset();
                self.track = Some(Track { path, position, id });
                self.start(0.0, false);
            }
            PlayerCommand::Play => {
                let Some(track) = &self.track else {
                    self.set_playing(false);
                    return;
                };
                let total = track.position.total_frames();
                let finished =
                    self.sink.as_ref().is_none_or(rodio::Sink::empty) || playback_exhausted(self.offset, total);
                if finished {
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
                    track.position.reset();
                }
                self.set_playing(false);
            }
            PlayerCommand::Seek(progress, resume) => {
                if let Some(track) = &self.track {
                    track.position.seek_to(progress);
                }
                self.start(progress, resume);
            }
            PlayerCommand::Ended { track: id, segment } => {
                if segment != self.segment || self.track.as_ref().is_none_or(|track| track.id != id) {
                    return;
                }
                if self.looping.load(Ordering::Acquire) {
                    if let Some(track) = &self.track {
                        track.position.reset();
                    }
                    self.start(0.0, true);
                    self.handle.emit(PlayerEvent::Looped(id));
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

/// Tags the rodio queue at the device rate (its default filler is 44100 Hz).
fn prime_output_queue(sink: &rodio::Sink, output: OutputFormat) {
    if output.channels == 0 || output.sample_rate == 0 {
        return;
    }
    let silence = vec![0.0_f32; usize::from(output.channels)];
    sink.append(SamplesBuffer::new(output.channels, output.sample_rate, silence));
}

/// Whether resuming at `offset` would play nothing, so play should restart.
fn playback_exhausted(offset: f64, total_frames: u64) -> bool {
    if !offset.is_finite() || offset >= 1.0 {
        return true;
    }
    if total_frames == 0 {
        return false;
    }
    let skip_frames = (offset.clamp(0.0, 1.0) * total_frames as f64).round() as u64;
    skip_frames >= total_frames
}

#[cfg(test)]
mod tests {
    use super::playback_exhausted;

    #[test]
    fn play_from_start_when_offset_at_end() {
        assert!(playback_exhausted(1.0, 100));
        assert!(playback_exhausted(f64::NAN, 100));
        assert!(!playback_exhausted(0.0, 100));
        assert!(!playback_exhausted(0.5, 100));
    }

    #[test]
    fn play_from_start_when_skip_consumes_all_frames() {
        assert!(playback_exhausted(0.999, 100));
        assert!(!playback_exhausted(0.99, 100));
    }
}
