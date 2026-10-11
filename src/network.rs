//! Extras on the network card: how long a server takes to answer (the time to open a
//! connection to it, which needs no admin rights unlike a true ping), and how much was
//! downloaded and uploaded today and this month, kept in hub-usage.toml so a restart
//! doesn't lose it.

use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const USAGE_FILE: &str = "hub-usage.toml";

/// Connects to `host` ("1.1.1.1", "example.com", or with a port, "nas.local:5000"; port
/// 443 otherwise) and returns how long that took. Blocks for up to two seconds.
pub fn latency(host: &str) -> Result<Duration, String> {
    let host = host.trim();
    let target = if host.contains(':') && !host.starts_with('[') && host.matches(':').count() == 1 {
        host.to_string()
    } else {
        format!("{host}:443")
    };
    let address = target
        .to_socket_addrs()
        .map_err(|e| format!("couldn't find {host}: {e}"))?
        .next()
        .ok_or_else(|| format!("couldn't find {host}"))?;
    let started = Instant::now();
    TcpStream::connect_timeout(&address, Duration::from_secs(2)).map_err(|_| format!("no answer from {host}"))?;
    Ok(started.elapsed())
}

/// Bytes downloaded and uploaded on one day, and in its month.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct DailyUsage {
    /// "2026-10-11"
    pub date: String,
    pub received: u64,
    pub sent: u64,
    /// "2026-10"
    pub month: String,
    pub month_received: u64,
    pub month_sent: u64,
}

impl DailyUsage {
    pub fn path_beside(config_path: Option<&Path>) -> PathBuf {
        config_path
            .and_then(Path::parent)
            .map(|dir| dir.join(USAGE_FILE))
            .unwrap_or_else(|| PathBuf::from(USAGE_FILE))
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Err(err) = toml::to_string(self).map_err(|e| e.to_string()).and_then(|t| std::fs::write(path, t).map_err(|e| e.to_string())) {
            eprintln!("network: could not save {}: {err}", path.display());
        }
    }

    /// Counts more traffic, starting over when the day, or the month, changes. `today`
    /// is "YYYY-MM-DD".
    pub fn add(&mut self, today: &str, received: u64, sent: u64) {
        let month = today.get(..7).unwrap_or(today);
        if self.month != month {
            // Files from before monthly totals still hold the day so far, which counts too.
            let same_month = self.month.is_empty() && self.date.starts_with(month);
            self.month = month.to_string();
            self.month_received = if same_month { self.received } else { 0 };
            self.month_sent = if same_month { self.sent } else { 0 };
        }
        if self.date != today {
            self.date = today.to_string();
            self.received = 0;
            self.sent = 0;
        }
        self.received += received;
        self.sent += sent;
        self.month_received += received;
        self.month_sent += sent;
    }
}

/// 1_234_567_890 -> "1.1 GB", 5_000_000 -> "4.8 MB"
pub fn bytes(count: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    let mib = count as f64 / MIB;
    if mib >= 1024.0 {
        format!("{:.1} GB", mib / 1024.0)
    } else if mib >= 10.0 {
        format!("{mib:.0} MB")
    } else {
        format!("{mib:.1} MB")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_starts_over_each_day() {
        let mut usage = DailyUsage::default();
        usage.add("2026-10-11", 100, 10);
        usage.add("2026-10-11", 50, 5);
        assert_eq!((usage.received, usage.sent), (150, 15));
        usage.add("2026-10-12", 7, 1);
        assert_eq!((usage.date.as_str(), usage.received, usage.sent), ("2026-10-12", 7, 1));
        // The month keeps counting across days, and starts over with a new month.
        assert_eq!((usage.month.as_str(), usage.month_received, usage.month_sent), ("2026-10", 157, 16));
        usage.add("2026-11-01", 3, 2);
        assert_eq!((usage.month_received, usage.month_sent), (3, 2));
    }

    #[test]
    fn older_usage_files_still_load() {
        let mut usage: DailyUsage = toml::from_str("date = \"2026-10-11\"\nreceived = 5\nsent = 1\n").unwrap();
        assert_eq!((usage.received, usage.month.as_str()), (5, ""));
        // The month so far starts with the day so far.
        usage.add("2026-10-11", 1, 1);
        assert_eq!((usage.month_received, usage.month_sent), (6, 2));
    }

    #[test]
    fn usage_survives_a_restart() {
        let path = std::env::temp_dir().join(format!("pc-hub-usage-test-{}.toml", std::process::id()));
        let mut usage = DailyUsage::default();
        usage.add("2026-10-11", 123, 45);
        usage.save(&path);
        assert_eq!(DailyUsage::load(&path), usage);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn byte_counts() {
        assert_eq!(bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(bytes(300 * 1024 * 1024), "300 MB");
        assert_eq!(bytes(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
    }

    #[test]
    fn unknown_hosts_are_reported() {
        assert!(latency("no-such-host.invalid").is_err());
    }
}
