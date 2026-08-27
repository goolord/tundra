use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub struct PlaybackPosition {
    frame: AtomicU64,
    total_frames: AtomicU64,
}

impl PlaybackPosition {
    pub fn new(total_frames: u64) -> Arc<Self> {
        Arc::new(Self {
            frame: AtomicU64::new(0),
            total_frames: AtomicU64::new(total_frames),
        })
    }

    pub fn progress(&self) -> f64 {
        let total = self.total_frames.load(Ordering::Acquire);
        if total == 0 {
            return 0.0;
        }
        self.frame.load(Ordering::Acquire) as f64 / total as f64
    }

    pub fn total_frames(&self) -> u64 {
        self.total_frames.load(Ordering::Acquire)
    }

    pub fn set_total_frames(&self, total_frames: u64) {
        self.total_frames
            .store(total_frames, Ordering::Release);
        let frame = self.frame.load(Ordering::Acquire);
        if total_frames > 0 && frame > total_frames {
            self.frame.store(total_frames, Ordering::Release);
        }
    }

    pub fn set_frame(&self, frame: u64) {
        let total = self.total_frames.load(Ordering::Acquire);
        let capped = if total == 0 {
            frame
        } else {
            frame.min(total)
        };
        self.frame.store(capped, Ordering::Release);
    }

    pub fn reset(&self) {
        self.frame.store(0, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_total_frames_enables_progress() {
        let position = PlaybackPosition::new(0);
        position.set_frame(500);
        assert_eq!(position.progress(), 0.0);
        position.set_total_frames(1_000);
        assert!((position.progress() - 0.5).abs() < f64::EPSILON);
    }
}
