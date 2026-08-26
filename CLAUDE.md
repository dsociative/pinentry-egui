# CLAUDE.md

## Overview

Custom pinentry for GPG on pure Wayland (niri, Hyprland, sway, …) — replaces broken `pinentry-gtk`/`pinentry-gnome3`/`pinentry-qt`. Native Wayland GUI via egui (eframe + glow).

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
- **eframe 0.36 line**: `App::ui(&mut Ui, &mut Frame)` (no `update`), content wrapped in `egui::Frame::central_panel`; the default `run_and_return: true` reuses one event loop, so several dialogs in one Assuan session work
- **No TextEdit for the passphrase**: keystrokes append to a `zeroize::Zeroizing<String>` and only a bullet mask is drawn, so the plaintext never enters egui's retained widget state or undo history; caret and focus frame are painted manually (see `pin_dialog_ui`)
- **`mpsc::channel`** to pass dialog result from eframe App back to protocol handler
- **`secrecy::SecretString`** for password zeroing in memory
- **Percent-encoding** for Assuan protocol: decode incoming `%XX`, encode outgoing `%`, CR, LF
- UI function `pin_dialog_ui()` extracted from App for testability with `egui_kittest`
- **`inspection` cargo feature (off by default)**: eframe's inspection port for UI automation via `egui_mcp`; active only with `EGUI_INSPECTION=1`. Never enable it in the production binary. egui_inspection 0.36 limitation: only the first dialog of a process is served

## Testing

Tests use `egui_kittest` with `Harness::new_ui_state`. The masked field is not
an accessible widget: tests drive it with synthetic `egui::Event`s and assert
on state; caret/focus indication is asserted via `harness.output().shapes`.

```bash
cargo +stable test -- --nocapture   # egui 0.36 needs rustc 1.95+
```

Live UI automation: build with `--features inspection`, run with
`EGUI_INSPECTION=1`, drive with the `egui_mcp` MCP server (registered as
`egui` in the project MCP config).

## Build & Install

```bash
cargo +stable build --release   # default toolchain here is an old nightly; egui 0.36 needs rustc 1.95+
# In ~/.gnupg/gpg-agent.conf:
# pinentry-program /home/dsociative/study/pinentry-egui/target/release/pinentry-egui
gpgconf --kill gpg-agent
```

## Conventions

- User does NOT want Co-Authored-By in commits
- Wayland-only (no X11/DISPLAY)
