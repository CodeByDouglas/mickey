//! The question before a big transfer (PRD 0015): over `ask_over()`, the
//! owner says Bring (to `Downloads/Mickey`) or Ignore, seeing what the
//! files are (`mickey-ui bring`).

use crate::transfer::cleanup;
use crate::transfer::offer::{Kind, Offer};
use std::path::Path;
use std::time::SystemTime;

/// Above this many bytes, a transfer is asked about first: Bring (to
/// `Downloads/Mickey`) or Ignore. The owner sets it (`[interface]
/// bring_over_gb`), 1 GB by default.
pub fn ask_over() -> u64 {
    interface().bring_over()
}

/// `[interface]` as config.toml says now; its defaults if it cannot be read.
fn interface() -> mickey_config::Interface {
    mickey_config::Config::load()
        .ok()
        .flatten()
        .map(|config| config.interface)
        .unwrap_or_default()
}

/// Asks the owner whether to bring `size` bytes of files offered by
/// `who` into `folder`: Bring or Ignore, with what they are (`mickey-ui
/// bring`, which shows sizes and ages as people read them). When `folder`
/// is worth cleaning first, the window says what is in it.
pub fn ask(offer: &Offer, who: &str, size: u64, folder: &Path) -> bool {
    let interface = interface();
    let limits = cleanup::Limits {
        over: interface.clean_over_gb.saturating_mul(1_000_000_000),
        below_percent: interface.clean_below_percent,
    };
    let items = cleanup::look(folder);
    let held = items
        .iter()
        .fold(0u64, |sum, item| sum.saturating_add(item.bytes));
    let worth = crate::platform::space::of(folder)
        .is_ok_and(|space| cleanup::worth_cleaning(held, size, space, limits));
    let (held, items) = if worth {
        (held, cleaning_lines(&items, SystemTime::now()))
    } else {
        (0, String::new())
    };
    let (size, held, folder) = (
        size.to_string(),
        held.to_string(),
        folder.display().to_string(),
    );
    let what = top_level(offer);
    let args = ["bring", who, &size, &what, &held, &folder, &items];
    let answer = crate::window::ui_command(&args).and_then(|mut ui| ui.status());
    matches!(answer.map(|status| status.code()), Ok(Some(0)))
}

/// The biggest few `items`, one line each for the window:
/// `<name>\t<bytes>\t<days since it arrived>\t<days since it changed>\t<1 if incomplete>`.
fn cleaning_lines(items: &[cleanup::Item], now: SystemTime) -> String {
    const SHOWN: usize = 5;
    let days = |at: SystemTime| now.duration_since(at).unwrap_or_default().as_secs() / 86_400;
    items
        .iter()
        .take(SHOWN)
        .map(|item| {
            let name = item.name.replace(['\t', '\n', '\r'], " ");
            format!(
                "{name}\t{}\t{}\t{}\t{}",
                item.bytes,
                days(item.arrived),
                days(item.changed),
                u8::from(item.incomplete)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// What `offer` holds, one line per copied file or folder (a folder with
/// how many files it holds), the first few only.
fn top_level(offer: &Offer) -> String {
    const SHOWN: usize = 4;
    let tops: Vec<&str> = offer
        .entries
        .iter()
        .filter(|entry| !entry.path.contains('/'))
        .map(|entry| entry.path.as_str())
        .collect();
    let mut lines: Vec<String> = tops
        .iter()
        .take(SHOWN)
        .map(|top| {
            let inside = offer
                .entries
                .iter()
                .filter(|entry| {
                    matches!(entry.kind, Kind::File { .. })
                        && entry
                            .path
                            .strip_prefix(top)
                            .is_some_and(|rest| rest.starts_with('/'))
                })
                .count();
            if inside > 0 {
                format!("{top}/  ({inside})")
            } else {
                top.to_string()
            }
        })
        .collect();
    if tops.len() > SHOWN {
        lines.push(format!("+{}", tops.len() - SHOWN));
    }
    lines.join("\n")
}

/// The bytes of every file in `offer`.
pub fn total(offer: &Offer) -> u64 {
    offer.entries.iter().fold(0, |sum, entry| match entry.kind {
        Kind::File { size } => sum.saturating_add(size),
        Kind::Folder => sum,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_bring_question_lists_the_copied_things_first_few_only() {
        let entry = |path: &str, kind| crate::transfer::offer::Entry {
            path: path.into(),
            kind,
        };
        let file = Kind::File { size: 1 };
        let mut entries = vec![
            entry("fotos", Kind::Folder),
            entry("fotos/a.jpg", file),
            entry("fotos/2026", Kind::Folder),
            entry("fotos/2026/b.jpg", file),
            entry("fotos-velhas.zip", file),
        ];
        entries.extend((0..5).map(|n| entry(&format!("{n}.txt"), file)));
        let text = top_level(&Offer { id: 1, entries });
        assert_eq!(text, "fotos/  (2)\nfotos-velhas.zip\n0.txt\n1.txt\n+3");
    }

    #[test]
    fn the_cleaning_hint_lists_the_biggest_few_with_their_age() {
        let now = SystemTime::now();
        let ago = |days: u64| now - Duration::from_secs(days * 86_400);
        let item = |name: &str, bytes, arrived, changed, incomplete| cleanup::Item {
            name: name.into(),
            bytes,
            arrived: ago(arrived),
            changed: ago(changed),
            incomplete,
        };
        let items: Vec<cleanup::Item> = (0..7)
            .map(|n| item(&format!("{n}.zip"), 7 - n, 3, 3, false))
            .chain([item("fotos", 0, 47, 47, false), item("x", 0, 1, 0, true)])
            .collect();
        let lines = cleaning_lines(&items, now);
        assert_eq!(lines.lines().count(), 5, "the first few only");
        assert_eq!(lines.lines().next(), Some("0.zip\t7\t3\t3\t0"));
        let fotos = [item("fotos", 9, 47, 47, false), item("x", 1, 1, 0, true)];
        assert_eq!(
            cleaning_lines(&fotos, now),
            "fotos\t9\t47\t47\t0\nx\t1\t1\t0\t1"
        );
    }
}
