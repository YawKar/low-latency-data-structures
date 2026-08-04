//! Per-crate adapters. Each adapter module is gated behind its own
//! `_bench_<crate>` feature so `_bench_utils` alone stays free of extra
//! third-party deps.

pub mod ours_spsc;

#[cfg(feature = "_bench_rtrb")]
pub mod rtrb_spsc;

#[cfg(feature = "_bench_ringbuf")]
pub mod ringbuf_spsc;

#[cfg(feature = "_bench_heapless")]
pub mod heapless_spsc;

#[cfg(feature = "_bench_nexus")]
pub mod nexus_spsc;

#[cfg(feature = "_bench_cbchan")]
pub mod cbchan_spsc;

#[cfg(feature = "_bench_flume")]
pub mod flume_spsc;

#[cfg(feature = "_bench_stdmpsc")]
pub mod stdmpsc_spsc;
