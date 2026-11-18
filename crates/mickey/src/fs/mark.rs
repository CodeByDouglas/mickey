//! The Mark of the Web: Windows' note that a file came from elsewhere.

use std::path::Path;

/// Marks `path` as come from elsewhere, so Windows warns before running
/// it, as it does for a download.
pub fn as_downloaded(path: &Path) {
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    // ZoneId 3: the Internet zone.
    let _ = std::fs::write(stream, "[ZoneTransfer]\r\nZoneId=3\r\n");
}
