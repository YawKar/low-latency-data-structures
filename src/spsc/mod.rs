//! Single-producer, single-consumer bounded FIFO queue.
//!
//! Wait-free on both sides. Bounded with a compile-time power-of-two
//! `CAPACITY`. See [`new`] for the heap-backed variant and
//! [`new_hugepage_backed`] for the 2 MiB hugepage-backed variant (useful
//! when the slot buffer is large enough to thrash the dTLB).

mod consumer;
mod producer;
mod queue;

pub use consumer::Consumer;
pub use producer::Producer;
pub use queue::{new, new_hugepage_backed};
