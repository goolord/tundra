//! Audio output, independent of the UI: the audio thread (`worker`), the
//! decoding source it plays (`stream`), and the shared playhead (`position`).

mod callback;
mod position;
mod stream;
mod worker;

pub use position::PlaybackPosition;
pub use stream::probe_decoder;
pub use worker::{clamp_volume, PlayerCommand, PlayerEvent, PlayerWorker};
