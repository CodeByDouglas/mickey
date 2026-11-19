//! Paths inside an offer: relative, with `/` between folders, every part a
//! plain name. They come from the network on the receiving PC, so each
//! one is checked before it is joined to a folder there.

use crate::fs::names::safe_name;
use std::path::PathBuf;

/// The most folders deep a path may be.
pub const MAX_DEPTH: usize = 64;
/// The longest path, in bytes: Windows refuses paths over 260 characters
/// in all, and the folder files land in (`C:\Users\…\Downloads\Mickey`)
/// takes some of them.
pub const MAX_LEN: usize = 200;

/// `text` as a path to join to the folder files land in, if every part is
/// a plain name and it is neither too deep nor too long.
pub fn checked(text: &str) -> Result<PathBuf, String> {
    if text.len() > MAX_LEN {
        return Err(format!("a path longer than {MAX_LEN} bytes"));
    }
    let mut path = PathBuf::new();
    for (depth, part) in text.split('/').enumerate() {
        if depth == MAX_DEPTH {
            return Err(format!("a path more than {MAX_DEPTH} folders deep"));
        }
        let part =
            safe_name(part).ok_or_else(|| format!("refused a path from the network: {text:?}"))?;
        path.push(part);
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_relative_paths_pass() {
        assert_eq!(checked("fotos").unwrap(), PathBuf::from("fotos"));
        assert_eq!(
            checked("fotos/2026/praia.jpg").unwrap(),
            ["fotos", "2026", "praia.jpg"].iter().collect::<PathBuf>()
        );
    }

    #[test]
    fn paths_that_could_leave_the_folder_are_refused() {
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../../x",
            "a//b",
            "a/",
            "a\\b",
            "C:/x",
            "a/CON",
        ] {
            assert!(checked(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn too_deep_or_too_long_is_refused() {
        let deep = vec!["a"; MAX_DEPTH + 1].join("/");
        assert!(checked(&deep).is_err());
        assert!(checked(&vec!["a"; MAX_DEPTH].join("/")).is_ok());
        assert!(checked(&"x".repeat(MAX_LEN + 1)).is_err());
    }
}
