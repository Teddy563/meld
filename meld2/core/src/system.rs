//! CPU, memory and disk now, and a short history: Meld 1's System card
//! (CPU/RAM bars, disk free, a sparkline). CPU is the share of busy ticks
//! between two readings; memory "in use" is total minus available, as Task
//! Manager shows it. `None` where Meld cannot read a value (CPU and memory
//! on macOS).

use serde::Serialize;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Readings kept: 150 x 2 s = the last five minutes.
const KEEP: usize = 150;
const EVERY: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Default, Serialize)]
pub struct Sample {
    /// Unix seconds.
    pub t: u64,
    pub cpu_pct: Option<f64>,
    pub ram_used_mb: Option<u64>,
    pub ram_total_mb: Option<u64>,
    pub disk_free_mb: Option<u64>,
    pub disk_total_mb: Option<u64>,
}

/// Idle and total CPU ticks since boot.
#[cfg(windows)]
fn ticks() -> Option<(u64, u64)> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetSystemTimes;
    let z = || FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut idle, mut kernel, mut user) = (z(), z(), z());
    // SAFETY: three live out-pointers.
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return None;
    }
    let v = |f: FILETIME| (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64;
    // Kernel time includes idle time.
    Some((v(idle), v(kernel) + v(user)))
}

#[cfg(not(windows))]
fn ticks() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let n: Vec<u64> = stat
        .lines()
        .next()?
        .split_whitespace()
        .skip(1)
        .filter_map(|x| x.parse().ok())
        .collect();
    // user nice system idle iowait ...
    Some((n.get(3)? + n.get(4).unwrap_or(&0), n.iter().sum()))
}

/// Busy share between two `(idle, total)` readings, in percent.
pub fn busy_pct(a: (u64, u64), b: (u64, u64)) -> Option<f64> {
    let total = b.1.checked_sub(a.1)?;
    let idle = b.0.checked_sub(a.0)?;
    (total > 0).then(|| (100.0 * (1.0 - idle as f64 / total as f64)).clamp(0.0, 100.0))
}

/// Memory in use and in total, in MB.
#[cfg(windows)]
fn memory() -> Option<(u64, u64)> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: a zeroed struct with its length set, as the call requires.
    unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut m) == 0 {
            return None;
        }
        let mb = 1024 * 1024;
        Some(((m.ullTotalPhys - m.ullAvailPhys) / mb, m.ullTotalPhys / mb))
    }
}

#[cfg(not(windows))]
fn memory() -> Option<(u64, u64)> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb = |key: &str| -> Option<u64> {
        let line = info.lines().find(|l| l.starts_with(key))?;
        line.split_whitespace().nth(1)?.parse().ok()
    };
    let (total, avail) = (kb("MemTotal:")?, kb("MemAvailable:")?);
    Some(((total - avail) / 1024, total / 1024))
}

/// Samples every 2 s in a thread of its own, keeping the last five minutes.
pub struct Sampler {
    hist: Mutex<VecDeque<Sample>>,
}

impl Sampler {
    /// Starts sampling; `disk` is a path on the volume to report (the saves).
    pub fn start(disk: PathBuf) -> Arc<Self> {
        let s = Arc::new(Self {
            hist: Mutex::new(VecDeque::new()),
        });
        let me = Arc::clone(&s);
        std::thread::spawn(move || {
            let mut last = ticks();
            loop {
                std::thread::sleep(EVERY);
                let now = ticks();
                let sample = read(last.zip(now).and_then(|(a, b)| busy_pct(a, b)), &disk);
                last = now;
                let mut h = me.hist.lock().unwrap_or_else(|e| e.into_inner());
                h.push_back(sample);
                while h.len() > KEEP {
                    h.pop_front();
                }
            }
        });
        s
    }

    /// The readings, oldest first.
    pub fn history(&self) -> Vec<Sample> {
        let h = self.hist.lock().unwrap_or_else(|e| e.into_inner());
        h.iter().cloned().collect()
    }
}

/// One reading, with the CPU share measured by the caller.
pub fn read(cpu_pct: Option<f64>, disk: &Path) -> Sample {
    let (ram_used_mb, ram_total_mb) = memory().unzip();
    let (disk_free_mb, disk_total_mb) = crate::plan::space_mb(disk).ok().unzip();
    Sample {
        t: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        cpu_pct,
        ram_used_mb,
        ram_total_mb,
        disk_free_mb,
        disk_total_mb,
    }
}

/// A reading now, measuring CPU over `window`.
pub fn now(disk: &Path, window: Duration) -> Sample {
    let a = ticks();
    std::thread::sleep(window);
    let cpu = a.zip(ticks()).and_then(|(a, b)| busy_pct(a, b));
    read(cpu, disk)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_share_and_a_live_reading() {
        assert_eq!(busy_pct((100, 1000), (150, 1100)), Some(50.0));
        assert_eq!(busy_pct((100, 1000), (100, 1000)), None, "no time passed");
        assert_eq!(
            busy_pct((100, 1000), (90, 1100)),
            None,
            "counters went back"
        );
        let s = now(&std::env::temp_dir(), Duration::from_millis(200));
        assert!(s.disk_total_mb.unwrap() >= s.disk_free_mb.unwrap());
        if cfg!(any(windows, target_os = "linux")) {
            let (used, total) = (s.ram_used_mb.unwrap(), s.ram_total_mb.unwrap());
            assert!(total > 0 && used <= total);
            assert!((0.0..=100.0).contains(&s.cpu_pct.unwrap()));
        }
    }
}
