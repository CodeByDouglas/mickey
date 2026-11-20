//! Sizes and times as people read them, for the transfer windows.

/// Bytes as people read them: `850 KB`, `12.4 MB`, `1.3 GB`.
pub fn size(bytes: u64) -> String {
    let bytes = bytes as f64;
    if bytes >= 1e9 {
        format!("{:.1} GB", bytes / 1e9)
    } else if bytes >= 1e6 {
        format!("{:.1} MB", bytes / 1e6)
    } else {
        format!("{:.0} KB", bytes / 1e3)
    }
}

/// Seconds as people read them: `45 s`, `3 min`, `1 h 05 min`.
pub fn time(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3600 => format!("{} min", seconds.div_ceil(60)),
        _ => format!("{} h {:02} min", seconds / 3600, seconds % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_times_read_as_people_say_them() {
        assert_eq!(size(850_000), "850 KB");
        assert_eq!(size(12_400_000), "12.4 MB");
        assert_eq!(size(1_300_000_000), "1.3 GB");
        assert_eq!(time(45), "45 s");
        assert_eq!(time(130), "3 min");
        assert_eq!(time(3900), "1 h 05 min");
    }
}
