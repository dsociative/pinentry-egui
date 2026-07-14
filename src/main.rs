use std::cell::RefCell;
use std::io::{self, BufRead, Write};
use std::num::NonZeroU32;
use std::process;
use std::rc::Rc;

use egui_software_backend::{BufferMutRef, ColorFieldOrder, EguiSoftwareRender};
use secrecy::{ExposeSecret, SecretString};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::platform::run_on_demand::EventLoopExtRunOnDemand;
use winit::window::{Window, WindowId};
use zeroize::Zeroizing;

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn percent_encode_password(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'%' => out.push_str("%25"),
            b'\r' => out.push_str("%0D"),
            b'\n' => out.push_str("%0A"),
            _ => out.push(b as char),
        }
    }
    out
}

#[derive(Default)]
struct PinentryState {
    description: String,
    prompt: String,
    title: String,
    ok_label: String,
    cancel_label: String,
    error: String,
}

struct PinDialogState {
    /// Backing store for the passphrase. Keystrokes are appended here directly
    /// (never through an egui `TextEdit`), and it is zeroized on drop, so the
    /// secret is not copied into egui's retained widget/undo state and is wiped
    /// from memory when the dialog ends. Emptied into the result on submit.
    password: Zeroizing<String>,
    /// Some(true) = OK, Some(false) = Cancel.
    submitted: Option<bool>,
}

impl Default for PinDialogState {
    fn default() -> Self {
        PinDialogState {
            // Reserve up front so ordinary typing does not reallocate (which
            // would leave un-zeroized copies of the partial secret behind).
            password: Zeroizing::new(String::with_capacity(256)),
            submitted: None,
        }
    }
}

/// Lays out the dialog's widgets into `ui` and records the user's action.
///
/// Kept separate from the event loop so it can be driven headlessly in tests
/// (via egui_kittest): feed input events, run a frame, and read back the
/// resulting password / submitted flag without opening a window.
fn pin_dialog_ui(
    ui: &mut egui::Ui,
    pin_state: &PinentryState,
    dialog: &mut PinDialogState,
    want_pin: bool,
) {
    ui.vertical_centered(|ui| {
        ui.add_space(8.0);

        if !pin_state.error.is_empty() {
            ui.colored_label(egui::Color32::RED, &pin_state.error);
            ui.add_space(4.0);
        }

        if !pin_state.description.is_empty() {
            ui.label(&pin_state.description);
            ui.add_space(8.0);
        }

        if want_pin {
            let prompt = if pin_state.prompt.is_empty() {
                "Passphrase:"
            } else {
                &pin_state.prompt
            };

            // Feed keystrokes straight into our zeroizing buffer instead of an
            // egui `TextEdit`, so the plaintext never enters egui's retained
            // widget state or undo history. Only a masked view is drawn.
            ui.input(|input| {
                for event in &input.events {
                    match event {
                        egui::Event::Text(text) | egui::Event::Paste(text) => {
                            dialog.password.push_str(text);
                        }
                        egui::Event::Key {
                            key: egui::Key::Backspace,
                            pressed: true,
                            ..
                        } => {
                            dialog.password.pop();
                        }
                        _ => {}
                    }
                }
            });

            ui.label(prompt);
            ui.add_space(4.0);
            let dots = "\u{2022}".repeat(dialog.password.chars().count());
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::symmetric(6, 4))
                .show(ui, |ui| {
                    ui.set_min_width(240.0);
                    ui.add(
                        egui::Label::new(egui::RichText::new(dots).monospace()).selectable(false),
                    );
                });
            ui.add_space(12.0);
        }

        ui.horizontal(|ui| {
            let ok_text = if pin_state.ok_label.is_empty() {
                "OK"
            } else {
                &pin_state.ok_label
            };
            let cancel_text = if pin_state.cancel_label.is_empty() {
                "Cancel"
            } else {
                &pin_state.cancel_label
            };

            if ui.button(ok_text).clicked() {
                dialog.submitted = Some(true);
            }
            if ui.button(cancel_text).clicked() {
                dialog.submitted = Some(false);
            }
        });
    });

    // Enter submits, Escape cancels. Read from the global input rather than a
    // focused widget, since there is no `TextEdit` to own focus.
    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        dialog.submitted = Some(true);
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        dialog.submitted = Some(false);
    }
}

enum DialogResult {
    Pin(SecretString),
    Confirmed,
    Cancelled,
}

type SbSurface = softbuffer::Surface<Rc<Window>, Rc<Window>>;

thread_local! {
    /// winit permits only one event loop per process (and a *failed* creation
    /// still counts), so we keep one alive per thread and reuse it across
    /// dialogs. This lets one gpg-agent connection drive several
    /// `GETPIN`/`CONFIRM` prompts instead of failing with "EventLoop can't be
    /// recreated". Access only from the thread that first created it — the main
    /// thread, which is where winit requires the loop to run.
    static EVENT_LOOP: RefCell<Option<EventLoop<()>>> = const { RefCell::new(None) };
}

/// Builds one egui frame, mutating `dialog` in response to input.
///
/// `run` is used rather than the newer `run_ui` for API stability across the
/// egui 0.34 line; the deprecation is intentional.
#[allow(deprecated)]
fn build_frame(
    egui_ctx: &egui::Context,
    raw_input: egui::RawInput,
    pin_state: &PinentryState,
    dialog: &mut PinDialogState,
    want_pin: bool,
) -> egui::FullOutput {
    egui_ctx.run(raw_input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            pin_dialog_ui(ui, pin_state, dialog, want_pin);
        });
    })
}

struct App {
    pin_state: PinentryState,
    dialog: PinDialogState,
    want_pin: bool,

    egui_ctx: egui::Context,
    sw_render: EguiSoftwareRender,

    window: Option<Rc<Window>>,
    surface: Option<SbSurface>,
    egui_state: Option<egui_winit::State>,

    result: Option<DialogResult>,
    error: Option<String>,
}

impl App {
    fn new(pin_state: PinentryState, want_pin: bool) -> Self {
        App {
            pin_state,
            dialog: PinDialogState::default(),
            want_pin,
            egui_ctx: egui::Context::default(),
            // softbuffer wants 0x00RRGGBB (BGRA byte order on little-endian).
            sw_render: EguiSoftwareRender::new(ColorFieldOrder::Bgra),
            window: None,
            surface: None,
            egui_state: None,
            result: None,
            error: None,
        }
    }

    fn fail(&mut self, elwt: &ActiveEventLoop, err: String) {
        self.error = Some(err);
        elwt.exit();
    }

    fn redraw(&mut self, elwt: &ActiveEventLoop, window: &Rc<Window>) {
        let Some(state) = self.egui_state.as_mut() else {
            return;
        };
        let raw_input = state.take_egui_input(window);

        let full = build_frame(
            &self.egui_ctx,
            raw_input,
            &self.pin_state,
            &mut self.dialog,
            self.want_pin,
        );
        state.handle_platform_output(window, full.platform_output);

        if let Some(ok) = self.dialog.submitted.take() {
            self.result = Some(if ok {
                if self.want_pin {
                    // Move the inner String out (leaving the Zeroizing wrapper
                    // holding an empty one) into the secret, which zeroizes it in
                    // turn.
                    DialogResult::Pin(SecretString::new(
                        std::mem::take(&mut *self.dialog.password).into(),
                    ))
                } else {
                    DialogResult::Confirmed
                }
            } else {
                DialogResult::Cancelled
            });
            elwt.exit();
            return;
        }

        let clipped = self.egui_ctx.tessellate(full.shapes, full.pixels_per_point);
        if let Err(e) = self.present_frame(
            &clipped,
            &full.textures_delta,
            full.pixels_per_point,
            window,
        ) {
            self.fail(elwt, e);
        }
    }

    /// Rasterizes `clipped` into the softbuffer surface and presents it.
    fn present_frame(
        &mut self,
        clipped: &[egui::ClippedPrimitive],
        textures_delta: &egui::TexturesDelta,
        pixels_per_point: f32,
        window: &Rc<Window>,
    ) -> Result<(), String> {
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return Ok(());
        };
        let surface = self
            .surface
            .as_mut()
            .ok_or_else(|| "surface not initialized".to_string())?;
        surface.resize(w, h).map_err(|e| e.to_string())?;
        let mut buffer = surface.buffer_mut().map_err(|e| e.to_string())?;
        buffer.fill(0);
        {
            let pixels: &mut [[u8; 4]] = bytemuck::cast_slice_mut(&mut buffer);
            let mut bref = BufferMutRef::new(pixels, size.width as usize, size.height as usize);
            self.sw_render
                .render(&mut bref, clipped, textures_delta, pixels_per_point);
        }
        buffer.present().map_err(|e| e.to_string())?;
        Ok(())
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, elwt: &ActiveEventLoop) {
        // Modal: only redraw in response to input, not continuously.
        elwt.set_control_flow(ControlFlow::Wait);
        if self.window.is_some() {
            return;
        }
        let title = if self.pin_state.title.is_empty() {
            "pinentry-egui"
        } else {
            &self.pin_state.title
        };
        let attrs = Window::default_attributes()
            .with_title(title)
            .with_inner_size(winit::dpi::LogicalSize::new(400.0, 200.0))
            .with_resizable(false);
        let window = match elwt.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(e) => {
                self.fail(elwt, e.to_string());
                return;
            }
        };

        let context = match softbuffer::Context::new(window.clone()) {
            Ok(c) => c,
            Err(e) => {
                self.fail(elwt, e.to_string());
                return;
            }
        };
        let surface = match softbuffer::Surface::new(&context, window.clone()) {
            Ok(s) => s,
            Err(e) => {
                self.fail(elwt, e.to_string());
                return;
            }
        };

        let egui_state = egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &*window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );

        self.surface = Some(surface);
        self.egui_state = Some(egui_state);
        self.window = Some(window.clone());
        window.request_redraw();
    }

    fn window_event(&mut self, elwt: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else {
            return;
        };

        if let Some(state) = self.egui_state.as_mut() {
            let response = state.on_window_event(&window, &event);
            if response.repaint {
                window.request_redraw();
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                self.result = Some(DialogResult::Cancelled);
                elwt.exit();
            }
            WindowEvent::RedrawRequested => self.redraw(elwt, &window),
            _ => {}
        }
    }
}

/// Shows the dialog described by `state` and returns its outcome.
///
/// Reuses one process-wide event loop via `run_app_on_demand`, so it can be
/// called repeatedly across `GETPIN`/`CONFIRM` commands on one connection. Must
/// be called from the main thread (winit requirement) — the Assuan loop is.
fn show_dialog(state: PinentryState, want_pin: bool) -> DialogResult {
    match run_dialog(state, want_pin) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("pinentry-egui error: {}", e);
            DialogResult::Cancelled
        }
    }
}

fn run_dialog(state: PinentryState, want_pin: bool) -> Result<DialogResult, String> {
    EVENT_LOOP.with_borrow_mut(|slot| {
        if slot.is_none() {
            *slot = Some(EventLoop::new().map_err(|e| e.to_string())?);
        }
        let event_loop = slot.as_mut().expect("event loop initialized above");

        let mut app = App::new(state, want_pin);
        event_loop
            .run_app_on_demand(&mut app)
            .map_err(|e| e.to_string())?;

        if let Some(err) = app.error.take() {
            return Err(err);
        }
        Ok(app.result.take().unwrap_or(DialogResult::Cancelled))
    })
}

fn respond(out: &mut impl Write, msg: &str) {
    if let Err(e) = writeln!(out, "{}", msg) {
        eprintln!("Failed to write response: {}", e);
        process::exit(1);
    }
    if let Err(e) = out.flush() {
        eprintln!("Failed to flush output: {}", e);
        process::exit(1);
    }
}

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    respond(&mut stdout, "OK Pleased to meet you");

    let mut state = PinentryState::default();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let (cmd, arg) = match line.find(' ') {
            Some(pos) => (&line[..pos], line[pos + 1..].trim()),
            None => (line.as_str(), ""),
        };

        match cmd.to_uppercase().as_str() {
            "SETDESC" => {
                state.description = percent_decode(arg);
                respond(&mut stdout, "OK");
            }
            "SETPROMPT" => {
                state.prompt = percent_decode(arg);
                respond(&mut stdout, "OK");
            }
            "SETTITLE" => {
                state.title = percent_decode(arg);
                respond(&mut stdout, "OK");
            }
            "SETOK" => {
                state.ok_label = percent_decode(arg);
                respond(&mut stdout, "OK");
            }
            "SETCANCEL" | "SETNOTOK" => {
                state.cancel_label = percent_decode(arg);
                respond(&mut stdout, "OK");
            }
            "SETERROR" => {
                state.error = percent_decode(arg);
                respond(&mut stdout, "OK");
            }
            "SETKEYINFO" | "SETQUALITYBAR" | "SETQUALITYBAR_TT" => {
                respond(&mut stdout, "OK");
            }
            "OPTION" => {
                respond(&mut stdout, "OK");
            }
            "GETPIN" => {
                let current_state = std::mem::take(&mut state);
                match show_dialog(current_state, true) {
                    DialogResult::Pin(secret) => {
                        let encoded = percent_encode_password(secret.expose_secret());
                        respond(&mut stdout, &format!("D {}", encoded));
                        respond(&mut stdout, "OK");
                    }
                    _ => {
                        respond(&mut stdout, "ERR 83886179 Operation cancelled");
                    }
                }
            }
            "CONFIRM" | "MESSAGE" => {
                let current_state = std::mem::take(&mut state);
                match show_dialog(current_state, false) {
                    DialogResult::Cancelled => {
                        respond(&mut stdout, "ERR 83886179 Operation cancelled");
                    }
                    _ => {
                        respond(&mut stdout, "OK");
                    }
                }
            }
            "GETINFO" => {
                if arg == "pid" {
                    respond(&mut stdout, &format!("D {}", process::id()));
                    respond(&mut stdout, "OK");
                } else if arg == "version" {
                    respond(&mut stdout, "D 0.1.0");
                    respond(&mut stdout, "OK");
                } else {
                    respond(&mut stdout, "OK");
                }
            }
            "BYE" => {
                respond(&mut stdout, "OK closing connection");
                break;
            }
            _ => {
                respond(&mut stdout, "OK");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    struct TestState {
        pin_state: PinentryState,
        dialog: PinDialogState,
        want_pin: bool,
    }

    /// A kittest harness that drives [`pin_dialog_ui`] for `desc`, with an empty
    /// password and no submission to start.
    fn make_harness(desc: &str, want_pin: bool) -> Harness<'static, TestState> {
        let state = TestState {
            pin_state: PinentryState {
                description: desc.to_string(),
                prompt: "Passphrase:".to_string(),
                ..Default::default()
            },
            dialog: PinDialogState::default(),
            want_pin,
        };

        Harness::new_ui_state(
            |ui, state| {
                pin_dialog_ui(ui, &state.pin_state, &mut state.dialog, state.want_pin);
            },
            state,
        )
    }

    #[test]
    fn test_type_password() {
        let mut harness = make_harness("Enter passphrase", true);
        harness.run();

        harness.event(egui::Event::Text("secret123".into()));
        harness.run();

        assert_eq!(harness.state().dialog.password.as_str(), "secret123");
        // Typing alone does not submit.
        assert_eq!(harness.state().dialog.submitted, None);
    }

    #[test]
    fn test_backspace_removes_last_char() {
        let mut harness = make_harness("Enter passphrase", true);
        harness.run();

        harness.event(egui::Event::Text("abc".into()));
        harness.run();
        harness.key_press(egui::Key::Backspace);
        harness.run();

        assert_eq!(harness.state().dialog.password.as_str(), "ab");
    }

    #[test]
    fn test_enter_submits() {
        let mut harness = make_harness("Enter passphrase", true);
        harness.run();

        harness.event(egui::Event::Text("mypass".into()));
        harness.run();
        harness.key_press(egui::Key::Enter);
        harness.run();

        assert_eq!(harness.state().dialog.submitted, Some(true));
        // The typed value is preserved for the caller to take.
        assert_eq!(harness.state().dialog.password.as_str(), "mypass");
    }

    #[test]
    fn test_escape_cancels() {
        let mut harness = make_harness("Enter passphrase", true);
        harness.run();

        harness.key_press(egui::Key::Escape);
        harness.run();

        assert_eq!(harness.state().dialog.submitted, Some(false));
    }

    #[test]
    fn test_ok_button_submits() {
        let mut harness = make_harness("Enter passphrase", true);
        harness.run();

        harness.get_by_label("OK").click();
        harness.run();

        assert_eq!(harness.state().dialog.submitted, Some(true));
    }

    #[test]
    fn test_cancel_button() {
        let mut harness = make_harness("Enter passphrase", true);
        harness.run();

        harness.get_by_label("Cancel").click();
        harness.run();

        assert_eq!(harness.state().dialog.submitted, Some(false));
    }

    #[test]
    fn test_confirm_dialog() {
        let mut harness = make_harness("Do you trust this key?", false);
        harness.run();

        harness.get_by_label("OK").click();
        harness.run();

        assert_eq!(harness.state().dialog.submitted, Some(true));
    }
}
