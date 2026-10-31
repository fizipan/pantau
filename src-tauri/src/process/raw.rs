//! Reads the process list. Names, parents, command lines and CPU come from `sysinfo`; memory
//! comes from `proc_pid_rusage` because `sysinfo` reports RSS, while Activity Monitor shows
//! `phys_footprint`.

use std::path::PathBuf;

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[derive(Debug, Clone)]
pub struct RawProcess {
    pub pid: u32,
    pub ppid: u32,
    pub uid: u32,
    pub start_time: u64,
    pub name: String,
    pub exe: Option<PathBuf>,
    pub cmd: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// `None` when the process is gone or belongs to another user.
    pub footprint: Option<u64>,
    /// 0..100, already divided by the core count.
    pub cpu_pct: f32,
}

pub fn read_footprint(pid: u32) -> Option<u64> {
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::proc_pid_rusage(
            pid as libc::c_int,
            libc::RUSAGE_INFO_V4,
            &mut info as *mut _ as *mut libc::rusage_info_t,
        )
    };
    (rc == 0).then_some(info.ri_phys_footprint)
}

/// The parent from `PROC_PIDT_SHORTBSDINFO`, or 0 when the process is gone. macOS answers this
/// flavor for root and other users' processes, where it refuses the `PROC_PIDTBSDINFO` that sysinfo
/// reads. Without it `ghostty -> /usr/bin/login (root) -> zsh -> claude` is cut at `login`, and the
/// session looks unrelated to its terminal.
fn read_parent(pid: u32) -> u32 {
    /// `struct proc_bsdshortinfo`: `pbsi_pid`, then `pbsi_ppid`, 64 bytes in all.
    const FLAVOR: libc::c_int = 13;
    const SIZE: usize = 64;
    const PPID_AT: usize = 4;
    let mut buf = [0u8; SIZE];
    // SAFETY: `buf` outlives the call and the size passed is its length.
    let got = unsafe { libc::proc_pidinfo(pid as libc::c_int, FLAVOR, 0, buf.as_mut_ptr().cast(), SIZE as libc::c_int) };
    if got != SIZE as libc::c_int {
        return 0;
    }
    u32::from_ne_bytes([buf[PPID_AT], buf[PPID_AT + 1], buf[PPID_AT + 2], buf[PPID_AT + 3]])
}

pub struct ProcessReader {
    sys: System,
    read_once: bool,
}

impl ProcessReader {
    pub fn new() -> Self {
        Self { sys: System::new(), read_once: false }
    }

    /// False until the first `read`: CPU is a delta, so that read's CPU numbers are all 0.
    pub fn has_baseline(&self) -> bool {
        self.read_once
    }

    /// CPU is a delta since the previous call, so the first read reports 0 for everything.
    pub fn read(&mut self) -> Vec<RawProcess> {
        let kind = ProcessRefreshKind::nothing()
            .with_cpu()
            .with_cmd(UpdateKind::OnlyIfNotSet)
            .with_cwd(UpdateKind::Always)
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_user(UpdateKind::OnlyIfNotSet);
        self.sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
        self.read_once = true;
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f32;
        self.sys
            .processes()
            .values()
            .map(|p| RawProcess {
                pid: p.pid().as_u32(),
                ppid: p.parent().map_or_else(|| read_parent(p.pid().as_u32()), |pp| pp.as_u32()),
                uid: p.user_id().map_or(0, |u| **u),
                start_time: p.start_time(),
                name: p.name().to_string_lossy().into_owned(),
                exe: p.exe().map(PathBuf::from),
                cmd: p.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect(),
                cwd: p.cwd().map(PathBuf::from),
                footprint: read_footprint(p.pid().as_u32()),
                cpu_pct: (p.cpu_usage() / cores).clamp(0.0, 100.0),
            })
            .collect()
    }
}

impl Default for ProcessReader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footprint_of_this_test_process_is_readable_and_above_1_mb() {
        assert!(read_footprint(std::process::id()).unwrap() > 1_000_000);
    }

    #[test]
    fn footprint_of_a_pid_that_does_not_exist_is_none_without_panicking() {
        assert_eq!(read_footprint(u32::MAX - 1), None);
    }

    /// `(pid, ppid)` of every root process, as `ps` sees them. `ps` reads the kernel table, which
    /// is what the reader must agree with.
    fn root_parents_from_ps() -> Vec<(u32, u32)> {
        let out = std::process::Command::new("/bin/ps").args(["-axo", "pid=,ppid=,uid="]).output().unwrap();
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .filter_map(|l| {
                let f: Vec<u32> = l.split_whitespace().filter_map(|n| n.parse().ok()).collect();
                (f.len() == 3 && f[2] == 0 && f[0] > 1).then(|| (f[0], f[1]))
            })
            .collect()
    }

    #[test]
    fn root_processes_keep_their_real_parent() {
        // macOS refuses PROC_PIDTBSDINFO for these, so sysinfo reports no parent. A parent lost here
        // cuts `ghostty -> /usr/bin/login (root) -> zsh -> claude` in two, and hides the session.
        let truth = root_parents_from_ps();
        assert!(truth.len() > 10, "this machine always has root daemons: {}", truth.len());
        let all = ProcessReader::new().read();
        let wrong: Vec<_> = truth
            .iter()
            .filter_map(|&(pid, ppid)| all.iter().find(|p| p.pid == pid).filter(|p| p.ppid != ppid).map(|p| (pid, p.name.clone(), p.ppid, ppid)))
            .collect();
        assert!(wrong.is_empty(), "{} of {} wrong (pid, name, read, ps): {:?}", wrong.len(), truth.len(), &wrong[..wrong.len().min(5)]);
    }

    #[test]
    fn the_parent_of_a_pid_that_does_not_exist_is_0() {
        assert_eq!(read_parent(i32::MAX as u32 - 1), 0);
    }

    #[test]
    fn reader_finds_this_process_with_its_cwd_and_a_cpu_within_range() {
        let mut r = ProcessReader::new();
        r.read();
        let all = r.read();
        let me = all.iter().find(|p| p.pid == std::process::id()).unwrap();
        assert!(me.cwd.is_some());
        assert!((0.0..=100.0).contains(&me.cpu_pct));
    }
}
