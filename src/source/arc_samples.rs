use std::sync::Arc;
use std::time::Duration;

use rodio::{ChannelCount, Sample, SampleRate, Source};

pub struct ArcSamplesSource {
    samples: Arc<Vec<Sample>>,
    channels: ChannelCount,
    sample_rate: SampleRate,
    offset: usize,
}

impl ArcSamplesSource {
    pub fn new(
        samples: Arc<Vec<Sample>>,
        channels: ChannelCount,
        sample_rate: SampleRate,
        offset: usize,
    ) -> Self {
        let offset = offset.min(samples.len());
        Self {
            samples,
            channels,
            sample_rate,
            offset,
        }
    }
}

impl Iterator for ArcSamplesSource {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        if self.offset >= self.samples.len() {
            return None;
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
