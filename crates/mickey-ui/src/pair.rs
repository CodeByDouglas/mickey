//! `mickey-ui pair <who> <code>`: asks whether to pair with another PC
//! (PRD 0010). `mickey` starts it when a PC it does not know yet connects,
//! or when it connects to one; both PCs show the same code unless a
//! machine sits in between.
//!
//! `mickey-ui join <who> <by>`: asks whether to let in a PC that `by`, a
//! paired PC, introduced to the group (#184).
//!
//! `mickey-ui hotkeys <who> <changes>`: asks whether to use the hotkeys
//! `who` shared; `<changes>` is one line per hotkey that would change
//! (#349).
//!
//! `mickey-ui bring <who> <size> <what>`: asks whether to bring files
//! `who` offered, over 1 GB (PRD 0015); `<what>` is one line per copied
//! file or folder. Enter brings, Esc ignores.
//!
//! Exit code 0 means yes, 1 anything else.

use crate::i18n;
use eframe::egui;
use std::cell::Cell;
use std::process::ExitCode;
use std::rc::Rc;

/// What the window asks.
pub enum Ask {
    /// Pair with a PC that connected: both PCs show this code.
    Pair { code: String },
    /// Let in a PC that `by` introduced through the group.
    Join { by: String },
    /// Use the hotkeys a paired PC shared: what would change, one per line.
    Hotkeys { changes: String },
    /// Bring files a PC offered: how big, and what, one per line.
    Bring { size: String, what: String },
}

struct Pair {
    who: String,
    ask: Ask,
    answer: Rc<Cell<bool>>,
    /// The corner the window still has to move itself to, where the
    /// compositor does not place it (Windows, X11).
    place: Option<crate::place::Corner>,
}

impl eframe::App for Pair {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(corner) = self.place
            && crate::place::move_to(ui.ctx(), corner)
        {
            self.place = None;
        }
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                use crate::theme::{self, icon};
                ui.add_space(6.0);
                let (title_key, yes, no) = match &self.ask {
                    Ask::Pair { .. } => (
                        i18n::tf("pair.connect_with", &[("name", &self.who)]),
                        i18n::t("pair.pair"),
                        i18n::t("pair.refuse"),
                    ),
                    Ask::Join { .. } => (
                        i18n::tf("pair.let_in_with", &[("name", &self.who)]),
                        i18n::t("pair.let_in"),
                        i18n::t("pair.refuse"),
                    ),
                    Ask::Hotkeys { .. } => (
                        i18n::tf("pair.use_hotkeys", &[("name", &self.who)]),
                        i18n::t("pair.use_them"),
                        i18n::t("pair.keep_mine"),
                    ),
                    Ask::Bring { .. } => (
                        i18n::tf("pair.bring_from", &[("name", &self.who)]),
                        i18n::t("pair.bring"),
                        i18n::t("pair.ignore"),
                    ),
                };
                let heading = match self.ask {
                    Ask::Bring { .. } => icon::ARROW_DOWN,
                    _ => icon::LINK,
                };
                ui.label(
                    egui::RichText::new(format!("{heading}  {title_key}"))
                        .size(16.0)
                        .strong(),
                );
                ui.add_space(6.0);
                match &self.ask {
                    Ask::Pair { code } => {
                        theme::card(ui, |ui| {
                            ui.label(egui::RichText::new(code).size(36.0).monospace().strong());
                        });
                        ui.add_space(6.0);
                        ui.weak(i18n::tf("pair.pair_check", &[("icon", icon::SHIELD)]));
                    }
                    Ask::Join { by } => {
                        theme::card(ui, |ui| {
                            ui.label(i18n::tf(
                                "pair.join_says",
                                &[("by", by), ("name", &self.who)],
                            ));
                        });
                        ui.add_space(6.0);
                        ui.weak(i18n::tf("pair.join_can_use", &[("icon", icon::SHIELD)]));
                    }
                    Ask::Hotkeys { changes } => {
                        theme::card(ui, |ui| {
                            for change in changes.lines() {
                                ui.label(change);
                            }
                        });
                        ui.add_space(6.0);
                        ui.weak(i18n::t("pair.hotkeys_keep_own"));
                    }
                    Ask::Bring { size, what } => {
                        theme::card(ui, |ui| {
                            for line in what.lines() {
                                ui.label(format!("{}  {line}", icon::FILE));
                            }
                        });
                        ui.add_space(6.0);
                        ui.label(i18n::tf("pair.bring_size", &[("size", size)]));
                        ui.weak(i18n::tf("pair.bring_keep", &[("name", &self.who)]));
                    }
                }
                // Only here: Enter must never pair, nor let a PC in.
                if matches!(self.ask, Ask::Bring { .. }) {
                    let (enter, escape) = ui.input(|input| {
                        (
                            input.key_pressed(egui::Key::Enter),
                            input.key_pressed(egui::Key::Escape),
                        )
                    });
                    if enter {
                        self.answer.set(true);
                    }
                    if enter || escape {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    // Centre the two buttons.
                    ui.add_space((ui.available_width() - 190.0).max(0.0) / 2.0);
                    let close =
                        |ui: &egui::Ui| ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    if ui
                        .add_sized([90.0, 30.0], theme::primary(icon::CHECK, yes))
                        .clicked()
                    {
                        self.answer.set(true);
                        close(ui);
                    }
                    if ui
                        .add_sized(
                            [90.0, 28.0],
                            egui::Button::new(format!("{}  {no}", icon::X)),
                        )
                        .clicked()
                    {
                        close(ui);
                    }
                });
            });
        });
    }
}

pub fn window(who: &str, ask: Ask) -> ExitCode {
    // Bring is a notice about files arriving: in the bottom-right corner,
    // with an app id of its own for the compositor's rules. The others
    // ask about trust, in the middle of the screen.
    let (app_id, corner) = match &ask {
        Ask::Bring { .. } => ("mickey-bring", Some(bring_corner())),
        _ => ("mickey-pair", None),
    };
    let placed_by_compositor =
        corner.is_some_and(|corner| crate::place::ask_compositor(corner, app_id));
    let place = corner.filter(|_| !placed_by_compositor);
    let options = eframe::NativeOptions {
        viewport: crate::theme::viewport()
            .with_title(match ask {
                Ask::Bring { .. } => i18n::t("pair.bring_window_title"),
                _ => i18n::t("pair.window_title"),
            })
            .with_app_id(app_id)
            .with_inner_size(match &ask {
                // Room for each line of what is brought (five at most).
                Ask::Bring { what, .. } => [420.0, 200.0 + 28.0 * what.lines().count() as f32],
                _ => [400.0, 260.0],
            })
            .with_resizable(false)
            .with_always_on_top(),
        ..Default::default()
    };
    let answer = Rc::new(Cell::new(false));
    let window_answer = Rc::clone(&answer);
    let who = who.to_string();
    let result = eframe::run_native(
        "mickey-pair",
        options,
        Box::new(|creation| {
            crate::theme::apply(&creation.egui_ctx);
            Ok(Box::new(Pair {
                who,
                ask,
                answer: window_answer,
                place,
            }))
        }),
    );
    match (result, answer.get()) {
        (Ok(()), true) => ExitCode::SUCCESS,
        (Ok(()), false) => ExitCode::from(1),
        (Err(err), _) => {
            eprintln!("mickey-ui: cannot open a window: {err}");
            ExitCode::from(2)
        }
    }
}

/// Where the owner wants the Bring question (`[interface] bring_position`),
/// the bottom-right corner by default.
fn bring_corner() -> crate::place::Corner {
    let position = mickey_config::Config::load()
        .ok()
        .flatten()
        .map(|config| config.interface.bring_position)
        .unwrap_or_default();
    crate::place::Corner::parse(&position).unwrap_or(crate::place::Corner::BottomRight)
}
