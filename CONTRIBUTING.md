# Contributing

Issues and pull requests are welcome, in English or French.

## Layout

- `app/src` : the Rust core and the Tauri shell: `engine.rs` drives capture, network and pairing; `wire.rs` is the encrypted protocol; `layout.rs` maps the physical layout.
- `app/ui` : the pairing and layout windows, plain HTML, CSS and JavaScript.
- `vendor` : patched copies of Lan Mouse's input crates.
- `assets/logo` : the logo, as SVG.

## Before a pull request

```bash
cd app
cargo test
cargo clippy
```

Test on real machines when you touch input or networking: unit tests do not catch stutter.

## Code style

Follow the surrounding code. Comments explain why, not what. User-facing text goes through `texts.rs` and the HTML pages, in both French and English.
