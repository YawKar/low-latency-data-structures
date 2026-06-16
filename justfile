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
    cargo check

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
