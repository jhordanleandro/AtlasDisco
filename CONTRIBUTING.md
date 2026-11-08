# Contributing to AtlasDisco

Thanks for helping! Issues and pull requests in English or Portuguese are welcome.

## Before opening a pull request

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

On Windows, also run the end-to-end test: `powershell -File test/e2e.ps1` (after `cargo build --release`).

## Guidelines

- Keep the scanner read-only: nothing in a scan may open file contents (this protects cloud placeholders from being downloaded).
- Destructive operations (delete, move, hardlink) must ask for confirmation in the interface and report every failure.
- Platform-specific code lives in `crates/core/src/platform.rs` behind `cfg(windows)` / `cfg(unix)`.
- New report formats go in `crates/core/src/export.rs` and must be covered by the `todos_os_formatos` test.

## Releasing

Push a tag `vX.Y.Z`; the Release workflow builds Windows, Linux and macOS packages and publishes them.
