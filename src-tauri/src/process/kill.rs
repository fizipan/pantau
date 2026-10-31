//! Safe kill (spec 5.4): who may never be killed, what a kill takes down with it, and signals
//! that only reach the exact process the owner clicked.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::raw::RawProcess;
use super::rules::{is_agent_session, match_tool};

/// How long the UI waits after SIGTERM before offering `Paksa`.
pub const TERM_WAIT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcId {
    pub pid: u32,
    pub start_time: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillBlock {
    Root,
    OtherUser,
    System,
    Itself,
}

pub fn kill_block(p: &RawProcess, me_uid: u32, self_pid: u32) -> Option<KillBlock> {
    if p.pid == self_pid {
        Some(KillBlock::Itself)
    } else if p.uid == 0 {
        Some(KillBlock::Root)
    } else if p.uid != me_uid {
        Some(KillBlock::OtherUser)
    } else if p.exe.as_deref().is_some_and(|e| e.starts_with("/System")) {
        Some(KillBlock::System)
    } else {
        None
    }
}

/// Refuses a target the spec says cannot be killed. The UI's `killable` flag is only a hint, so
/// commands must call this on a fresh process list. A target missing from `raw`, or whose start
/// time differs, is not blocked here: `send` already treats gone and reused PIDs.
pub fn authorize(target: &ProcId, raw: &[RawProcess], me_uid: u32, self_pid: u32) -> Result<(), String> {
    let found = raw.iter().find(|p| p.pid == target.pid && p.start_time == target.start_time);
    match found.and_then(|p| kill_block(p, me_uid, self_pid)) {
        None => Ok(()),
        Some(KillBlock::Root) => Err("Proses milik root tidak boleh dihentikan".into()),
        Some(KillBlock::OtherUser) => Err("Proses milik pengguna lain tidak boleh dihentikan".into()),
        Some(KillBlock::System) => Err("Proses sistem macOS tidak boleh dihentikan".into()),
        Some(KillBlock::Itself) => Err("Pantau tidak bisa menghentikan dirinya sendiri".into()),
    }
}

fn errno_message(errno: Option<i32>) -> String {
    match errno {
        Some(libc::EPERM) => "Tidak bisa dihentikan: izin ditolak".into(),
        _ => "Tidak bisa dihentikan: terjadi kesalahan sistem".into(),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct KillPlan {
    pub needs_confirm: bool,
    pub warnings: Vec<String>,
}

/// Sessions that would die with the targets: agent sessions among their descendants that are not
/// targets themselves. A kill takes the whole subtree down with it when the target is a terminal.
pub fn plan_kill(
    targets: &[ProcId],
    raw: &[RawProcess],
    project_of: &dyn Fn(&Path) -> Option<String>,
) -> KillPlan {
    let mut children: HashMap<u32, Vec<&RawProcess>> = HashMap::new();
    for p in raw {
        children.entry(p.ppid).or_default().push(p);
    }
    let is_target = |pid: u32| targets.iter().any(|t| t.pid == pid);

    let mut warnings: Vec<String> = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut stack: Vec<u32> = raw
        .iter()
        .filter(|p| targets.iter().any(|t| t.pid == p.pid && t.start_time == p.start_time))
        .map(|p| p.pid)
        .collect();
    seen.extend(&stack);
    while let Some(pid) = stack.pop() {
        for child in children.get(&pid).into_iter().flatten() {
            if !seen.insert(child.pid) {
                continue;
            }
            stack.push(child.pid);
            if is_agent_session(child) && !is_target(child.pid) {
                let label = match_tool(child).map_or("AI", |(_, l)| l);
                let warning = match child.cwd.as_deref().and_then(project_of) {
                    Some(project) => format!("Sesi {label} di {project} ikut mati"),
                    None => format!("Sesi {label} ikut mati"),
                };
                if !warnings.contains(&warning) {
                    warnings.push(warning);
                }
            }
        }
    }
    let session_in_targets = raw.iter().any(|p| is_target(p.pid) && is_agent_session(p));
    warnings.sort();
    KillPlan {
        needs_confirm: targets.len() > 1 || session_in_targets || !warnings.is_empty(),
        warnings,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum KillOutcome {
    Exited,
    StillAlive,
}

/// Start time in the same units and from the same source as `RawProcess.start_time` (seconds
/// since the epoch). Read with `sysctl(KERN_PROC_PID)`, which works for other users' and root
/// processes too, unlike `proc_pidinfo`, so a live process we may not signal is never mistaken
/// for an exited one. `None` only when the process is gone or a zombie.
pub fn read_start(pid: u32) -> Option<u64> {
    // Prefix of `struct kinfo_proc` (macOS): `extern_proc` begins with `p_starttime` (a timeval),
    // `p_stat` sits at byte 36 and `p_pid` at byte 40.
    const KINFO_PROC_SIZE: usize = 648;
    const STAT_AT: usize = 36;
    const PID_AT: usize = 40;
    if pid > i32::MAX as u32 {
        return None;
    }
    let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid as libc::c_int];
    let mut buf = [0u8; KINFO_PROC_SIZE];
    let mut len = buf.len();
    let rc = unsafe {
        libc::sysctl(mib.as_mut_ptr(), 4, buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0)
    };
    if rc != 0 || len < PID_AT + 4 {
        return None;
    }
    let found_pid = i32::from_ne_bytes(buf[PID_AT..PID_AT + 4].try_into().ok()?);
    if found_pid != pid as i32 || u32::from(buf[STAT_AT]) == libc::SZOMB {
        return None;
    }
    let secs = i64::from_ne_bytes(buf[..8].try_into().ok()?);
    u64::try_from(secs).ok()
}

/// `kill(2)` with pid 0 or a negative value signals whole process groups, and pid 1 is launchd.
fn signalable(pid: u32) -> bool {
    pid > 1 && pid <= i32::MAX as u32
}

fn send(target: ProcId, signal: libc::c_int, read_start: &dyn Fn(u32) -> Option<u64>) -> Result<(), String> {
    if !signalable(target.pid) || target.pid == std::process::id() {
        return Err(format!("PID {} tidak boleh dikirimi sinyal", target.pid));
    }
    match read_start(target.pid) {
        None => return Ok(()),
        Some(start) if start != target.start_time => {
            return Err("PID ini sudah dipakai proses lain, tidak dikirimi sinyal".into());
        }
        Some(_) => {}
    }
    // Small window between the check and the signal is inherent to kill(2) by pid.
    if unsafe { libc::kill(target.pid as libc::pid_t, signal) } == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::ESRCH) => Ok(()),
        errno => Err(errno_message(errno)),
    }
}

pub fn send_term(target: ProcId, read_start: &dyn Fn(u32) -> Option<u64>) -> Result<(), String> {
    send(target, libc::SIGTERM, read_start)
}

pub fn send_force(target: ProcId, read_start: &dyn Fn(u32) -> Option<u64>) -> Result<(), String> {
    send(target, libc::SIGKILL, read_start)
}

/// Polls until every target is gone (or its pid belongs to a new process), or `timeout` passes.
pub fn wait_exit(
    targets: &[ProcId],
    read_start: &dyn Fn(u32) -> Option<u64>,
    timeout: Duration,
) -> KillOutcome {
    let deadline = Instant::now() + timeout;
    loop {
        let alive = targets.iter().any(|t| read_start(t.pid) == Some(t.start_time));
        if !alive {
            return KillOutcome::Exited;
        }
        if Instant::now() >= deadline {
            return KillOutcome::StillAlive;
        }
        std::thread::sleep(POLL.min(deadline - Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::process::{Child, Command, Stdio};

    use super::*;
    use crate::process::test_support::proc;

    const ME: u32 = 501;
    const SELF: u32 = 4242;

    fn name_of(p: &Path) -> Option<String> {
        p.file_name().map(|n| n.to_string_lossy().into_owned())
    }

    fn id(pid: u32) -> ProcId {
        ProcId { pid, start_time: 0 }
    }

    fn ids(raw: &[RawProcess]) -> Vec<ProcId> {
        raw.iter().map(|p| id(p.pid)).collect()
    }

    #[test]
    fn root_other_users_system_paths_and_pantau_itself_are_blocked() {
        assert_eq!(kill_block(&proc(5, 1).uid(0), ME, SELF), Some(KillBlock::Root));
        assert_eq!(kill_block(&proc(5, 1).uid(777), ME, SELF), Some(KillBlock::OtherUser));
        assert_eq!(
            kill_block(&proc(5, 1).uid(ME).exe("/System/Library/x"), ME, SELF),
            Some(KillBlock::System)
        );
        assert_eq!(kill_block(&proc(SELF, 1).uid(ME), ME, SELF), Some(KillBlock::Itself));
        assert_eq!(kill_block(&proc(5, 1).uid(ME).exe("/usr/bin/node"), ME, SELF), None);
    }

    #[test]
    fn one_plain_process_needs_no_confirmation() {
        let raw = [proc(5, 1).name("node").cmd(&["node", "worker.js"])];
        let plan = plan_kill(&ids(&raw), &raw, &name_of);
        assert!(!plan.needs_confirm);
        assert!(plan.warnings.is_empty());
    }

    #[test]
    fn several_processes_or_an_agent_session_need_confirmation() {
        let two = [proc(5, 1).name("node"), proc(6, 1).name("node")];
        assert!(plan_kill(&ids(&two), &two, &name_of).needs_confirm);

        let session = [proc(7, 1).name("claude").cmd(&["claude"]).cwd("/w/a")];
        assert!(plan_kill(&ids(&session), &session, &name_of).needs_confirm);
    }

    #[test]
    fn killing_a_terminal_that_hosts_a_session_names_the_session() {
        // The shape the reader really produces for `login`: root, and sysinfo could read nothing
        // but its name, so no command line, memory or start time.
        let mut login = proc(2, 1).name("login").uid(0).no_mem().exe("/usr/bin/login");
        login.start_time = 0;
        let raw = [
            proc(1, 0).exe("/Applications/Ghostty.app/Contents/MacOS/ghostty"),
            login,
            proc(3, 2).name("zsh"),
            proc(4, 3).name("claude").cmd(&["claude"]).cwd("/w/TOOLS-LAB"),
        ];
        let plan = plan_kill(&[id(1)], &raw, &name_of);
        assert!(plan.needs_confirm);
        assert_eq!(plan.warnings, vec!["Sesi Claude Code di TOOLS-LAB ikut mati"]);
    }

    #[test]
    fn a_session_with_no_project_is_still_named_and_cycles_do_not_hang() {
        let raw = [
            proc(1, 2).name("zsh"),
            proc(2, 1).name("zsh"),
            proc(3, 1).name("codex").cmd(&["codex"]),
        ];
        let plan = plan_kill(&[id(1)], &raw, &name_of);
        assert_eq!(plan.warnings, vec!["Sesi Codex ikut mati"]);
    }

    #[test]
    fn a_stale_target_does_not_pull_in_the_new_process_with_its_pid() {
        let raw = [
            proc(1, 0).name("zsh"),
            proc(2, 1).name("claude").cmd(&["claude"]).cwd("/w/a"),
        ];
        let stale = ProcId { pid: 1, start_time: 999 };
        let plan = plan_kill(&[stale], &raw, &name_of);
        assert!(plan.warnings.is_empty());
    }

    #[test]
    fn real_root_processes_stay_blocked_with_their_parent_filled_in() {
        // Only reads. `login` and every other process sysinfo cannot read must still count as root.
        let out = Command::new("/bin/ps").args(["-axo", "pid=,uid="]).output().unwrap();
        let roots: Vec<u32> = String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .filter_map(|l| {
                let f: Vec<u32> = l.split_whitespace().filter_map(|n| n.parse().ok()).collect();
                (f.len() == 2 && f[1] == 0 && f[0] > 1).then(|| f[0])
            })
            .collect();
        let raw = crate::process::raw::ProcessReader::new().read();
        let checked: Vec<&RawProcess> = raw.iter().filter(|p| roots.contains(&p.pid)).collect();
        assert!(checked.len() > 10, "{}", checked.len());
        let me = unsafe { libc::getuid() };
        for p in checked {
            assert_eq!(kill_block(p, me, std::process::id()), Some(KillBlock::Root), "{} ({})", p.name, p.pid);
        }
    }

    /// Asks what killing the real Ghostty would warn about, and prints it. Sends no signal:
    /// `cargo test print_real_ghostty_plan -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads the real process list; sends nothing"]
    fn print_real_ghostty_plan() {
        let mut reader = crate::process::raw::ProcessReader::new();
        let raw = reader.read();
        let unreadable = raw.iter().filter(|p| p.ppid == 0).count();
        println!("{} processes, {unreadable} with ppid 0", raw.len());
        let terminals: Vec<&RawProcess> = raw.iter().filter(|p| p.exe.as_deref().is_some_and(|e| e.to_string_lossy().contains("Ghostty.app"))).collect();
        println!("Ghostty processes: {:?}", terminals.iter().map(|p| p.pid).collect::<Vec<_>>());
        for p in terminals {
            let plan = plan_kill(&[ProcId { pid: p.pid, start_time: p.start_time }], &raw, &crate::process::group::project_name);
            println!("pid {}: needs_confirm={} warnings={:?}", p.pid, plan.needs_confirm, plan.warnings);
        }
    }

    #[test]
    fn a_reused_pid_is_never_signalled() {
        let target = ProcId { pid: 77, start_time: 1000 };
        assert!(send_term(target, &|_| Some(2000)).is_err());
        assert!(send_force(target, &|_| Some(2000)).is_err());
    }

    #[test]
    fn a_process_that_already_exited_counts_as_exited() {
        let target = ProcId { pid: 77, start_time: 1000 };
        assert!(send_term(target, &|_| None).is_ok());
        assert!(send_force(target, &|_| None).is_ok());
        assert!(matches!(wait_exit(&[target], &|_| None, Duration::ZERO), KillOutcome::Exited));
    }

    #[test]
    fn pids_that_would_signal_a_group_or_init_are_refused_even_if_the_start_matches() {
        // If the guard regressed, a matching start would reach kill(0|-1, ..) and hit whole groups.
        let never = |_: u32| -> Option<u64> { panic!("guard harus menolak sebelum read_start") };
        for pid in [0, 1, i32::MAX as u32 + 1, u32::MAX] {
            let target = ProcId { pid, start_time: 5 };
            assert!(send_term(target, &never).is_err(), "pid {pid}");
            assert!(send_force(target, &never).is_err(), "pid {pid}");
        }
    }

    #[test]
    fn this_pantau_process_is_never_signalled_even_if_the_start_matches() {
        let me = ProcId { pid: std::process::id(), start_time: 5 };
        let never = |_: u32| -> Option<u64> { panic!("guard harus menolak sebelum read_start") };
        assert!(send_term(me, &never).is_err());
        assert!(send_force(me, &never).is_err());
    }

    #[test]
    fn authorize_blocks_by_kill_block_and_leaves_gone_or_reused_pids_to_the_start_check() {
        let raw = [
            proc(10, 1).uid(0),
            proc(11, 1).uid(777),
            proc(12, 1).exe("/System/Library/CoreServices/Dock.app/Contents/MacOS/Dock"),
            proc(SELF, 1),
            proc(13, 1).exe("/usr/bin/node"),
        ];
        let ok = |pid| authorize(&id(pid), &raw, ME, SELF);
        assert!(ok(10).is_err() && ok(11).is_err() && ok(12).is_err() && ok(SELF).is_err());
        assert!(ok(13).is_ok());
        assert!(ok(999).is_ok(), "absent target is left to send's start check");
        let reused = ProcId { pid: 10, start_time: 55 };
        assert!(authorize(&reused, &raw, ME, SELF).is_ok());
    }

    #[test]
    fn errno_messages_are_indonesian() {
        assert_eq!(errno_message(Some(libc::EPERM)), "Tidak bisa dihentikan: izin ditolak");
        let other = errno_message(Some(libc::EIO));
        assert!(other.starts_with("Tidak bisa dihentikan"), "{other}");
        assert!(!other.contains("os error"));
        assert!(!errno_message(None).contains("os error"));
    }

    #[test]
    fn read_start_sees_processes_it_may_not_signal() {
        // launchd belongs to root; reading must work, and nothing is signalled here.
        assert!(read_start(1).is_some());
        assert_eq!(read_start(i32::MAX as u32), None);
    }

    #[test]
    fn wait_exit_reports_still_alive_when_the_start_still_matches() {
        let target = ProcId { pid: 77, start_time: 1000 };
        let polls = Cell::new(0);
        let read = |_: u32| {
            polls.set(polls.get() + 1);
            Some(1000)
        };
        assert_eq!(wait_exit(&[target], &read, Duration::from_millis(250)), KillOutcome::StillAlive);
        assert!(polls.get() >= 2);
    }

    #[test]
    fn a_reused_pid_counts_as_exited_when_waiting() {
        let target = ProcId { pid: 77, start_time: 1000 };
        assert_eq!(wait_exit(&[target], &|_| Some(2000), Duration::ZERO), KillOutcome::Exited);
    }

    /// A spawned test child that is killed and reaped when dropped, so a failed assert leaks nothing.
    struct Guard(Child);

    impl std::ops::Deref for Guard {
        type Target = Child;
        fn deref(&self) -> &Child {
            &self.0
        }
    }

    impl std::ops::DerefMut for Guard {
        fn deref_mut(&mut self) -> &mut Child {
            &mut self.0
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn spawn(script: &str) -> Guard {
        Guard(
            Command::new("sh")
                .args(["-c", script])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }

    /// Waits for the shell to print "ready", so the TERM trap is installed before we signal.
    fn ready(child: &mut Child) {
        use std::io::Read;
        let mut buf = [0u8; 5];
        child.stdout.as_mut().unwrap().read_exact(&mut buf).unwrap();
    }

    fn id_of(child: &Child) -> ProcId {
        let pid = child.id();
        ProcId { pid, start_time: read_start(pid).expect("child is alive") }
    }

    #[test]
    fn read_start_matches_what_sysinfo_reports() {
        let mut child = spawn("echo ready; exec sleep 60");
        ready(&mut child);
        let pid = child.id();
        let mut reader = crate::process::raw::ProcessReader::new();
        let from_sysinfo = reader.read().into_iter().find(|p| p.pid == pid).unwrap().start_time;
        assert_eq!(read_start(pid), Some(from_sysinfo));
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(read_start(pid), None);
    }

    #[test]
    fn term_stops_a_real_child_and_force_stops_one_that_ignores_term() {
        let mut polite = spawn("echo ready; exec sleep 60");
        ready(&mut polite);
        let target = id_of(&polite);
        send_term(target, &read_start).unwrap();
        // Zombies count as gone, so the test does not need to reap before waiting.
        assert_eq!(wait_exit(&[target], &read_start, Duration::from_secs(5)), KillOutcome::Exited);
        polite.wait().unwrap();

        let mut stubborn = spawn(r#"trap "" TERM; echo ready; while :; do sleep 1; done"#);
        ready(&mut stubborn);
        let target = id_of(&stubborn);
        send_term(target, &read_start).unwrap();
        assert_eq!(wait_exit(&[target], &read_start, Duration::from_secs(1)), KillOutcome::StillAlive);
        send_force(target, &read_start).unwrap();
        assert_eq!(wait_exit(&[target], &read_start, Duration::from_secs(5)), KillOutcome::Exited);
        stubborn.wait().unwrap();
    }
}
