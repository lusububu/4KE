# 4000 E Console

A VST3 / CLAP channel strip and mix-bus compressor modelled on the circuits of a 1980s
SSL SL 4000 E console, by Them Changes. Unofficial, not affiliated with Solid State Logic.

- `console-4000e/` is the Rust plugin (nih-plug + egui).
  - `src/mna.rs` solves the EQ and filter netlists by modified nodal analysis and turns them
    into state-space filters.
  - `src/dsp.rs` holds the mic amp, dynamics, VCA fader, pan and bus compressor.
  - `src/editor.rs` and `src/knob.rs` draw the fixed-size interface.
  - `src/tests.rs` checks the model against the figures from the service and operator manuals.
- `.github/workflows/build-windows.yml` builds the Windows plugin on GitHub.
  See `console-4000e/BUILD_VIA_GITHUB.md`.
- `tools/` holds the background capture script and the measured layout used to place controls.

Local build (any OS with Rust):

    cd console-4000e
    cargo test --release
    cargo install --git https://github.com/robbert-vdh/nih-plug.git cargo-nih-plug
    cargo nih-plug bundle console-4000e --release
