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
    cargo check --features test_loom
    cargo check --features test_basic

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
build-release: (build "release")
[group("Packaging")]
build-debug: (build "")

[group("Debug & Profiling")]
heaptrack-release binary:
    RUSTFLAGS="-C force-frame-pointers=yes" cargo build --release --bin {{ binary }}
    heaptrack ./target/release/{{ binary }}

# Run all test groups
[group("Tests")]
test-all: test-basic test-loom test-dhat

# Run basic tests
[group("Tests")]
test-basic:
    cargo test --no-default-features --features test_basic

# Run loom tests (requires loom shim)
[group("Tests")]
test-loom:
    cargo test --no-default-features --features test_loom

# Run dhat tests (requires dhat global allocator)
[group("Tests")]
test-dhat:
    cargo test --no-default-features --features test_dhat

[group("Benches")]
view-bench-report:
    xdg-open ./target/criterion/report/index.html

[group("Benches")]
bench-throughput:
    # TODO: requires some meta-selection to find cpu cores that share L cache
    taskset -c 0,1 cargo bench --no-default-features --bench spsc_throughput 

[group("Benches")]
bench-latency:
    # TODO: requires some meta-selection to find cpu cores that share L cache
    taskset -c 0,1 cargo bench --no-default-features --bench spsc_latency 
