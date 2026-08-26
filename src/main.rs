use std::io::{self, BufRead, Write};
use std::process;
use std::sync::mpsc;

use eframe::egui;
use secrecy::{ExposeSecret, SecretString};
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

#[derive(Default)]
struct PinDialogState {
    // Keystrokes are appended here directly (never through an egui `TextEdit`,
    // whose retained widget state and undo history would keep the plaintext), and
    // the buffer is zeroized on drop. Moved into the `SecretString` on submit.
    password: Zeroizing<String>,
    submitted: Option<bool>, // Some(true) = OK, Some(false) = Cancel
}

fn pin_dialog_ui(
    ui: &mut egui::Ui,
    pin_state: &PinentryState,
    dialog: &mut PinDialogState,
    want_pin: bool,
) {
    ui.vertical_centered(|ui| {
        // Make text field stroke more visible
        let visuals = ui.visuals_mut();
        visuals.widgets.inactive.bg_stroke =
            egui::Stroke::new(1.0, egui::Color32::from_gray(140));
        visuals.widgets.hovered.bg_stroke =
            egui::Stroke::new(1.5, egui::Color32::from_gray(180));
        visuals.selection.stroke =
            egui::Stroke::new(2.0, egui::Color32::from_rgb(100, 150, 255));

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
            ui.label(prompt);
            ui.add_space(4.0);

            // Feed keystrokes straight into the zeroizing buffer instead of an
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
            // There is no TextEdit here (see above), so paint the two affordances
            // a text box normally provides: a focus-colored frame while keystrokes
            // are routed to the field, and a caret after the last bullet.
            let dots = "\u{2022}".repeat(dialog.password.chars().count());
            let focus_stroke = ui.visuals().selection.stroke;
            let caret_stroke = ui.visuals().text_cursor.stroke;
            egui::Frame::group(ui.style())
                .stroke(focus_stroke)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
                    ui.set_min_height(row_height);
                    let response =
                        ui.add(egui::Label::new(egui::RichText::new(dots).monospace()));
                    let rect = response.rect;
                    let caret_x = rect.right() + 2.0;
                    let caret_y = if rect.height() >= 1.0 {
                        rect.y_range()
                    } else {
                        egui::Rangef::new(rect.center().y - row_height / 2.0, rect.center().y + row_height / 2.0)
                    };
                    ui.painter().vline(caret_x, caret_y, caret_stroke);
                });

            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                dialog.submitted = Some(true);
            }
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
}

enum DialogResult {
    Pin(SecretString),
    Confirmed,
    Cancelled,
}

struct PinDialog {
    pin_state: PinentryState,
    dialog: PinDialogState,
    want_pin: bool,
    tx: mpsc::Sender<DialogResult>,
}

impl eframe::App for PinDialog {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            let _ = self.tx.send(DialogResult::Cancelled);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        egui::Frame::central_panel(ui.style()).show(ui, |ui| {
            pin_dialog_ui(ui, &self.pin_state, &mut self.dialog, self.want_pin);
        });

        if let Some(ok) = self.dialog.submitted.take() {
            if ok {
                if self.want_pin {
                    // Move the inner String out (leaving an empty one) into the
                    // secret, which zeroizes it in turn.
                    let value = std::mem::take(&mut *self.dialog.password);
                    let _ = self.tx.send(DialogResult::Pin(SecretString::from(value)));
                } else {
                    let _ = self.tx.send(DialogResult::Confirmed);
                }
            } else {
                let _ = self.tx.send(DialogResult::Cancelled);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn show_dialog(state: PinentryState, want_pin: bool) -> DialogResult {
    let title = if state.title.is_empty() {
        "pinentry-egui".to_string()
    } else {
        state.title.clone()
    };

    let (tx, rx) = mpsc::channel();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(&title)
            .with_inner_size([400.0, 200.0])
            .with_resizable(false),
        ..Default::default()
    };

    if let Err(e) = eframe::run_native(
        &title,
        options,
        Box::new(move |_cc| {
            Ok(Box::new(PinDialog {
                pin_state: state,
                dialog: PinDialogState::default(),
                want_pin,
                tx,
            }))
        }),
    ) {
        eprintln!("eframe error: {}", e);
    }

    rx.try_recv().unwrap_or(DialogResult::Cancelled)
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

    // The custom masked field is not an accessible widget, so drive it with
    // synthetic input events and assert on the resulting buffer/state.
    #[test]
    fn test_type_password() {
        let mut harness = make_harness("Enter passphrase", true);
        harness.run();

        harness.event(egui::Event::Text("secret123".into()));
        harness.run();

        assert_eq!(harness.state().dialog.password.as_str(), "secret123");
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
        assert_eq!(harness.state().dialog.password.as_str(), "mypass");
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

    // The masked field paints its own caret (a vertical line in the text-cursor
    // color) and a focus-colored frame, since there is no TextEdit to do it.
    // Assert on the emitted shapes so no GPU renderer is needed.
    fn find_caret(harness: &Harness<'_, TestState>) -> bool {
        let caret_color = harness
            .ctx
            .global_style()
            .visuals
            .text_cursor
            .stroke
            .color;
        harness.output().shapes.iter().any(|clipped| {
            if let egui::epaint::Shape::LineSegment { points, stroke } = &clipped.shape {
                stroke.color == caret_color && (points[0].x - points[1].x).abs() < 0.5
            } else {
                false
            }
        })
    }

    #[test]
    fn test_masked_field_has_caret() {
        let mut harness = make_harness("Enter passphrase", true);
        harness.run();
        assert!(find_caret(&harness), "empty masked field should show a caret");

        harness.event(egui::Event::Text("abc".into()));
        harness.run();
        assert!(
            find_caret(&harness),
            "masked field with input should show a caret"
        );
    }

    #[test]
    fn test_masked_field_has_focus_frame() {
        let mut harness = make_harness("Enter passphrase", true);
        harness.run();

        // pin_dialog_ui sets the selection stroke to this exact color; the field
        // frame must use it to signal that keystrokes go to the field.
        let focus_color = egui::Color32::from_rgb(100, 150, 255);
        let found = harness.output().shapes.iter().any(|clipped| {
            if let egui::epaint::Shape::Rect(rect_shape) = &clipped.shape {
                rect_shape.stroke.color == focus_color
            } else {
                false
            }
        });
        assert!(found, "masked field should draw a focus-colored frame");
    }

    #[test]
    fn test_confirm_dialog_has_no_caret() {
        let mut harness = make_harness("Do you trust this key?", false);
        harness.run();
        assert!(
            !find_caret(&harness),
            "confirm dialog has no input field, so no caret"
        );
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
