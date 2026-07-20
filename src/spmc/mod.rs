//! Single-producer, multi-consumer broadcast queue.
//!
//! Every consumer observes every value the producer publishes, unless the
//! consumer falls behind by more than `CAPACITY` slots, in which case the
//! next [`Consumer::try_read`] returns a [`ReadResult::Lapped`] and jumps the
//! read cursor to the most recently published slot.
//!
//! See [`new`] for the entry point, [`Producer`] for publishing, and
//! [`Consumer`] / [`ReadResult`] for reading.

mod builder;
mod consumer;
mod producer;
mod queue;

pub use builder::Options;
pub use consumer::{Consumer, ReadResult};
pub use producer::Producer;
pub use queue::{Slot, new};
