//! File names that come from the network.
//!
//! A name that could leave the folder it is written to (`..`, a slash, a
//! drive letter) is refused, never "cleaned up".

/// `name` if it is a plain file name, safe to join to a folder on any of
/// Mickey's systems: Windows also refuses `<>"|?*`, a name ending in a dot
/// or a space, and device names such as `CON` or `nul.txt`, which would
/// open the device instead of a file.
pub fn safe_name(name: &str) -> Option<&str> {
    const DEVICES: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
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
        });
    (!bad).then_some(name)
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
        ] {
            assert_eq!(safe_name(bad), None, "{bad:?}");
        }
    }
}
