//! SeqLock implementation.
//! What needs to be known: SeqLocks technically utilize UB, but it works.
//! What exact UB is utilized:
//!     data race on `*mut T` (reads via volatile while another thread writes).
//!     SeqLock is a UB in Rust undefined core guidelines (C11/C++11 memory model): https://internals.rust-lang.org/t/include-racy-reads-in-rust-memory-model-with-maybeinvalid-t/24289
//! What mitigates it: torn read value don't propagate thanks to seq number check.

mod lock;
pub mod reader;
pub mod writer;

pub use lock::new;
