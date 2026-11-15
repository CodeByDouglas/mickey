//! The "What's new" window: the part of CHANGELOG.md for this version,
//! shown once by `mickey run` after an update (or from the tray). The
//! changelog is built into the program, so it always matches the version
//! running and needs no internet.

use crate::i18n;
use eframe::egui;
use std::process::ExitCode;

const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

/// The lines under `## {version}` in `changelog`, up to the next `## `.
/// Line by line, so Windows' `\r\n` (a Windows checkout) reads the same.
fn notes(changelog: &str, version: &str) -> Option<String> {
    let heading = format!("## {version}");
    let mut lines = changelog.lines().map(str::trim_end);
    lines.find(|line| *line == heading)?;
    let section: Vec<&str> = lines.take_while(|line| !line.starts_with("## ")).collect();
    Some(section.join("\n").trim().to_string())
}

/// Shows this version's notes, if the changelog has them.
pub fn window() -> ExitCode {
    let version = env!("CARGO_PKG_VERSION");
    let Some(notes) = notes(CHANGELOG, version) else {
        eprintln!("mickey-ui: no notes for {version} in the changelog");
        return ExitCode::from(1);
    };
    let options = eframe::NativeOptions {
        viewport: crate::theme::viewport()
            .with_title(i18n::tf("whats_new.window_title", &[("version", version)]))
            .with_app_id("mickey-whats-new")
            .with_inner_size([520.0, 420.0]),
        ..Default::default()
    };
    let result = eframe::run_native(
        "mickey-whats-new",
        options,
        Box::new(|creation| {
            crate::theme::apply(&creation.egui_ctx);
            Ok(Box::new(WhatsNew { version, notes }))
        }),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("mickey-ui: cannot open a window: {err}");
            ExitCode::from(2)
        }
    }
}

struct WhatsNew {
    version: &'static str,
    notes: String,
}

impl eframe::App for WhatsNew {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading(i18n::tf("whats_new.heading", &[("version", self.version)]));
            ui.add_space(8.0);
            egui::ScrollArea::vertical()
                .max_height(ui.available_height() - 40.0)
                .show(ui, |ui| {
                    // The changelog is in English; its lines are shown as
                    // written, a "- " item per paragraph.
                    for item in self.notes.split("\n- ") {
                        let text = item.trim_start_matches("- ").replace("\n  ", " ");
                        ui.label(format!("•  {}", text.trim()));
                        ui.add_space(6.0);
                    }
                });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(i18n::t("whats_new.close")).clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                ui.hyperlink_to(
                    i18n::t("whats_new.all_releases"),
                    "https://github.com/EduardoMG12/mickey/releases",
                );
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# What's new\n\nIntro.\n\n## Unreleased\n\n- Next.\n\n## 0.5.0\n\n- Taps click.\n- Edges on\n  COSMIC.\n\n## 0.4.5\n\n- Older.\n";

    #[test]
    fn a_versions_notes_end_at_the_next_heading() {
        let notes = |version| notes(SAMPLE, version);
        assert_eq!(
            notes("0.5.0").as_deref(),
            Some("- Taps click.\n- Edges on\n  COSMIC.")
        );
        assert_eq!(notes("0.4.5").as_deref(), Some("- Older."));
        assert_eq!(notes("0.4"), None, "not a prefix of another version");
        assert_eq!(notes("9.9.9"), None);
    }

    /// A Windows checkout turns every `\n` into `\r\n`.
    #[test]
    fn windows_line_endings_read_the_same() {
        let windows = SAMPLE.replace('\n', "\r\n");
        assert!(notes(&windows, "0.5.0").is_some());
        assert_eq!(notes(&windows, "0.5.0"), notes(SAMPLE, "0.5.0"));
    }

    #[test]
    fn the_real_changelog_has_an_unreleased_section() {
        assert!(notes(CHANGELOG, "Unreleased").is_some());
    }
}
