name: Build VST3 (Windows)

# Runs on GitHub's own Windows machines, not yours, so your machine never has to compile
# anything. Trigger it either by pushing to the repo, or manually from the Actions tab
# ("Run workflow" button) without needing to push at all.
on:
  push:
    paths:
      - "tape-machine-vst3/**"
      - ".github/workflows/build-windows.yml"
  workflow_dispatch: {}

jobs:
  build-windows:
    runs-on: windows-latest
    defaults:
      run:
        working-directory: tape-machine-vst3
    steps:
      - name: Check out the repo
        uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Cache cargo
        uses: Swatinem/rust-cache@v2
        with:
          workspaces: tape-machine-vst3

      - name: Install the nih-plug bundler
        run: cargo install --git https://github.com/robbert-vdh/nih-plug.git cargo-nih-plug

      - name: Run the parity tests (same checks run locally)
        run: cargo test --release

      - name: Build and bundle the plugin
        run: cargo nih-plug bundle tape-machine-vst3 --release

      - name: Upload the built VST3/CLAP
        uses: actions/upload-artifact@v4
        with:
          name: tape-machine-vst3-windows
          path: tape-machine-vst3/target/bundled/
