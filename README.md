# pinentry-egui

[![CI](https://github.com/dsociative/pinentry-egui/actions/workflows/ci.yml/badge.svg)](https://github.com/dsociative/pinentry-egui/actions/workflows/ci.yml)
[![Security](https://github.com/dsociative/pinentry-egui/actions/workflows/security.yml/badge.svg)](https://github.com/dsociative/pinentry-egui/actions/workflows/security.yml)
[![CodeQL](https://github.com/dsociative/pinentry-egui/actions/workflows/codeql.yml/badge.svg)](https://github.com/dsociative/pinentry-egui/actions/workflows/codeql.yml)
[![Crates.io](https://img.shields.io/crates/v/pinentry-egui.svg)](https://crates.io/crates/pinentry-egui)
[![Downloads](https://img.shields.io/crates/d/pinentry-egui.svg)](https://crates.io/crates/pinentry-egui)
[![License](https://img.shields.io/crates/l/pinentry-egui.svg)](https://github.com/dsociative/pinentry-egui#license)

A modern, native Wayland pinentry implementation for GPG using [egui](https://github.com/emilk/egui).

## Why?

Existing pinentry implementations (`pinentry-gtk`, `pinentry-gnome3`, `pinentry-qt`) often have issues on pure Wayland compositors like [niri](https://github.com/YaLTeR/niri), sway, or Hyprland. This implementation provides a lightweight, native Wayland GUI that just works.

## Features

- **Pure Wayland** - No X11/DISPLAY dependencies
- **Assuan protocol** - Full compatibility with gpg-agent
- **Minimal dependencies** - Single Rust binary with egui + glow (OpenGL)
- **Secure** - Keystrokes go straight into a zeroizing buffer (`zeroize` + `secrecy`); the plaintext never enters egui's retained widget state or undo history

## Installation

### From crates.io (recommended)

```bash
cargo install pinentry-egui
```

The binary will be installed to `~/.cargo/bin/pinentry-egui`.

### From source

```bash
git clone https://github.com/dsociative/pinentry-egui.git
cd pinentry-egui
cargo build --release
# Binary will be at ./target/release/pinentry-egui
```

### Configure GPG

Add to `~/.gnupg/gpg-agent.conf`:

```conf
# If installed via cargo install:
pinentry-program ~/.cargo/bin/pinentry-egui

# Or if built from source:
# pinentry-program /path/to/pinentry-egui/target/release/pinentry-egui
```

Restart gpg-agent:

```bash
gpgconf --kill gpg-agent
```

### Requirements

- Wayland compositor (niri, sway, Hyprland, etc.)
- OpenGL support
- Rust 1.95+ toolchain (for building from source)

## Testing

Test the password dialog:

```bash
# If installed via cargo install:
echo -e "SETDESC Enter your password\nSETPROMPT Password:\nGETPIN\nBYE" | pinentry-egui

# Or from source:
echo -e "SETDESC Enter your password\nSETPROMPT Password:\nGETPIN\nBYE" | ./target/release/pinentry-egui
```

Run unit tests (from source):

```bash
cargo test
```

### UI automation with egui_mcp

An opt-in `inspection` cargo feature exposes eframe's [inspection
protocol](https://crates.io/crates/egui_inspection) so an AI agent (or any
[egui_mcp](https://crates.io/crates/egui_mcp) client) can read the widget
tree, type into the dialog, click buttons and take screenshots:

```bash
cargo build --features inspection
EGUI_INSPECTION=1 ./target/debug/pinentry-egui   # binds 127.0.0.1:5719
```

Notes:

- **Never enable `inspection` in the binary you point gpg-agent at for real
  use**: with `EGUI_INSPECTION` set, any local process can read the dialog
  (including the passphrase dots and window contents) and inject input.
- Inspection serves the **first** dialog of a process. eframe re-attaches
  the plugin for every dialog, but the first dialog's accept thread never
  releases the port, so later binds fail with `AddrInUse` (eframe only
  logs a warning) while the stale listener keeps answering for a dead
  context — an `egui_inspection` 0.36 limitation. Later dialogs still work
  for the user, and gpg-agent launches one pinentry process per prompt, so
  in practice every prompt is inspectable.

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
