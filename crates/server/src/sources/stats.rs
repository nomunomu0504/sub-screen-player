//! System figures for the dashboard: CPU, memory, network and disk, with a short history.

use std::collections::VecDeque;
use std::path::Path;
use std::time::{Duration, Instant};

use sysinfo::{Disks, Networks, System};

/// How many samples the graphs keep (one per refresh, so about a minute).
pub const HISTORY: usize = 60;

/// Disks change slowly; their list and sizes are refreshed this often.
const DISK_INTERVAL: Duration = Duration::from_secs(10);

/// Samples closer together than this are skipped: CPU use cannot be measured over a shorter
/// time, and dividing a few bytes by a tiny time would show a huge network speed.
const MIN_SAMPLE_INTERVAL: Duration = Duration::from_millis(250);

/// The latest figures.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    /// CPU use over all cores, 0-100.
    pub cpu_percent: f32,
    /// Number of logical CPUs.
    pub cpu_count: usize,
    /// One-minute load average; `None` where the system has none (Windows).
    pub load: Option<f64>,
    /// Memory in use, bytes.
    pub memory_used: u64,
    /// Installed memory, bytes.
    pub memory_total: u64,
    /// Received bytes per second over all interfaces except loopback.
    pub rx_per_sec: f64,
    /// Sent bytes per second over all interfaces except loopback.
    pub tx_per_sec: f64,
    /// Used and total bytes of the system disk, if found.
    pub disk: Option<(u64, u64)>,
}

impl Snapshot {
    /// Memory in use, 0-100.
    pub fn memory_percent(&self) -> f32 {
        percent(self.memory_used, self.memory_total)
    }
}

/// Share of `part` in `total`, 0-100.
pub fn percent(part: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (part as f64 / total as f64 * 100.0) as f32
    }
}

/// Recent values, oldest first.
#[derive(Debug, Clone, Default)]
pub struct History(VecDeque<f32>);

impl History {
    /// Appends a value, dropping the oldest beyond [`HISTORY`].
    pub fn push(&mut self, value: f32) {
        if self.0.len() == HISTORY {
            self.0.pop_front();
        }
        self.0.push_back(value);
    }

    /// The values, oldest first.
    pub fn values(&self) -> Vec<f32> {
        self.0.iter().copied().collect()
    }

    /// The largest value, or 0.
    pub fn max(&self) -> f32 {
        self.0.iter().copied().fold(0.0, f32::max)
    }
}

/// Reads the system's figures and keeps their history.
pub struct Stats {
    system: System,
    networks: Networks,
    disks: Disks,
    disks_read: Instant,
    last: Instant,
    /// The latest figures.
    pub now: Snapshot,
    /// CPU use history, 0-100.
    pub cpu: History,
    /// Memory use history, 0-100.
    pub memory: History,
    /// Received bytes per second.
    pub rx: History,
    /// Sent bytes per second.
    pub tx: History,
}

impl Stats {
    /// Starts measuring. The first [`Stats::refresh`] gives rates over the time since this call.
    pub fn new() -> Self {
        let mut system = System::new();
        system.refresh_cpu_usage();
        Self {
            system,
            networks: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
            disks_read: Instant::now(),
            last: Instant::now(),
            now: Snapshot::default(),
            cpu: History::default(),
            memory: History::default(),
            rx: History::default(),
            tx: History::default(),
        }
    }

    /// Takes a new sample. Call it about once a second; calls right after the previous one (or
    /// after [`Stats::new`]) are ignored.
    pub fn refresh(&mut self) {
        let elapsed = self.last.elapsed();
        if elapsed < min_interval() {
            return;
        }
        let elapsed = elapsed.as_secs_f64();
        self.last = Instant::now();
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.networks.refresh(true);
        if self.disks_read.elapsed() >= DISK_INTERVAL {
            self.disks.refresh(true);
            self.disks_read = Instant::now();
        }

        let (rx, tx) = self
            .networks
            .iter()
            .filter(|(name, _)| !is_loopback(name))
            .fold((0, 0), |(rx, tx), (_, data)| {
                (rx + data.received(), tx + data.transmitted())
            });
        let load = System::load_average().one;
        self.now = Snapshot {
            cpu_percent: self.system.global_cpu_usage().clamp(0.0, 100.0),
            cpu_count: self.system.cpus().len(),
            load: (!cfg!(windows)).then_some(load),
            memory_used: self.system.used_memory(),
            memory_total: self.system.total_memory(),
            rx_per_sec: rx as f64 / elapsed,
            tx_per_sec: tx as f64 / elapsed,
            disk: system_disk(&self.disks),
        };
        self.cpu.push(self.now.cpu_percent);
        self.memory.push(self.now.memory_percent());
        self.rx.push(self.now.rx_per_sec as f32);
        self.tx.push(self.now.tx_per_sec as f32);
    }
}

impl Stats {
    /// Like [`Stats::refresh`], but before the first sample it waits until one can be taken,
    /// so the figures are never empty.
    pub fn refresh_or_wait(&mut self) {
        if self.cpu.0.is_empty() {
            std::thread::sleep(min_interval().saturating_sub(self.last.elapsed()));
        }
        self.refresh();
    }
}

fn min_interval() -> Duration {
    MIN_SAMPLE_INTERVAL.max(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL)
}

impl Default for Stats {
    fn default() -> Self {
        Self::new()
    }
}

fn is_loopback(name: &str) -> bool {
    name == "lo" || name.starts_with("lo0") || name.starts_with("Loopback")
}

/// Used and total bytes of the disk holding the system: `/` on macOS and Linux, the system drive
/// (usually `C:\`) on Windows.
fn system_disk(disks: &Disks) -> Option<(u64, u64)> {
    let root = if cfg!(windows) {
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        format!("{drive}\\")
    } else {
        "/".to_owned()
    };
    let disk = disks.iter().find(|d| d.mount_point() == Path::new(&root))?;
    let total = disk.total_space();
    Some((total.saturating_sub(disk.available_space()), total))
}

/// Formats a byte count with a decimal unit (KB = 1000 B), e.g. `"512 GB"` or `"1.25 TB"`.
pub fn bytes(value: u64) -> String {
    scaled(value as f64, &["B", "KB", "MB", "GB", "TB", "PB"])
}

/// Formats a rate in bytes per second, e.g. `"12.3 MB/s"`.
pub fn rate(per_sec: f64) -> String {
    scaled(per_sec, &["B/s", "KB/s", "MB/s", "GB/s", "TB/s"])
}

/// Formats memory with a binary unit (GB = 1024³ B), as operating systems show it.
pub fn memory(value: u64) -> String {
    format!("{:.1} GB", value as f64 / f64::from(1u32 << 30))
}

fn scaled(mut value: f64, units: &[&str]) -> String {
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < units.len() {
        value /= 1000.0;
        unit += 1;
    }
    let digits = if unit == 0 || value >= 100.0 {
        0
    } else if value >= 10.0 {
        1
    } else {
        2
    };
    format!("{value:.digits$} {}", units[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sizes_and_rates() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(512_000_000_000), "512 GB");
        assert_eq!(bytes(1_250_000_000_000), "1.25 TB");
        assert_eq!(rate(12_345_678.0), "12.3 MB/s");
        assert_eq!(rate(950.0), "950 B/s");
        assert_eq!(memory(16 << 30), "16.0 GB");
    }

    #[test]
    fn history_keeps_the_latest_values() {
        let mut history = History::default();
        for i in 0..(HISTORY + 5) {
            history.push(i as f32);
        }
        let values = history.values();
        assert_eq!(values.len(), HISTORY);
        assert_eq!(values[0], 5.0);
        assert_eq!(history.max(), (HISTORY + 4) as f32);
    }

    #[test]
    fn reads_the_system() {
        let mut stats = Stats::new();
        // Too soon after starting: no sample yet.
        stats.refresh();
        assert!(stats.cpu.values().is_empty());
        std::thread::sleep(MIN_SAMPLE_INTERVAL.max(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL));
        stats.refresh();
        assert!(stats.now.cpu_count > 0);
        assert!(stats.now.memory_total > 0);
        assert!((0.0..=100.0).contains(&stats.now.cpu_percent));
        assert_eq!(stats.cpu.values().len(), 1);
    }
}
