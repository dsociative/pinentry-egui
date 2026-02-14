use std::io::{self, BufRead, Write};
use std::process;
use std::sync::mpsc;

use eframe::egui;
use secrecy::{ExposeSecret, SecretString};

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
    password: String,
    submitted: Option<bool>, // Some(true) = OK, Some(false) = Cancel
    focus_set: bool,
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
            let response = ui.add_sized(
                [ui.available_width(), 28.0],
                egui::TextEdit::singleline(&mut dialog.password)
                    .password(true)
                    .hint_text("Enter passphrase")
                    .font(egui::TextStyle::Body),
            );
            if !dialog.focus_set {
                dialog.focus_set = true;
                response.request_focus();
            }
            if response.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
            {
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
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            let _ = self.tx.send(DialogResult::Cancelled);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            pin_dialog_ui(ui, &self.pin_state, &mut self.dialog, self.want_pin);
        });

        if let Some(ok) = self.dialog.submitted.take() {
            if ok {
                if self.want_pin {
                    let _ = self.tx.send(DialogResult::Pin(SecretString::from(
                        self.dialog.password.clone(),
                    )));
                    self.dialog.password.clear();
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

    let _ = eframe::run_native(
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
    );

    rx.try_recv().unwrap_or(DialogResult::Cancelled)
}

fn respond(out: &mut impl Write, msg: &str) {
    let _ = writeln!(out, "{}", msg);
    let _ = out.flush();
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
    use egui_kittest::kittest::{Queryable, NodeT};
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

    #[test]
    fn test_password_field_gets_focus() {
        let mut harness = make_harness("Enter your GPG passphrase", true);
        harness.run();

        let ti = harness.get_by_role(accesskit::Role::PasswordInput);
        println!("focused: {}, value: {:?}", ti.is_focused(), ti.accesskit_node().value());
        assert!(ti.is_focused(), "password field should be auto-focused");
    }

    #[test]
    fn test_type_password() {
        let mut harness = make_harness("Enter passphrase", true);
        harness.run();

        let ti = harness.get_by_role(accesskit::Role::PasswordInput);
        assert!(ti.is_focused());

        ti.type_text("secret123");
        harness.run();

        println!("password: {:?}", harness.state().dialog.password);
        assert_eq!(harness.state().dialog.password, "secret123");
    }

    #[test]
    fn test_enter_submits() {
        let mut harness = make_harness("Enter passphrase", true);
        harness.run();

        let ti = harness.get_by_role(accesskit::Role::PasswordInput);
        ti.type_text("mypass");
        harness.run();

        harness.key_press(egui::Key::Enter);
        harness.run();

        println!("submitted: {:?}, password: {:?}", harness.state().dialog.submitted, harness.state().dialog.password);
        assert_eq!(harness.state().dialog.submitted, Some(true));
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
