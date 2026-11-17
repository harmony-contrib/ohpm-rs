# ohpm-native

Native OpenHarmony build support for `ohpm-rs`.

This crate is a port of the `cargo-ohrs` CLI used by the `ohos-rs` project. It
resolves an OpenHarmony/HarmonyOS SDK (including the layout produced by
`openharmony-rs/setup-ohos-sdk`), configures the cross-compilation environment,
builds N-API modules, generates `index.d.ts`, and packages `.har` artifacts.
