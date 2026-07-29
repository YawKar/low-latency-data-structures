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
    cargo check --all-targets --no-default-features
    cargo check --all-targets --features tests_basic
    cargo check --tests --no-default-features --features tests_loom
[group("Bootstrap")]
enable-hugepages:
    sudo sysctl -w vm.nr_hugepages=64
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
    {{ if fix == "" { "cargo check --all-targets" } else { "echo cargo check skipped..." } }}
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

# Rehearse `cargo publish` without uploading (runs the sandboxed build crates.io will run server-side).
[group("Packaging")]
publish-dry-run:
    cargo publish --dry-run

# Gets the name of the package
[group("Packaging")]
[private]
_get-name:
    #!/usr/bin/env bash
    set -euo pipefail

    # --- extract crate name + version from Cargo.toml via cargo pkgid ---
    pkgid=$(cargo pkgid)
    remainder="${pkgid##*#}"
    case "$remainder" in
        *@*) name="${remainder%@*}"; version="${remainder##*@}" ;;
        *)   version="$remainder"; before="${pkgid%#*}"; name="${before##*/}" ;;
    esac
    echo "$name"

# Gets the version of the package
[group("Packaging")]
[private]
_get-version:
    #!/usr/bin/env bash
    set -euo pipefail

    # --- extract crate name + version from Cargo.toml via cargo pkgid ---
    pkgid=$(cargo pkgid)
    remainder="${pkgid##*#}"
    case "$remainder" in
        *@*) name="${remainder%@*}"; version="${remainder##*@}" ;;
        *)   version="$remainder"; before="${pkgid%#*}"; name="${before##*/}" ;;
    esac
    echo "$version"

# Checks that: working tree is clean, we're on main synced with origin,
# the tag is either absent or already points at HEAD (retry mode), and
# crates.io doesn't already have the given version.
[group("Packaging")]
[private]
_pre-publish-checks:
    #!/usr/bin/env bash
    set -euo pipefail

    name=$({{ just_executable() }} _get-name)
    version=$({{ just_executable() }} _get-version)
    tag="v$version"

    # --- pre-flight: clean working tree ---
    if [ -n "$(git status --porcelain)" ]; then
        echo "ERROR: working tree is dirty. Commit (or stash) everything first so" >&2
        echo "       the release tag points at a meaningful commit." >&2
        exit 1
    fi

    # --- pre-flight: must be on main ---
    branch=$(git rev-parse --abbrev-ref HEAD)
    if [ "$branch" != "main" ]; then
        echo "ERROR: not on main (current branch: $branch)." >&2
        echo "       Releases are cut from main only." >&2
        exit 1
    fi

    # --- pre-flight: main must be in sync with origin/main ---
    echo "fetching origin/main..."
    git fetch --quiet origin main
    if [ "$(git rev-parse HEAD)" != "$(git rev-parse origin/main)" ]; then
        echo "ERROR: local main is not in sync with origin/main." >&2
        echo "       Pull or rebase before releasing so the release commit is a" >&2
        echo "       clean fast-forward on top of origin/main." >&2
        exit 1
    fi

    # --- pre-flight: tag must be absent OR already at HEAD (retry-after-failure) ---
    if git rev-parse -q --verify "refs/tags/$tag" >/dev/null 2>&1; then
        tagged=$(git rev-parse "refs/tags/$tag^{commit}")
        head=$(git rev-parse HEAD)
        if [ "$tagged" != "$head" ]; then
            echo "ERROR: local git tag $tag already exists but does not point at HEAD." >&2
            echo "       Bump the version in Cargo.toml, or delete the stale tag with: git tag -d $tag" >&2
            exit 1
        fi
        echo "note: tag $tag already at HEAD (retry mode; tag/push step will be skipped)."
    fi

    # --- pre-flight: version must not already be on crates.io ---
    # TODO: parameterize over github.com, try to use metadata from the Cargo.toml
    echo "checking crates.io for $name v$version..."
    status=$(curl -sS -A "just-publish ($name; https://github.com/YawKar/low-latency-data-structures)" \
        -o /dev/null -w '%{http_code}' \
        "https://crates.io/api/v1/crates/$name/$version" || echo 000)
    if [ "$status" = "200" ]; then
        echo "ERROR: $name v$version is already published on crates.io." >&2
        echo "       Bump the version in Cargo.toml (reconsider semver first) and retry." >&2
        exit 1
    elif [ "$status" != "404" ]; then
        echo "WARN: crates.io returned HTTP $status for the version probe; proceeding anyway." >&2
    fi

# Regenerate CHANGELOG.md, create the release commit + tag, and push branch
# and tag to origin atomically. No-op if the tag already points at HEAD (a
# `just publish` retry after a `cargo publish` failure).
[group("Packaging")]
[private]
_prepare-release: _pre-publish-checks
    #!/usr/bin/env bash
    set -euo pipefail

    name=$({{ just_executable() }} _get-name)
    version=$({{ just_executable() }} _get-version)
    tag="v$version"

    # Retry mode: tag already at HEAD from a prior partial run.
    if git rev-parse -q --verify "refs/tags/$tag" >/dev/null 2>&1 \
        && [ "$(git rev-parse "refs/tags/$tag^{commit}")" = "$(git rev-parse HEAD)" ]; then
        echo "skipping tag/push: $tag already prepared."
        exit 0
    fi

    # --- regenerate changelog and commit it as the release commit ---
    echo "regenerating CHANGELOG.md for $tag..."
    git cliff --unreleased --tag "$tag" -o CHANGELOG.md
    git add CHANGELOG.md
    # --allow-empty: on a rerun where CHANGELOG.md happens to be identical,
    # we still want the release commit as an anchor for the tag.
    git commit --allow-empty -m "chore(release): prepare for $tag"

    # --- tag and push branch + tag atomically ---
    # --atomic ensures we don't end up with only one of the two refs pushed.
    echo "tagging $tag and pushing branch + tag to origin..."
    git tag -a "$tag" -m "$tag"
    git push --atomic origin HEAD "$tag"

    echo "done: $tag pushed with release commit."

# Cut a new release: pre-checks, regenerate CHANGELOG.md, commit, tag, push branch + tag to origin, then `cargo publish`. Idempotent on retry after a `cargo publish` failure.
[group("Packaging")]
publish: _prepare-release
    #!/usr/bin/env bash
    set -euo pipefail

    name=$({{ just_executable() }} _get-name)
    version=$({{ just_executable() }} _get-version)

    echo "publishing $name v$version to crates.io..."
    cargo publish

    echo "done: crate $name v$version published"

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
# iTLB (instruction TLB: useful if code footprint is large)
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
test-basic *ARGS:
    @cargo test --no-default-features --features tests_basic {{ ARGS }}
    @if [[ $(sysctl --values vm.nr_hugepages) != "0" ]]; then \
        cargo test --no-default-features --features tests_basic,tests_hugepage {{ ARGS }}; \
    else \
        echo "{{ style("warning") }}[WARN]{{ NORMAL }} Your system doesn't have hugepages to run tests_hugepage"; \
    fi

# Run loom tests (requires loom shim). Doc-tests are excluded because they
# touch the shim outside of a `loom::model` block, which loom forbids.
[group("Tests")]
test-loom:
    @cargo test --no-default-features --features tests_loom --tests

# Run dhat tests (requires dhat global allocator)
[group("Tests")]
test-dhat:
    @# dhat tests must run sequentially: HeapStats is process-global.
    @# --release: dhat measures what production sees. Debug builds add
    @# slow-path stubs (overflow panics, format helpers) that may lazily
    @# allocate inside the hot loop and confuse the assertion.
    @cargo test --release --no-default-features --features tests_dhat -- --test-threads=1

# Run doc-tests
[group("Tests")]
test-doc:
    @cargo test --no-default-features --doc

# Build rustdoc and fail on any warning (broken intra-doc link, missing docs).
[group("Tests")]
doc-check:
    @RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --no-default-features

# Build every example, including the bench utilities, to make sure they all
# compile. Bench examples need the internal `_bench_utils` feature.
[group("Tests")]
examples-build:
    @cargo build --examples
    @cargo build --features _bench_utils --examples

[group("Benches")]
view-bench-report:
    xdg-open ./target/criterion/report/index.html

# Setup these cores for benchmarking. just setup-cores 7,8
[group("Benches")]
setup-cores cores:
    cores="{{ cores }}"; \
    for i in ${cores//,/ }; do \
        set -x; \
        echo performance | sudo tee /sys/devices/system/cpu/cpu$i/cpufreq/scaling_governor; \
        set +x; \
        sibling=$(cat /sys/devices/system/cpu/cpu$i/topology/thread_siblings_list \
            | tr ',' '\n' | grep -v "^$i$" || true); \
        if [ -n "$sibling" ] && [ "$sibling" != "0" ]; then \
            set -x; \
            echo 0 | sudo tee /sys/devices/system/cpu/cpu$sibling/online; \
            set +x; \
            echo "offlined sibling $sibling of $i"; \
        fi \
    done
    echo 1 | sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo

# Un-setup these cores for benchmarking. just unsetup-cores 7,8
[group("Benches")]
unsetup-cores cores:
    cores="{{ cores }}"; \
    for i in ${cores//,/ }; do \
        set -x; \
        echo powersave | sudo tee /sys/devices/system/cpu/cpu$i/cpufreq/scaling_governor; \
        set +x; \
        sibling=$(cat /sys/devices/system/cpu/cpu$i/topology/thread_siblings_list \
            | tr ',' '\n' | grep -v "^$i$" || true); \
        if [ -n "$sibling" ] && [ "$sibling" != "0" ]; then \
            set -x; \
            echo 1 | sudo tee /sys/devices/system/cpu/cpu$sibling/online; \
            set +x; \
            echo "onlined sibling $sibling of $i"; \
        fi \
    done
    echo 0 | sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo

# Benches only very tiny single-threaded deterministic flow
[group("Benches: SPSC")]
bench-spsc-micro:
    cargo bench --no-default-features --bench spsc

# Handoff benchmark. Measures latency from the push to the pop of the item.
[group("Benches: SPSC")]
bench-spsc-handoff cores:
    sudo bash -c "ulimit -l 32000 && cargo build --release --features _bench_utils --example spsc_bench_handoff && taskset -c {{ cores }} cargo run --release --features _bench_utils --example spsc_bench_handoff"

# Throttled-producer offered-load sweep with coordinated-omission correction.
# Pass env through sudo: `BENCH_DEBUG=1 just bench-spsc-throttled 7,8` works
# because justfile recipes inherit env, then `sudo -E` forwards it.
[group("Benches: SPSC")]
bench-spsc-throttled cores:
    sudo -E bash -c "ulimit -l 32000 && cargo build --release --features _bench_utils --example spsc_bench_throttled && taskset -c {{ cores }} cargo run --release --features _bench_utils --example spsc_bench_throttled"

# Cold-cache single-thread drain sweep. Compares regular vs hugepage allocator
# across capacities to surface dTLB / cache effects. Needs hugepages enabled.
[group("Benches: SPSC")]
bench-spsc-drain core:
    sudo -E bash -c "ulimit -l unlimited && cargo build --release --features _bench_utils --example spsc_bench_drain && taskset -c {{ core }} cargo run --release --features _bench_utils --example spsc_bench_drain"

# Benches only very tiny single-threaded deterministic flow
[group("Benches: SeqLock")]
bench-seqlock-micro:
    cargo bench --no-default-features --bench seqlock

# Handoff benchmark. Measures latency from the write to the read of the item.
[group("Benches: SeqLock")]
bench-seqlock-handoff cores:
    sudo bash -c "ulimit -l 32000 && cargo build --release --features _bench_utils --example seqlock_bench_handoff && taskset -c {{ cores }} cargo run --release --features _bench_utils --example seqlock_bench_handoff"

# Benches only very tiny single-threaded deterministic flow
[group("Benches: SPMC")]
bench-spmc-micro:
    cargo bench --no-default-features --bench spmc

# Handoff benchmark. Measures latency from the push to the pop of the item.
[group("Benches: SPMC")]
bench-spmc-handoff cores:
    sudo bash -c "ulimit -l 32000 && cargo build --release --features _bench_utils --example spmc_bench_handoff && taskset -c {{ cores }} cargo run --release --features _bench_utils --example spmc_bench_handoff"

# Lapped recovery latency. Producer runs flat out, consumer adds a per-read
# delay (sweep via BENCH_DELAYS=...). Reports value latency, recovery cycles
# from Lapped to next Value, and skipped-count distribution.
[group("Benches: SPMC")]
bench-spmc-lapped cores:
    sudo -E bash -c "ulimit -l 32000 && cargo build --release --features _bench_utils --example spmc_bench_lapped && taskset -c {{ cores }} cargo run --release --features _bench_utils --example spmc_bench_lapped"

# Capacity sweep with a sustained producer. Single consumer reads as fast as
# it can. Reports value latency and lap count per capacity, useful for
# arguing about slot padding.
[group("Benches: SPMC")]
bench-spmc-capacity-sweep cores:
    sudo bash -c "ulimit -l 32000 && cargo build --release --features _bench_utils --example spmc_bench_capacity_sweep && taskset -c {{ cores }} cargo run --release --features _bench_utils --example spmc_bench_capacity_sweep"
