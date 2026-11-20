//! `mickey-ui progress <who>`: how a transfer of files from `who` is going
//! (PRD 0015), in the corner the Bring question uses. `mickey` writes one
//! line on this window's input whenever there is news, fields separated by
//! tabs:
//!
//! ```text
//! <stage>  <done bytes>  <total bytes>  <bytes a second>  <seconds left>
//! done     <files>       <bytes>        <folder>
//! failed   <why>
//! ```
//!
//! The stage is `space`, `rereading`, `transferring` or `paused`; speed and
//! time left may be empty while not known. Cancel prints `cancel` and
//! exits 1; closing after `done` exits 0.

use crate::i18n;
use crate::units;
use eframe::egui;
use std::io::BufRead;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// What the window shows: the last line `mickey` sent.
#[derive(Debug, Clone, PartialEq)]
enum State {
    Starting,
    Going {
        stage: String,
        done: u64,
        total: u64,
        speed: Option<f64>,
        left: Option<u64>,
    },
    Done {
        files: u64,
        bytes: u64,
        folder: String,
    },
    Failed(String),
}

/// One line from `mickey`; `None` for one it does not understand.
fn parse(line: &str) -> Option<State> {
    // Only the line's end: trailing tabs are empty fields.
    let fields: Vec<&str> = line.trim_end_matches(['\n', '\r']).split('\t').collect();
    Some(match fields.as_slice() {
        ["done", files, bytes, folder] => State::Done {
            files: files.parse().ok()?,
            bytes: bytes.parse().ok()?,
            folder: folder.to_string(),
        },
        ["failed", why] => State::Failed(why.to_string()),
        [stage, done, total, speed, left] => State::Going {
            stage: stage.to_string(),
            done: done.parse().ok()?,
            total: total.parse().ok()?,
            speed: speed.parse().ok(),
            left: left.parse().ok(),
        },
        _ => return None,
    })
}

struct Progress {
    who: String,
    state: Arc<Mutex<State>>,
    place: Option<crate::place::Corner>,
    /// Set by Cancel; read once the window has closed.
    cancelled: Arc<AtomicBool>,
}

impl eframe::App for Progress {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(corner) = self.place
            && crate::place::move_to(ui.ctx(), corner)
        {
            self.place = None;
        }
        let state = self
            .state
            .lock()
            .map(|state| state.clone())
            .unwrap_or(State::Starting);
        egui::CentralPanel::default().show(ui, |ui| {
            use crate::theme::{self, icon};
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!(
                    "{}  {}",
                    icon::ARROW_DOWN,
                    i18n::tf("progress.from", &[("name", &self.who)])
                ))
                .size(16.0)
                .strong(),
            );
            ui.add_space(8.0);
            let close = |ui: &egui::Ui| ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            match &state {
                State::Starting => {
                    ui.label(i18n::t("progress.starting"));
                }
                State::Going {
                    stage,
                    done,
                    total,
                    speed,
                    left,
                } => {
                    let fraction = if *total == 0 {
                        1.0
                    } else {
                        *done as f32 / *total as f32
                    };
                    ui.add(egui::ProgressBar::new(fraction).show_percentage());
                    ui.add_space(4.0);
                    let mut line = format!("{} / {}", units::size(*done), units::size(*total));
                    if let Some(speed) = speed {
                        line += &format!(" · {}/s", units::size(*speed as u64));
                    }
                    if let Some(left) = left {
                        line += &format!(
                            " · {}",
                            i18n::tf("progress.left", &[("time", &units::time(*left))])
                        );
                    }
                    ui.label(line);
                    ui.weak(stage_text(stage));
                    ui.add_space(8.0);
                    if ui
                        .button(format!("{}  {}", icon::X, i18n::t("progress.cancel")))
                        .clicked()
                    {
                        // `mickey` reads this and stops; the `.part` stays.
                        println!("cancel");
                        self.cancelled.store(true, Ordering::SeqCst);
                        close(ui);
                    }
                }
                State::Done {
                    files,
                    bytes,
                    folder,
                } => {
                    ui.label(i18n::tf(
                        "progress.done",
                        &[
                            ("files", &files.to_string()),
                            ("size", &units::size(*bytes)),
                        ],
                    ));
                    ui.weak(folder.as_str());
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(theme::primary(icon::FILE, i18n::t("progress.open")))
                            .clicked()
                        {
                            open_folder(folder);
                            close(ui);
                        }
                        if ui
                            .button(format!("{}  {}", icon::CHECK, i18n::t("progress.close")))
                            .clicked()
                        {
                            close(ui);
                        }
                    });
                }
                State::Failed(why) => {
                    ui.label(i18n::t("progress.failed"));
                    ui.weak(why.as_str());
                    ui.add_space(8.0);
                    if ui
                        .button(format!("{}  {}", icon::X, i18n::t("progress.close")))
                        .clicked()
                    {
                        close(ui);
                    }
                }
            }
        });
    }
}

/// The stage's words for the owner.
fn stage_text(stage: &str) -> &'static str {
    match stage {
        "space" => i18n::t("progress.stage_space"),
        "rereading" => i18n::t("progress.stage_rereading"),
        "paused" => i18n::t("progress.stage_paused"),
        _ => i18n::t("progress.stage_transferring"),
    }
}

/// Opens `folder` in the system's file manager.
fn open_folder(folder: &str) {
    let program = if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    if let Err(err) = std::process::Command::new(program).arg(folder).spawn() {
        eprintln!("mickey-ui: cannot open {folder}: {err}");
    }
}

pub fn window(who: &str) -> ExitCode {
    let corner = crate::place::Corner::for_transfers();
    let placed_by_compositor = crate::place::ask_compositor(corner, "mickey-progress");
    let options = eframe::NativeOptions {
        viewport: crate::theme::viewport()
            .with_title(i18n::t("progress.window_title"))
            .with_app_id("mickey-progress")
            .with_inner_size([400.0, 180.0])
            .with_resizable(false)
            .with_always_on_top(),
        ..Default::default()
    };
    let state = Arc::new(Mutex::new(State::Starting));
    let cancelled = Arc::new(AtomicBool::new(false));
    let (who, window_state, window_cancelled) = (who.to_string(), state.clone(), cancelled.clone());
    let result = eframe::run_native(
        "mickey-progress",
        options,
        Box::new(move |creation| {
            crate::theme::apply(&creation.egui_ctx);
            // Reads `mickey`'s lines on a thread, waking the window for each.
            let (ctx, reading) = (creation.egui_ctx.clone(), window_state.clone());
            std::thread::spawn(move || {
                for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                    if let (Some(new), Ok(mut state)) = (parse(&line), reading.lock()) {
                        *state = new;
                    }
                    ctx.request_repaint();
                }
            });
            Ok(Box::new(Progress {
                who,
                state: window_state,
                place: (!placed_by_compositor).then_some(corner),
                cancelled: window_cancelled,
            }))
        }),
    );
    match (result, cancelled.load(Ordering::SeqCst)) {
        (Ok(()), false) => ExitCode::SUCCESS,
        (Ok(()), true) => ExitCode::from(1),
        (Err(err), _) => {
            eprintln!("mickey-ui: cannot open a window: {err}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mickeys_lines_are_read() {
        assert_eq!(
            parse("transferring\t500\t1000\t25000000\t20\n"),
            Some(State::Going {
                stage: "transferring".into(),
                done: 500,
                total: 1000,
                speed: Some(25e6),
                left: Some(20),
            })
        );
        assert_eq!(
            parse("space\t0\t1000\t\t"),
            Some(State::Going {
                stage: "space".into(),
                done: 0,
                total: 1000,
                speed: None,
                left: None,
            })
        );
        assert_eq!(
            parse("done\t3\t1300000000\t/home/ana/Downloads/Mickey"),
            Some(State::Done {
                files: 3,
                bytes: 1_300_000_000,
                folder: "/home/ana/Downloads/Mickey".into(),
            })
        );
        assert_eq!(
            parse("failed\tno room"),
            Some(State::Failed("no room".into()))
        );
        assert_eq!(parse("transferring\tmany\t1\t\t"), None);
        assert_eq!(parse(""), None);
    }
}
