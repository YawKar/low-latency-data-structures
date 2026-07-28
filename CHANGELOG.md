## [unreleased]

### ⚙️ Miscellaneous Tasks

- Remove cargo-show-asm from cargo bin
- Add git-cliff and conventional commits
## [0.0.4] - 2026-07-27

### 🚜 Refactor

- *(mem)* Add Allocator::Allocation<T> GAT
- *(all)* Rename according to RFC 430
- *(all)* Use crossbeam_utils::CachePadded
- *(spmc)* Make consumers cloneable

### ⚙️ Miscellaneous Tasks

- Bump crate version
## [0.0.3] - 2026-07-23

### 🐛 Bug Fixes

- *(crate)* Update version to 0.0.3
## [0.0.2] - 2026-07-23

### 🚀 Features

- *(spsc)* Implement Leslie Lamport's SPSC with Rigtorp's caches
- *(spsc)* Use C memory layout for Queue
- *(spsc)* Always inline hot path push/pop
- *(mem)* MAP_POPULATE and mlock backing buffer
- *(seqlock)* Add naive implementation
- *(spmc)* Basic impl + tests
- *(spmc+spsc)* Add flexible builders

### 🐛 Bug Fixes

- *(mem)* Alloc_buffer should use the UnsafeCell<MaybeUninit<T>> layout so that if for some reason it gets heavier it won't cause instant UB (future-proof kind of)
- *(spsc)* Hugepages allocator
- *(spsc)* Consumer/Producer: Send only if AllocT is Send too
- *(mem)* Check std::alloc::alloc for OOM
- *(shim)* Make UnsafeCell repr transparent
- *(spsc)* Use tail.wrapping_sub(head) in drop() instead of head..tail
- *(seqlock)* Use fences and reduce redundant fetches
- *(seqlock/bench)* Handoff measures seqlock
- *(seqlock)* Make T: bytemuck::AnyBitPattern
- *(spmc)* Set correct Sync, Send traits
- *(spmc)* Remove infinite loop in consumer
- *(tests/dhat)* Warmup

### 💼 Other

- *(spsc)* Add drain bench for hugepages effect
- *(seqlock)* Add questionable benches
- *(spmc)* Add handoff bench
- V0.0.2

### 🚜 Refactor

- *(mem)* Don't bother alloc_zeroed + panic if alloc failed
- *(spsc)* Remove redundant arc clone
- *(shim)* Remove unused
- *(spsc/bench)* Use quanta::Clock instead of rdtscp
- *(tests)* Make features plural and sync them with tests modules
- *(mem)* Make 'ptr()' static dispatch without additional step to differentiate between kinds of allocations
- *(spsc)* Remove '(): Allocation' and use a nice little test
- *(spsc)* Make capacity a const generic parameter
- *(spsc)* Better names for cold path checks
- Remove bad bench + simplify other redundancies
- *(benches)* Combine all benches into 1 binary
- *(benches)* Remove weak benches, strengthen the base handoff
- *(benches)* Make better isolation
- *(spsc/bench)* Rename handoff
- *(bench)* - add new bench_throttled for latency distributions under different load rates
- *(seqlock)* Align(128) on it to be sure about cache lines
- *(seqlock)* Use UnsafeCell instead of Cell
- *(seqlock/tests)* More readable assertions
- *(seqlock/bench/handoff)* Warmup up to 100k
- *(spmc)* Remove should-it-compile
- *(spmc+spsc)* Add Producer/Consumer types
- *(tests/smoke)* Add spmc
- *(benches)* Rename to remove collisions
- *(spmc)* - remove redundant fence and relax seq store before fence release
- *(spmc)* Remove NCONSUMERS const
- *(spmc)* Remove redundant repr(C)

### 📚 Documentation

- *(seqlock)* Add better comments here and there
- *(seqlock)* Update safety comments on impl Sync
- *(seqlock/bench)* Note p100 contamination by LOC
- *(seqlock)* Update safety comment on impl Sync
- *(crate)* Add docs + better the packaging itself
- *(readme)* Spmc test-covered by dhat+hugepage

### ⚡ Performance

- *(spsc)* Make slow&rare cache update #[cold] to hint LLVM make linear code for hot path
- *(benches)* Disable intel turbo for more independence on temperature

### 🧪 Testing

- *(spsc)* Static assert that Consumer/Producer is not Sync
- *(spsc)* Use get_mut() on slot_ptr in pop() so loom can track it as an exclusive access
- *(spsc)* Use Unsafe shim with loom
- *(spsc)* Add debug asserts that tail can't get further than capacity from head
- *(seqlock)* 0 allocations on the hot-path
- *(spmc)* 0 allocations

### ⚙️ Miscellaneous Tasks

- Init
- Set up heaptrack recipe to profile allocations
- Add 'just asm' recipe to easily get to the asm interleaved with rust source code
- Add perf recipes for caches
- Add recipes for publishing
- Add explicit error for targets other than linux
