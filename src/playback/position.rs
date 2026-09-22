//! The playhead, shared lock-free between the audio thread and the UI.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct PlaybackPosition {
    frame: AtomicU64,
    total_frames: AtomicU64,
}

impl PlaybackPosition {
    pub fn new(total_frames: u64) -> Arc<Self> {
        Arc::new(Self { frame: AtomicU64::new(0), total_frames: AtomicU64::new(total_frames) })
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
        self.total_frames.store(total_frames, Ordering::Release);
        if total_frames > 0 {
            self.frame.fetch_min(total_frames, Ordering::AcqRel);
        }
    }

    pub fn set_frame(&self, frame: u64) {
        let total = self.total_frames();
        self.frame.store(if total == 0 { frame } else { frame.min(total) }, Ordering::Release);
    }

    /// Moves the playhead to `progress` (0 to 1) of the track.
    pub fn seek_to(&self, progress: f64) {
        self.set_frame((progress.clamp(0.0, 1.0) * self.total_frames() as f64).round() as u64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_needs_a_total_and_caps_at_the_end() {
        let position = PlaybackPosition::new(0);
        position.set_frame(500);
        assert_eq!(position.progress(), 0.0);
        position.set_total_frames(1_000);
        assert_eq!(position.progress(), 0.5);
        position.set_frame(5_000);
        assert_eq!(position.progress(), 1.0);
    }
}
