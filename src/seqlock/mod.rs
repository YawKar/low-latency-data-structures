//! SeqLock implementation.
//! What needs to be known: SeqLocks technically utilize UB, but it works.

mod lock;
pub mod reader;
pub mod writer;

pub use lock::new;
