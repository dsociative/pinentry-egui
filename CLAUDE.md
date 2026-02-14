# CLAUDE.md

## Overview

Custom pinentry for GPG on pure Wayland (niri) — replaces broken `pinentry-gtk`/`pinentry-gnome3`/`pinentry-qt`. Native Wayland GUI via egui (eframe + glow).

## Architecture

Single-file Rust binary (`src/main.rs`) implementing:

- **Assuan protocol** (stdin/stdout, line-based) — the standard pinentry wire protocol used by gpg-agent
- **egui GUI** — password dialog (GETPIN) and confirmation dialog (CONFIRM/MESSAGE)

### Assuan Commands

SET commands accumulate state: `SETDESC`, `SETPROMPT`, `SETTITLE`, `SETOK`, `SETCANCEL`, `SETERROR`, `SETKEYINFO`, `OPTION`.

Action commands show GUI:
- `GETPIN` → password dialog, returns `D <percent-encoded password>\nOK` or `ERR 83886179`
- `CONFIRM`/`MESSAGE` → OK/Cancel dialog
- `GETINFO pid`/`version` → returns process info
- `BYE` → exit

### Key Design Decisions

- **glow (OpenGL)** backend, not wgpu — wgpu fails without Vulkan support
- **`mpsc::channel`** to pass dialog result from eframe App back to protocol handler
- **`secrecy::SecretString`** for password zeroing in memory
- **Percent-encoding** for Assuan protocol: decode incoming `%XX`, encode outgoing `%`, CR, LF
- **`request_focus()`** called exactly once on first frame — repeated calls break TextEdit input (egui issue #5187)
- UI function `pin_dialog_ui()` extracted from App for testability with `egui_kittest`

## Testing

Tests use `egui_kittest` with `Harness::new_ui_state`. Password field has `accesskit::Role::PasswordInput` (not `TextInput`).

```bash
cargo test -- --nocapture
```

## Build & Install

```bash
cargo build --release
# In ~/.gnupg/gpg-agent.conf:
# pinentry-program /home/dsociative/study/pinentry-egui/target/release/pinentry-egui
gpgconf --kill gpg-agent
```

## Conventions

- User does NOT want Co-Authored-By in commits
- Wayland-only (no X11/DISPLAY)
