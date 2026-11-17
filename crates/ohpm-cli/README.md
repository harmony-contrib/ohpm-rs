# ohpm-cli

The `ohpm-rs` binary: an ohpm (OpenHarmony package manager) CLI reimplemented
in Rust. Headline capability: `publish` authenticates entirely from
environment variables — no TUI prompts — plus workspace mode
(`ohpm-workspace.yaml`), pre-release versioning, HAR packaging, and the native
`build`/`artifact`/`cargo`/`doctor` commands ported from `cargo-ohrs`.

Install a prebuilt release with [cargo-binstall](https://github.com/cargo-bins/cargo-binstall):

```sh
cargo binstall ohpm-cli
ohpm-rs --version
```

Prebuilt releases support Linux (glibc 2.35+), macOS, and Windows on x86_64 and
ARM64. Older releases and other targets may fall back to source compilation.
Use `cargo binstall ohpm-cli --strategies crate-meta-data` to require a binary
from this repository, or `cargo install ohpm-cli --locked` to build from source.

See the [workspace README](../../README.md) for the full documentation.
