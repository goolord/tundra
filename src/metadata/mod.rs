mod auto_tag;
mod cache;
mod fields;
mod hints;
mod read;
mod riff;
mod search;
mod verify;
mod write;

pub use auto_tag::*;
pub use cache::*;
pub use fields::*;
pub use hints::*;
pub use read::*;
pub use search::*;
pub use write::*;

#[cfg(test)]
pub(crate) use riff::parse_riff_wave_chunks;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
