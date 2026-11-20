//! File names that come from the network.
//!
//! A name that could leave the folder it is written to (`..`, a slash, a
//! drive letter) is refused, never "cleaned up".

/// `name` if it is a plain file name, safe to join to a folder on any of
/// Mickey's systems: Windows also refuses `<>"|?*`, a name ending in a dot
/// or a space, and device names such as `CON` or `nul.txt`, which would
/// open the device instead of a file. The list is Windows' own (#483):
/// its superscript digits (`COM¹`) and the console's `CONIN$` and
/// `CONOUT$` are devices too.
pub fn safe_name(name: &str) -> Option<&str> {
    const DEVICES: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM0", "COM1", "COM2", "COM3", "COM4",
        "COM5", "COM6", "COM7", "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT0", "LPT1", "LPT2",
        "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
    ];
    let stem = name.split('.').next().unwrap_or("").trim_end();
    let bad = name.is_empty()
        || name.len() > 255
        || name.ends_with(['.', ' '])
        || DEVICES
            .iter()
            .any(|device| stem.eq_ignore_ascii_case(device))
        || name.chars().any(|c| {
            matches!(
                c,
                '/' | '\\' | ':' | '\0' | '<' | '>' | '"' | '|' | '?' | '*'
            ) || c.is_control()
                || hides_text(c)
        });
    (!bad).then_some(name)
}

/// Characters that change how a name reads without showing: direction
/// marks and overrides, which make `photo\u{202E}gpj.exe` show as
/// `photoexe.jpg`, and zero-width ones.
fn hides_text(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_names_are_safe() {
        assert_eq!(safe_name("report.pdf"), Some("report.pdf"));
        assert_eq!(safe_name("foto de férias.png"), Some("foto de férias.png"));
        assert_eq!(
            safe_name("console.txt"),
            Some("console.txt"),
            "only exact device names"
        );
        assert_eq!(safe_name(".bashrc"), Some(".bashrc"));
        for bad in [
            "",
            ".",
            "..",
            "../etc/passwd",
            "a/b",
            "a\\b",
            "C:evil",
            "x\0y",
            "tab\there",
            "CON",
            "nul.txt",
            "Com1.log",
            "trailing.",
            "trailing ",
            "what?.txt",
            "a|b",
            "COM¹",
            "lpt³.log",
            "CONIN$",
            "conout$.txt",
            "COM0",
            "photo\u{202E}gpj.exe",
            "a\u{200B}b",
        ] {
            assert_eq!(safe_name(bad), None, "{bad:?}");
        }
    }
}
