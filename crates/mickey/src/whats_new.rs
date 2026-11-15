//! After an update, the "What's new" window, once: Mickey keeps the last
//! version that ran in its data folder and opens `mickey-ui whats-new`
//! when this one is newer. A first install shows nothing: there is no
//! "before" to compare with. A PC that ran a Mickey from before this file
//! existed has an identity already, and that counts as "before".

use std::path::Path;

const FILE: &str = "last-version";

/// Whether a Mickey that last ran as `last` (`None`: unknown) shows the
/// notes as `now`; `known` says this PC had run Mickey before.
fn should_show(last: Option<&str>, known: bool, now: &str) -> bool {
    match last {
        Some(last) => last.trim() != now,
        None => known,
    }
}

/// Opens the window if this version is new on this PC, and remembers it.
/// Call it before the identity is made. Failing to read, write or open is
/// only logged: the notes are a nicety.
pub fn after_update(data_dir: &Path) {
    let path = data_dir.join(FILE);
    let now = env!("CARGO_PKG_VERSION");
    let last = std::fs::read_to_string(&path).ok();
    let known = data_dir.join(crate::net::identity::FILE_NAME).exists();
    // This version ran here already: nothing to show, nothing to write.
    if last.as_deref().map(str::trim) == Some(now) {
        return;
    }
    if let Err(err) = std::fs::create_dir_all(data_dir).and_then(|()| std::fs::write(&path, now)) {
        eprintln!("mickey: cannot remember the version that ran: {err}");
    }
    if should_show(last.as_deref(), known, now) {
        match crate::window::ui_command(&["whats-new"]).and_then(|mut ui| ui.spawn()) {
            // Waited for, so it leaves no zombie behind when closed.
            Ok(mut ui) => drop(std::thread::spawn(move || ui.wait())),
            Err(err) => eprintln!("mickey: cannot show what's new: {err}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notes_show_once_after_an_update_and_never_on_a_first_install() {
        assert!(should_show(Some("0.4.5"), true, "0.5.0"), "updated");
        assert!(
            !should_show(Some("0.5.0\n"), true, "0.5.0"),
            "already shown"
        );
        assert!(!should_show(None, false, "0.5.0"), "first install");
        assert!(
            should_show(None, true, "0.5.0"),
            "updated from a Mickey that kept no version"
        );
    }
}
