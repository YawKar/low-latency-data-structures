[default]
[private]
default:
    @just --list --unsorted

[private]
pre-commit-hook: format-check lint

# init dev environment
[group("Bootstrap")]
init:
    cargo fetch
    cargo install cargo-run-bin
    cargo bin -i
    cargo install cargo-show-asm
    cargo check --features tests_loom
    cargo check --features tests_basic
[group("Bootstrap")]
enable-hugepages:
    sudo sysctl -w vm.nr_hugepages=16
[group("Bootstrap")]
disable-hugepages:
    sudo sysctl -w vm.nr_hugepages=0

# format everything
[group("Code Style")]
format check="":
    # nix
    find . -type f -name "*.nix" -exec nixfmt -sv {{ if check != "" { "-c" } else { "" } }} {} + 
    # yaml
    yamlfmt {{ if check != "" { "-lint" } else { "" } }} .
    # just
    just --fmt {{ if check != "" { "--check" } else { "" } }}
    # rust
    cargo bin rustfmt-unstable {{ if check == "" { "-a" } else { "" } }}
[group("Code Style")]
format-check: (format "check")

# lint everything
[group("Code Style")]
lint fix="":
    # nix
    statix {{ if fix != "" { "fix" } else { "check" } }}
    # rust
    cargo check
    cargo clippy {{ if fix != "" { "--fix --allow-dirty" } else { "" } }}
[group("Code Style")]
lint-fix: (lint "fix")

[group("Packaging")]
[private]
build level:
    cargo build {{ if level != "" { "--" + level } else { "" } }}
[group("Packaging")]
[private]
build-bin level binary:
    cargo build {{ if level != "" { "--" + level } else { "" } }} --bin {{ binary }}
[group("Packaging")]
build-release: (build "release")
[group("Packaging")]
build-debug: (build "")

[group("Debug & Profiling")]
heaptrack-release binary:
    RUSTFLAGS="-C force-frame-pointers=yes" cargo build --release --bin {{ binary }}
    heaptrack ./target/release/{{ binary }}
[group("Debug & Profiling")]
asm *ARGS:
    cargo asm --rust {{ ARGS }}
# L1 data cache: loads, stores, misses, miss rate
[group("Debug & Profiling")]
perf-l1 bin *args: (build-bin "release" bin)
    perf stat -e \
        L1-dcache-loads,\
        L1-dcache-load-misses,\
        L1-dcache-stores \
        -- ./target/release/{{ bin }} {{ args }}
# L2 cache
[group("Debug & Profiling")]
perf-l2 bin *args: (build-bin "release" bin)
    perf stat -e \
        l2_rqsts.references,\
        l2_rqsts.miss,\
        l2_rqsts.all_demand_data_rd \
        -- ./target/release/{{ bin }} {{ args }}
# L3 / LLC: loads, load misses, stores, store misses
[group("Debug & Profiling")]
perf-llc bin *args: (build-bin "release" bin)
    perf stat -e \
        LLC-loads,\
        LLC-load-misses,\
        LLC-stores,\
        LLC-store-misses \
        -- ./target/release/{{ bin }} {{ args }}
# dTLB: loads, load misses, stores, store misses, page walk cycles
[group("Debug & Profiling")]
perf-dtlb bin *args: (build-bin "release" bin)
    perf stat -e \
        dTLB-loads,\
        dTLB-load-misses,\
        dTLB-stores,\
        dTLB-store-misses,\
        dtlb_load_misses.walk_completed,\
        dtlb_load_misses.walk_active \
        -- ./target/release/{{ bin }} {{ args }}
# iTLB (instruction TLB — useful if code footprint is large)
[group("Debug & Profiling")]
perf-itlb bin *args: (build-bin "release" bin)
    perf stat -e \
        iTLB-loads,\
        iTLB-load-misses \
        -- ./target/release/{{ bin }} {{ args }}
# Full cache hierarchy in one shot (may need multiple runs if >8 counters)
[group("Debug & Profiling")]
perf-cache-all bin *args: (build-bin "release" bin)
    perf stat -e \
        L1-dcache-loads,\
        L1-dcache-load-misses,\
        L1-dcache-stores,\
        LLC-loads,\
        LLC-load-misses,\
        dTLB-loads,\
        dTLB-load-misses,\
        dTLB-stores,\
        dTLB-store-misses \
        -- ./target/release/{{ bin }} {{ args }}
# Overall execution quality: IPC, branch misses, context switches
[group("Debug & Profiling")]
perf-overview bin *args: (build-bin "release" bin)
    perf stat -e \
        cycles,\
        instructions,\
        branches,\
        branch-misses,\
        cache-references,\
        cache-misses,\
        context-switches,\
        cpu-migrations \
        -- ./target/release/{{ bin }} {{ args }}
# False sharing detection
[group("Debug & Profiling")]
perf-false-sharing-record bin *args: (build-bin "release" bin)
    sudo perf c2c record -g -- ./target/release/{{ bin }} {{ args }}
    sudo chmod o+r ./perf.data
[group("Debug & Profiling")]
perf-false-sharing-report:
    perf c2c report --stdio
[group("Debug & Profiling")]
perf-all bin *args: (build-bin "release" bin)
    perf stat -e \
        cycles,\
        instructions,\
        L1-dcache-loads,\
        L1-dcache-load-misses,\
        L1-dcache-stores,\
        LLC-loads,\
        LLC-load-misses,\
        LLC-stores,\
        LLC-store-misses,\
        dTLB-loads,\
        dTLB-load-misses,\
        dTLB-stores,\
        dTLB-store-misses,\
        branches,\
        branch-misses,\
        context-switches,\
        cpu-migrations \
        -- ./target/release/{{ bin }} {{ args }}
[group("Debug & Profiling")]
perf-tlb-compare bin *args: (build-bin "release" bin)
    @echo "=== WITHOUT hugepages ==="
    perf stat -e dTLB-loads,dTLB-load-misses,dTLB-stores,dTLB-store-misses,cycles,instructions \
        -- ./target/release/{{ bin }} {{ args }}
    @echo ""
    @echo "=== WITH hugepages (set HUGEPAGES=1 or adjust binary flag) ==="
    HUGEPAGES=1 perf stat -e dTLB-loads,dTLB-load-misses,dTLB-stores,dTLB-store-misses,cycles,instructions \
        -- ./target/release/{{ bin }} {{ args }}
# Run on specific cores for stable results
[group("Debug & Profiling")]
perf-pinned cores bin *args: (build-bin "release" bin)
    taskset -c {{ cores }} perf stat -e \
        cycles,\
        instructions,\
        L1-dcache-load-misses,\
        LLC-load-misses,\
        dTLB-load-misses,\
        branch-misses \
        -- ./target/release/{{ bin }} {{ args }}

# Run all test groups
[group("Tests")]
test-all: test-e2e-smoke test-basic test-loom test-dhat

# Run e2e smoke test
[group("Tests")]
test-e2e-smoke:
    @cargo run --release --bin smoke

# Run basic tests
[group("Tests")]
test-basic:
    @cargo test --no-default-features --features tests_basic
    @if [[ $(sysctl --values vm.nr_hugepages) != "0" ]]; then \
        cargo test --no-default-features --features tests_basic,tests_hugepage; \
    else \
        echo "{{ style("warning") }}[WARN]{{ NORMAL }} Your system doesn't have hugepages to run tests_hugepage"; \
    fi

# Run loom tests (requires loom shim)
[group("Tests")]
test-loom:
    @cargo test --no-default-features --features tests_loom

# Run dhat tests (requires dhat global allocator)
[group("Tests")]
test-dhat:
    @cargo test --no-default-features --features tests_dhat

[group("Benches")]
view-bench-report:
    xdg-open ./target/criterion/report/index.html

[group("Benches")]
bench-spsc:
    # TODO: requires some meta-selection to find cpu cores that share L cache
    taskset -c 0,1 cargo bench --no-default-features --bench spsc
