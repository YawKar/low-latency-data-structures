//! Single-writer, multi-reader cell with seqlock-based validation.
//!
//! The reader/writer protocol relies on torn reads being well-defined for
//! `T: bytemuck::AnyBitPattern`: any bit pattern is a valid `T`, so a
//! materialised torn value is at worst stale or garbled, never undefined
//! behaviour. The surrounding sequence-number check rejects such reads
//! before they leave [`Reader::read`].
//!
//! See [`new`] for the entry point, [`Writer`] for writing, and [`Reader`]
//! for reading.

mod lock;
mod reader;
mod writer;

pub use lock::new;
pub use reader::Reader;
pub use writer::Writer;
