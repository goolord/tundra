use std::marker::PhantomData;
use std::time::Duration;

use rodio::{Sample, Source};

/// An empty source.
pub struct Callback<T, S> {
    pub phantom_data: PhantomData<S>,
    pub callback: Box<dyn Send + Fn(T)>,
    pub args: T,
}

impl<T, S> Callback<T, S> {
    #[inline]
    pub fn new(callback: Box<dyn Send + Fn(T)>, args: T) -> Callback<T, S> {
        Callback {
            phantom_data: PhantomData,
            callback,
            args,
        }
    }
}

impl<T, S> Iterator for Callback<T, S>
where
    T: Copy,
{
    type Item = S;

    #[inline]
    fn next(&mut self) -> Option<S> {
        (self.callback)(self.args);
        None
    }
}

impl<T, S> Source for Callback<T, S>
where
    S: Sample,
    T: Copy,
{
    #[inline]
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    #[inline]
    fn channels(&self) -> u16 {
        1
    }

    #[inline]
    fn sample_rate(&self) -> u32 {
        48000
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::new(0, 0))
    }
}
