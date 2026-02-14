# pinentry-egui

A modern, native Wayland pinentry implementation for GPG using [egui](https://github.com/emilk/egui).

## Why?

Existing pinentry implementations (`pinentry-gtk`, `pinentry-gnome3`, `pinentry-qt`) often have issues on pure Wayland compositors like [niri](https://github.com/YaLTeR/niri), sway, or Hyprland. This implementation provides a lightweight, native Wayland GUI that just works.

## Features

- **Pure Wayland** - No X11/DISPLAY dependencies
- **Assuan protocol** - Full compatibility with gpg-agent
- **Minimal dependencies** - Single Rust binary with egui + glow (OpenGL)
- **Secure** - Uses `secrecy` crate for password zeroing in memory

## Installation

### Build from source

```bash
cargo build --release
```

### Configure GPG

Add to `~/.gnupg/gpg-agent.conf`:

```
pinentry-program /path/to/pinentry-egui
```

Restart gpg-agent:

```bash
gpgconf --kill gpg-agent
```

## Testing

Test the password dialog:

```bash
echo -e "SETDESC Enter your password\nSETPROMPT Password:\nGETPIN\nBYE" | ./target/release/pinentry-egui
```

Run unit tests:

```bash
cargo test
```

## Implementation Details

- **glow backend** (OpenGL) - wgpu requires Vulkan which may not be available
- **mpsc channel** - Passes dialog results from egui App to protocol handler
- **Percent-encoding** - Proper Assuan protocol encoding/decoding

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you shall be dual licensed as above, without any additional terms or conditions.
