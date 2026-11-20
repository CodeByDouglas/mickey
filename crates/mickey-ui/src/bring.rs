//! `mickey-ui bring <who> <bytes> <what> <held> <folder>`: asks whether to bring files
//! `who` offered, over `[interface] bring_over_gb` (PRD 0015); `<what>` is one line per copied
//! file or folder. `<held>` is how many bytes `<folder>`, where they go,
//! already holds, when it is worth cleaning first; `0` when not. Bring (or
//! Enter) exits 0; Ignore, Esc or closing, 1.
//!
//! It is a notice about files arriving, so it appears where the owner
//! wants transfers shown (`Corner::for_transfers`), with an app id of its
//! own for the compositor's rules.

use crate::i18n;
use crate::place::{self, Corner};
use crate::theme::{self, icon};
use eframe::egui;
use std::cell::Cell;
use std::process::ExitCode;
use std::rc::Rc;

/// What the owner is asked about.
pub struct Asked {
    pub who: String,
    pub bytes: u64,
    pub what: String,
    /// What the folder already holds, when worth cleaning first.
    pub held: Option<u64>,
    pub folder: String,
}

struct Bring {
    asked: Asked,
    answer: Rc<Cell<bool>>,
    /// The corner the window still has to move itself to, where the
    /// compositor does not place it (Windows, X11).
    place: Option<Corner>,
}

impl eframe::App for Bring {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(corner) = self.place
            && place::move_to(ui.ctx(), corner)
        {
            self.place = None;
        }
        let close = |ui: &egui::Ui| ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(6.0);
                let question = i18n::tf("bring.question", &[("name", &self.asked.who)]);
                ui.label(
                    egui::RichText::new(format!("{}  {question}", icon::ARROW_DOWN))
                        .size(16.0)
                        .strong(),
                );
                ui.add_space(6.0);
                theme::card(ui, |ui| {
                    for line in self.asked.what.lines() {
                        ui.label(format!("{}  {line}", icon::FILE));
                    }
                });
                ui.add_space(6.0);
                let size = crate::units::size(self.asked.bytes);
                ui.label(i18n::tf("bring.size", &[("size", &size)]));
                ui.weak(i18n::tf("bring.keep", &[("name", &self.asked.who)]));
                if let Some(held) = self.asked.held {
                    ui.add_space(6.0);
                    let held = crate::units::size(held);
                    ui.label(i18n::tf("bring.worth_cleaning", &[("size", &held)]));
                    if ui
                        .button(format!("{}  {}", icon::FILE, i18n::t("progress.open")))
                        .clicked()
                    {
                        crate::progress::open_folder(&self.asked.folder);
                    }
                }
                ui.add_space(10.0);
                let keys = ui.input(|input| {
                    if input.key_pressed(egui::Key::Enter) {
                        Some(true)
                    } else if input.key_pressed(egui::Key::Escape) {
                        Some(false)
                    } else {
                        None
                    }
                });
                let clicked = theme::yes_no(ui, i18n::t("bring.bring"), i18n::t("bring.ignore"));
                if let Some(bring) = keys.or(clicked) {
                    self.answer.set(bring);
                    close(ui);
                }
            });
        });
    }
}

pub fn window(asked: Asked) -> ExitCode {
    const APP_ID: &str = "mickey-bring";
    let corner = Corner::for_transfers();
    let placed_by_compositor = place::ask_compositor(corner, APP_ID);
    // Room for each line of what is brought (five at most), and for the
    // cleaning hint.
    let hint = if asked.held.is_some() { 70.0 } else { 0.0 };
    let height = 200.0 + 28.0 * asked.what.lines().count() as f32 + hint;
    let options = eframe::NativeOptions {
        viewport: theme::viewport()
            .with_title(i18n::t("bring.window_title"))
            .with_app_id(APP_ID)
            .with_inner_size([420.0, height])
            .with_resizable(false)
            .with_always_on_top(),
        ..Default::default()
    };
    let answer = Rc::new(Cell::new(false));
    let window = Bring {
        asked,
        answer: Rc::clone(&answer),
        place: (!placed_by_compositor).then_some(corner),
    };
    let result = eframe::run_native(
        APP_ID,
        options,
        Box::new(|creation| {
            theme::apply(&creation.egui_ctx);
            Ok(Box::new(window))
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
