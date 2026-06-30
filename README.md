# low-latency-data-structures

[![Crates.io](https://img.shields.io/crates/v/low-latency-data-structures.svg)](https://crates.io/crates/low-latency-data-structures)
[![Docs.rs](https://docs.rs/low-latency-data-structures/badge.svg)](https://docs.rs/low-latency-data-structures)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

Experimental lock-free SPSC, SPMC broadcast, and SeqLock primitives tuned for
ultra-low-latency systems (HFT-style request paths, real-time telemetry,
market-data fan-out, low-jitter audio).

> **Status: experimental (`0.0.x`).** The public API may change on any release.
> Pin to an exact version if you depend on it.

## What's inside

| Module | Pattern | Send/Sync | Allocation |
| --- | --- | --- | --- |
| [`spsc`] | bounded FIFO, 1 producer, 1 consumer | `Send`, `!Sync` | up-front, optional hugepages |
| [`spmc`] | bounded broadcast, 1 producer, `N` consumers | `Send`, `!Sync` | up-front |
| [`seqlock`] | single-writer, multi-reader cell | reader is `Send + Sync`, writer is `Send` | up-front |

All primitives are bounded with a compile-time `CAPACITY` that must be a power
of two. None of them allocate on the hot path. The SPMC and SeqLock primitives
read with seq-number validation, so the value type must implement
[`bytemuck::AnyBitPattern`].

[`spsc`]: https://docs.rs/low-latency-data-structures/latest/low_latency_data_structures/spsc/
[`spmc`]: https://docs.rs/low-latency-data-structures/latest/low_latency_data_structures/spmc/
[`seqlock`]: https://docs.rs/low-latency-data-structures/latest/low_latency_data_structures/seqlock/

## Quick start

```toml
[dependencies]
low-latency-data-structures = "=0.0.1"
```

```rust
// SPSC
use low_latency_data_structures::spsc;
let (producer, consumer) = spsc::new::<u64, 1024>();
assert!(producer.push(42).is_none());
assert_eq!(consumer.pop(), Some(42));

// SPMC broadcast
use low_latency_data_structures::spmc::{self, ReadResult};
let (producer, [mut c1, mut c2]) = spmc::new::<u64, 1024, 2>();
producer.publish(42);
assert_eq!(c1.try_read(), ReadResult::Value(42));
assert_eq!(c2.try_read(), ReadResult::Value(42));

// SeqLock
use low_latency_data_structures::seqlock;
let (writer, reader) = seqlock::new(0u64);
writer.write(42);
assert_eq!(reader.read(), 42);
```

Three runnable hello-world examples ship in `examples/`:

```sh
cargo run --release --example spsc_hello
cargo run --release --example spmc_hello
cargo run --release --example seqlock_hello
```

## Benchmarks

Numbers below are from this machine and this kernel cmdline. They are useful
as an order-of-magnitude reference, not as a vendor-style "X ns" claim. To
reproduce on your hardware, run the recipes from the `justfile` listed under
each table.

### Single-thread micro (Criterion, no cross-core traffic)

| Primitive | Cap | Time per op |
| --- | --- | --- |
| SPSC ping-pong | 64 | 3.79 ns |
| SPSC ping-pong | 1024 | 3.57 ns |
| SPSC ping-pong | 65536 | 3.61 ns |
| SeqLock write+read | n/a | 3.18 ns |
| SPMC publish+try_read | 64 | 4.57 ns |
| SPMC publish+try_read | 1024 | 4.64 ns |
| SPMC publish+try_read | 65536 | 4.59 ns |

Recipes: `just bench-spsc-micro`, `just bench-seqlock-micro`, `just bench-spmc-micro`.

### Cross-core handoff (cores 7 + 8, isolated)

Latency from the producing write to the matching consuming read, in ns.

| Primitive | p50 | p90 | p99 | p99.9 | max |
| --- | --- | --- | --- | --- | --- |
| SPSC | 168 | 222 | 284 | 319 | 4465 |
| SPMC | 120 | 148 | 184 | 188 | 2306 |
| SeqLock | 84 | 85 | 97 | 98 | 1907 |

Recipes: `just bench-spsc-handoff 7,8`, `just bench-spmc-handoff 7,8`,
`just bench-seqlock-handoff 7,8`.

### SPSC throttled-producer offered-load sweep

User-perceived (coordinated-omission-corrected) latency for a producer firing
at a fixed rate, consumer draining as fast as possible:

| Offered (ops/s) | p50 | p99 | p99.9 | Notes |
| --- | --- | --- | --- | --- |
| 1 M | 192 ns | 271 ns | 303 ns | comfortable |
| 10 M | 202 ns | 309 ns | 337 ns | comfortable |
| 28 M | 398 ns | 516 ns | 572 ns | knee |
| 30 M | 438 ns | 6.6 ms | 7.96 ms | falling behind |
| 50 M+ | saturated (effective ~31.5 M/s) | | | |

Recipe: `just bench-spsc-throttled 7,8`.

### SPSC cold-cache drain (regular vs hugepage)

Per-item drain cost for capacities that span L1 to deep L2/L3, after a 64 MiB
flush buffer evicts every cache level:

| Capacity | Bytes | Regular | Hugepage | Hugepage / regular |
| --- | --- | --- | --- | --- |
| 512 | 32 KiB | 8 ns | 7 ns | 0.88x |
| 8 192 | 512 KiB | 6 ns | 6 ns | 1.00x |
| 131 072 | 8 MiB | 6 ns | 6 ns | 1.00x |
| 1 048 576 | 64 MiB | 6 ns | 7 ns | 1.14x |

Hugepages need `vm.nr_hugepages > 0`. Recipe: `just bench-spsc-drain 7,8`
after `just enable-hugepages`.

### SPMC lapped recovery (CAPACITY=128, 2s run)

Producer flat out, consumer adds a fixed delay between reads. `Value` latency
is the time taken by a successful read, `Lapped` latency is the cost of
detecting and recovering from a lap:

| Per-read delay | Lapped % | Value p50 | Value p99 | Lapped p50 | Skipped p50 |
| --- | --- | --- | --- | --- | --- |
| 0 ns | 1.6 % | 15 ns | 73 ns | 109 ns | 132 |
| 38 ns | 7.1 % | 15 ns | 77 ns | 74 ns | 131 |
| 192 ns | 14 % | 15 ns | 78 ns | 70 ns | 138 |
| 771 ns | 34 % | 57 ns | 131 ns | 87 ns | 183 |
| 3.8 us | 100 % | 62 ns | 142 ns | 75 ns | 267 |

Recipe: `just bench-spmc-lapped 7,8`.

### SPMC capacity sweep (sustained producer, 2s per cell)

Single trailing consumer, value latency and lap rate as the slot ring grows:

| Capacity | Published | Values read | Lapped | p50 | p99 | p99.9 |
| --- | --- | --- | --- | --- | --- | --- |
| 16 | 128 M | 18 M | 5.4 M | 55 ns | 113 ns | 132 ns |
| 256 | 132 M | 45 M | 336 k | 15 ns | 60 ns | 79 ns |
| 4 096 | 133 M | 49 M | 21 k | 14 ns | 17 ns | 67 ns |
| 65 536 | 132 M | 49 M | 1.3 k | 14 ns | 15 ns | 52 ns |
| 1 048 576 | 133 M | 49 M | 80 | 14 ns | 16 ns | 92 ns |

Recipe: `just bench-spmc-capacity-sweep 7,8`.

### Setup

```
CPU:        Intel i7-10750H @ 2.6 GHz, 6 cores / 12 threads
L1d/L1i:    32 KiB / 32 KiB per core
L2:         256 KiB per core
L3:         12 MiB shared
Kernel:     Linux 6.18.36 (NixOS 26.05)
cmdline:    isolcpus=7,8 nohz_full=7,8 rcu_nocbs=7,8
            intel_idle.max_cstate=0 processor.max_cstate=0
Tuning:     scaling_governor=performance on 7,8
            no_turbo=1, SMT siblings of 7 and 8 offlined (cpu 1,2)
            vm.nr_hugepages=64 (for the drain bench)
```

The `just setup-cores 7,8` recipe applies the runtime knobs (governor,
turbo, sibling offline). The boot-time knobs (`isolcpus`, `nohz_full`,
`rcu_nocbs`, `intel_idle.max_cstate=0`) need to be in your kernel cmdline.

## Test coverage

| Primitive | basic | loom | dhat | hugepage |
| --- | --- | --- | --- | --- |
| `spsc` | yes | yes | yes | yes |
| `spmc` | yes | no | no | n/a |
| `seqlock` | yes | no | yes | n/a |

- `basic` exercises behaviour through the public API on real `std::thread`
  threads.
- `loom` runs the SPSC model under [loom](https://crates.io/crates/loom),
  including should-panic cases that confirm loom catches a second producer
  and a second consumer.
- `dhat` asserts zero hot-path heap allocations via [dhat](https://crates.io/crates/dhat).
- `hugepage` exercises the optional `mmap(MAP_HUGETLB)` allocator. Needs
  `vm.nr_hugepages > 0` on the host.

Run with `just test-basic`, `just test-loom`, `just test-dhat`,
`just test-e2e-smoke`, or `just test-all`.

## MSRV and features

- Rust 1.95 (2024 edition).
- Default features: none.
- Internal-only features (used by the test suite and by the CI examples; not
  intended for downstream use):
  - `tests_basic`, `tests_loom`, `tests_dhat`, `tests_hugepage`: enable the
    corresponding test groups.
  - `_bench_utils`: gates an internal `bench` helper module used by the
    bundled benchmark examples.

## Other production-ready crates in this space

If you need a battle-tested, stable-API alternative today, take a look at:

- [`crossbeam-queue`](https://crates.io/crates/crossbeam-queue) for general
  MPMC and SPSC bounded queues.
- [`ringbuf`](https://crates.io/crates/ringbuf) for a mature SPSC ring with
  blocking and split APIs.
- [`kanal`](https://crates.io/crates/kanal) for cross-thread async/sync
  channels.
- [`rtrb`](https://crates.io/crates/rtrb) for a real-time-focused SPSC ring.

This crate is not trying to replace those. It is a focused 0.0.x experiment
in squeezing the last hundred-or-so nanoseconds out of a few specific
patterns with deliberate platform assumptions (x86_64, isolated cores,
hugepages where relevant).

## License

MIT. See [`LICENSE`](LICENSE).
