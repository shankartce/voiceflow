//! Machine description recorded in every results file.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Machine {
    pub cpu: String,
    pub physical_cores: Option<usize>,
    pub logical_cores: usize,
    pub ram_gb: f64,
    pub os: String,
    pub host: String,
}

pub fn describe() -> Machine {
    let mut sys = sysinfo::System::new();
    sys.refresh_cpu_all();
    sys.refresh_memory();
    let cpu = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| "unknown CPU".into());
    Machine {
        cpu,
        physical_cores: sysinfo::System::physical_core_count(),
        logical_cores: sys.cpus().len(),
        ram_gb: sys.total_memory() as f64 / 1_073_741_824.0,
        os: sysinfo::System::long_os_version().unwrap_or_else(|| std::env::consts::OS.into()),
        host: sysinfo::System::host_name().unwrap_or_else(|| "host".into()),
    }
}

/// Default inference threads: physical cores (hyper-threads rarely help GEMM-heavy inference).
pub fn default_threads() -> usize {
    sysinfo::System::physical_core_count()
        .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get()))
        .unwrap_or(4)
        .max(1)
}

/// Today's date (UTC) as `YYYY-MM-DD`, without a date crate.
pub fn today_utc() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's days-from-civil inverse.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// File-name-safe label (`[A-Za-z0-9-]`).
pub fn slug(s: &str) -> String {
    let out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "host".into()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(20_729), (2026, 10, 3));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("DESKTOP-AB12 (Shankar)"), "desktop-ab12--shankar");
        assert_eq!(slug("..."), "host");
    }
}
