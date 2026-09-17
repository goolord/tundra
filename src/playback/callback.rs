use std::time::Duration;

use rodio::{ChannelCount, Sample, SampleRate, Source};

/// A silent, empty source that runs `callback` when the sink reaches it.
///
/// rodio's `EmptyCallback` reports a fixed sample rate; this one reports the
/// output device's, so the sink's queue never switches rates for it.
pub struct Callback {
    callback: Box<dyn Send + Fn()>,
    sample_rate: SampleRate,
}

impl Callback {
    pub fn new(callback: impl Send + Fn() + 'static, sample_rate: SampleRate) -> Self {
        Self {
            callback: Box::new(callback),
            sample_rate,
        }
    }
}

impl Iterator for Callback {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        (self.callback)();
        None
    }
}

impl Source for Callback {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        1
    }

    fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::ZERO)
    }
}
