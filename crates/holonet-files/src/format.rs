//! Sizes, rates and time left as the SFTP screen writes them
//! (Sftp.dc.html: "48.2 MB", "4.2 MB/s", "4 s left", "3 min left").

/// "940 B", "6.1 KB", "48.2 MB", "1.3 GB" (1024-based, one decimal place
/// above bytes).
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// "4.2 MB/s"
pub fn rate(bytes_per_sec: f64) -> String {
    format!("{}/s", size(bytes_per_sec.max(0.0) as u64))
}

/// "4 s left", "3 min left", "2 h left"; `None` while the rate is unknown.
pub fn time_left(remaining: u64, bytes_per_sec: f64) -> Option<String> {
    if bytes_per_sec < 1.0 {
        return None;
    }
    let secs = (remaining as f64 / bytes_per_sec).ceil() as u64;
    Some(match secs {
        0..=59 => format!("{} s left", secs.max(1)),
        60..=3599 => format!("{} min left", secs.div_ceil(60)),
        _ => format!("{} h left", secs.div_ceil(3600)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_rates_and_time_left_read_as_drawn() {
        assert_eq!(size(72), "72 B");
        assert_eq!(size(940), "940 B");
        assert_eq!(size(6_246), "6.1 KB");
        assert_eq!(size(50_541_363), "48.2 MB");
        assert_eq!(size(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
        assert_eq!(rate(4.2 * 1_048_576.0), "4.2 MB/s");
        assert_eq!(time_left(4 * 1_048_576, 1_048_576.0).as_deref(), Some("4 s left"));
        assert_eq!(time_left(170 * 1_048_576, 1_048_576.0).as_deref(), Some("3 min left"));
        assert_eq!(time_left(1, 1_048_576.0).as_deref(), Some("1 s left"));
        assert_eq!(time_left(10, 0.0), None);
    }
}
