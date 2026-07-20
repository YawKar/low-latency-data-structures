//! Experimental lock-free SPSC, SPMC broadcast, and SeqLock primitives tuned
//! for ultra-low-latency systems (HFT-style request paths, real-time
//! telemetry, market-data fan-out, low-jitter audio).
//!
//! The crate is **experimental** (`0.0.x`) and the public API may break on
//! any release. Pin to an exact version if you depend on it.
//!
//! # Primitives
//!
//! - [`spsc`] - single-producer, single-consumer bounded FIFO queue.
//! - [`spmc`] - single-producer, multi-consumer broadcast queue. Every
//!   consumer observes every value the producer publishes, unless the
//!   consumer falls behind by more than `CAPACITY` slots (in which case it
//!   sees a [`spmc::ReadResult::Lapped`] and jumps to the latest slot).
//! - [`seqlock`] - single-writer, multi-reader cell. Readers spin-loop on a
//!   sequence number until they observe a consistent value.
//!
//! All primitives are bounded with a power-of-two `CAPACITY` (compile-time
//! enforced), allocate up front, and never block or allocate on the hot path.
//!
//! # Quick start
//!
//! ```
//! use low_latency_data_structures::spmc::{self, new, ReadResult};
//! use low_latency_data_structures::mem::global::GlobalAllocator;
//!
//! let (producer, [mut consumer]) = new::<u64, 1024, 1, GlobalAllocator>(
//!     spmc::Options::global_mlocked(),
//! );
//! producer.publish(42);
//! assert_eq!(consumer.try_read(), ReadResult::Value(42));
//! ```
//!
//! # Soundness
//!
//! All public APIs are safe to call. Internally the crate uses `unsafe` to
//! implement seqlock-style validation: a reader may observe a torn or stale
//! value, but the surrounding sequence-number check rejects it before it
//! escapes the public API. To make torn reads observable rather than
//! undefined, every value type carried by these primitives must implement
//! [`bytemuck::AnyBitPattern`].
//!
//! The SPSC queue is exercised under [loom](https://crates.io/crates/loom);
//! see the README for the per-primitive test coverage matrix.

#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(feature = "_bench_utils")]
#[doc(hidden)]
pub mod bench;
pub mod mem;
pub mod seqlock;
mod shim;
#[cfg(not(feature = "tests_loom"))]
pub mod spmc;
pub mod spsc;

#[cfg(test)]
#[cfg(feature = "tests_dhat")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;
