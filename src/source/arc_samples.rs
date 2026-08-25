use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rodio::{ChannelCount, Sample, SampleRate, Source};

pub struct PlaybackPosition {
    frame: AtomicU64,
    total_frames: u64,
}

impl PlaybackPosition {
    pub fn new(total_frames: u64) -> Arc<Self> {
        Arc::new(Self {
            frame: AtomicU64::new(0),
            total_frames,
        })
    }

    pub fn progress(&self) -> f64 {
        if self.total_frames == 0 {
            return 0.0;
        }
        self.frame.load(Ordering::Acquire) as f64 / self.total_frames as f64
    }

    pub fn total_frames(&self) -> u64 {
        self.total_frames
    }

    pub fn set_frame(&self, frame: u64) {
        self.frame
            .store(frame.min(self.total_frames), Ordering::Release);
    }

    pub fn reset(&self) {
        self.frame.store(0, Ordering::Release);
    }
}

pub struct ArcSamplesSource {
    samples: Arc<Vec<Sample>>,
    channels: ChannelCount,
    sample_rate: SampleRate,
    offset: usize,
    position: Option<Arc<PlaybackPosition>>,
}

impl ArcSamplesSource {
    pub fn new(
        samples: Arc<Vec<Sample>>,
        channels: ChannelCount,
        sample_rate: SampleRate,
        offset: usize,
        position: Option<Arc<PlaybackPosition>>,
    ) -> Self {
        let offset = offset.min(samples.len());
        Self {
            samples,
            channels,
            sample_rate,
            offset,
            position,
        }
    }
}

impl Iterator for ArcSamplesSource {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        if self.offset >= self.samples.len() {
            return None;
        }

        let channels = self.channels as usize;
        if channels > 0
            && self.offset.is_multiple_of(channels)
            && let Some(position) = &self.position
        {
            position.set_frame((self.offset / channels) as u64);
        }

        let sample = self.samples[self.offset];
        self.offset += 1;
        Some(sample)
    }
}

impl Source for ArcSamplesSource {
    fn current_span_len(&self) -> Option<usize> {
        Some(self.samples.len().saturating_sub(self.offset))
    }

    fn channels(&self) -> ChannelCount {
        self.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}
