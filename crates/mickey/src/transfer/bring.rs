//! The question before a big transfer (PRD 0015): over `ASK_OVER`, the
//! owner says Bring (to `Downloads/Mickey`) or Ignore, seeing what the
//! files are (`mickey-ui bring`).

use crate::transfer::offer::{Kind, Offer};

/// Above this, a transfer is asked about first: Bring (to
/// `Downloads/Mickey`) or Ignore.
pub const ASK_OVER: u64 = 1 << 30;

/// Asks the owner whether to bring `size` bytes of files offered by
/// `who`: Bring or Ignore, with what they are (`mickey-ui bring`, which
/// shows the size as people read it).
pub fn ask(offer: &Offer, who: &str, size: u64) -> bool {
    let size = size.to_string();
    let answer = crate::window::ui_command(&["bring", who, &size, &top_level(offer)])
        .and_then(|mut ui| ui.status());
    matches!(answer.map(|status| status.code()), Ok(Some(0)))
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
}
