use std::marker::PhantomData;
use std::time::Duration;

use rodio::{ChannelCount, Sample, SampleRate, Source};

/// Signals completion by invoking a callback once, then yielding no samples.
pub struct Callback<T> {
    callback: Box<dyn Send + Fn(T)>,
    args: T,
    sample_rate: SampleRate,
    _sample: PhantomData<Sample>,
}

impl<T> Callback<T> {
    #[inline]
    pub fn new(callback: Box<dyn Send + Fn(T)>, args: T, sample_rate: SampleRate) -> Self {
        Self {
            callback,
            args,
            sample_rate,
            _sample: PhantomData,
        }
    }
}

impl<T> Iterator for Callback<T>
where
    T: Copy,
{
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Sample> {
        (self.callback)(self.args);
        None
    }
}

impl<T> Source for Callback<T>
where
    T: Copy,
{
    #[inline]
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    #[inline]
    fn channels(&self) -> ChannelCount {
        1
    }

    #[inline]
    fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::ZERO)
    }
}
