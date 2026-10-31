//! Machine-wide metrics: CPU, memory, swap, memory pressure and disk. Cheap to read, so it runs
//! all the time; the menu bar title and the history graph both come from here.

use std::collections::VecDeque;
use std::ffi::CString;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

const HISTORY_LEN: usize = 30;
/// Disk usage is read from the Data volume. `/` is a read-only system snapshot and reports the wrong size.
const DATA_VOLUME: &str = "/System/Volumes/Data";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Pressure {
    Normal,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemSample {
    pub at_ms: u64,
    pub cpu_pct: f32,
    pub core_count: usize,
    pub ram_used: u64,
    pub ram_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub pressure: Pressure,
    pub disk_used: u64,
    pub disk_total: u64,
    pub disk_free: u64,
}

/// `kern.memorystatus_vm_pressure_level`: 1 normal, 2 warning, 4 critical.
pub fn pressure_from_level(level: i32) -> Pressure {
    match level {
        2 => Pressure::Warning,
        4 => Pressure::Critical,
        _ => Pressure::Normal,
    }
}

pub fn tray_title(s: &SystemSample) -> String {
    let ram = if s.ram_total == 0 { 0.0 } else { s.ram_used as f64 / s.ram_total as f64 * 100.0 };
    format!("CPU {:.0}%  RAM {:.0}%", s.cpu_pct, ram)
}

pub fn read_pressure() -> Pressure {
    let name = CString::new("kern.memorystatus_vm_pressure_level").unwrap();
    let mut level: i32 = 0;
    let mut size = size_of::<i32>();
    // SAFETY: `level` and `size` outlive the call and `size` matches the buffer.
    let rc = unsafe {
        libc::sysctlbyname(name.as_ptr(), (&mut level as *mut i32).cast(), &mut size, std::ptr::null_mut(), 0)
    };
    if rc == 0 { pressure_from_level(level) } else { Pressure::Normal }
}

/// Returns (used, total, free) in bytes, or zeros when the volume cannot be read.
fn read_disk() -> (u64, u64, u64) {
    let path = CString::new(DATA_VOLUME).unwrap();
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is NUL-terminated and `st` is a valid out-pointer.
    if unsafe { libc::statfs(path.as_ptr(), &mut st) } != 0 {
        return (0, 0, 0);
    }
    let bs = st.f_bsize as u64;
    let total = st.f_blocks * bs;
    let used = (st.f_blocks - st.f_bfree) * bs;
    (used, total, st.f_bavail * bs)
}

/// CPU usage is a delta between two refreshes, so keep the same `System` across calls.
pub fn read_system(sys: &mut sysinfo::System) -> SystemSample {
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    let (disk_used, disk_total, disk_free) = read_disk();
    SystemSample {
        at_ms: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64),
        cpu_pct: sys.global_cpu_usage(),
        core_count: sys.cpus().len(),
        ram_used: sys.used_memory(),
        ram_total: sys.total_memory(),
        swap_used: sys.used_swap(),
        swap_total: sys.total_swap(),
        pressure: read_pressure(),
        disk_used,
        disk_total,
        disk_free,
    }
}

/// The last 30 samples, 2 seconds apart.
#[derive(Default)]
pub struct History(VecDeque<SystemSample>);

impl History {
    pub fn push(&mut self, s: SystemSample) {
        if self.0.len() == HISTORY_LEN {
            self.0.pop_front();
        }
        self.0.push_back(s);
    }

    pub fn samples(&self) -> Vec<SystemSample> {
        self.0.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(cpu: f32, ram_used: u64, ram_total: u64) -> SystemSample {
        SystemSample {
            at_ms: 0, cpu_pct: cpu, core_count: 8,
            ram_used, ram_total, swap_used: 0, swap_total: 0,
            pressure: Pressure::Normal, disk_used: 0, disk_total: 0, disk_free: 0,
        }
    }

    fn sample_at(i: u64) -> SystemSample {
        SystemSample { at_ms: i, ..sample(0.0, 0, 1) }
    }

    #[test]
    fn pressure_levels_match_the_sysctl_values() {
        assert_eq!(pressure_from_level(1), Pressure::Normal);
        assert_eq!(pressure_from_level(2), Pressure::Warning);
        assert_eq!(pressure_from_level(4), Pressure::Critical);
        assert_eq!(pressure_from_level(0), Pressure::Normal);
    }

    #[test]
    fn tray_title_rounds_to_whole_percent() {
        let s = sample(62.6, 79, 100);
        assert_eq!(tray_title(&s), "CPU 63%  RAM 79%");
    }

    #[test]
    fn history_keeps_only_the_last_30_samples_at_2_second_steps() {
        let mut h = History::default();
        for i in 0..35 { h.push(sample_at(i)); }
        let all = h.samples();
        assert_eq!(all.len(), 30);
        assert_eq!(all[0].at_ms, 5);
    }

    #[test]
    fn pressure_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Pressure::Critical).unwrap(), "\"critical\"");
    }

    #[test]
    fn read_system_returns_plausible_numbers_on_this_machine() {
        let mut sys = sysinfo::System::new();
        let s = read_system(&mut sys);
        assert!(s.core_count > 0);
        assert!(s.ram_total > 0 && s.ram_used <= s.ram_total);
        assert!(s.disk_total > 0 && s.disk_used <= s.disk_total && s.disk_free <= s.disk_total);
    }
}
