//! Listening TCP ports per process, read from `lsof`.

use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const LSOF_TIMEOUT: Duration = Duration::from_secs(4);

/// Parses `lsof -Fpn` output: a `p<pid>` line, then one `n<address>` line per socket of that pid.
pub fn parse_lsof(output: &str) -> HashMap<u32, Vec<u16>> {
    let mut ports: HashMap<u32, Vec<u16>> = HashMap::new();
    let mut pid = None;
    for line in output.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().ok();
        } else if let (Some(addr), Some(pid)) = (line.strip_prefix('n'), pid)
            && let Some(port) = addr.rsplit(':').next().and_then(|p| p.parse().ok())
        {
            ports.entry(pid).or_default().push(port);
        }
    }
    for v in ports.values_mut() {
        v.sort_unstable();
        v.dedup();
    }
    ports
}

/// `lsof` exits 1 when it finds nothing, which is a valid answer. Any other code, or a kill by
/// signal (`None`), means the read failed and says nothing about who listens.
fn interpret_lsof(code: Option<i32>, stdout: &str) -> Option<HashMap<u32, Vec<u16>>> {
    matches!(code, Some(0 | 1)).then(|| parse_lsof(stdout))
}

/// Runs `cmd`, killing it after `timeout`. `None` when it cannot start, hangs, or is killed.
pub(crate) fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Option<(Option<i32>, String)> {
    let (code, out) = spawn_with_timeout(cmd, timeout).ok().flatten()?;
    Some((code, out?))
}

/// Like `run_with_timeout`, but keeps the reason a program could not start (not found, no
/// permission), and returns the exit code even when the output did not arrive in time (`None`).
pub(crate) fn spawn_with_timeout(cmd: &mut Command, timeout: Duration) -> std::io::Result<Option<(Option<i32>, Option<String>)>> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    let Some(mut pipe) = child.stdout.take() else { return Ok(None) };
    // Drain on another thread so a full pipe cannot stall the child before the deadline. The
    // thread is never joined: a grandchild that inherited the pipe keeps it open after the
    // child is gone, and waiting for it would make the timeout useless.
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        let _ = tx.send(String::from_utf8_lossy(&bytes).into_owned());
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let out = rx.recv_timeout(Duration::from_millis(500)).ok();
    Ok(status.map(|s| (s.code(), out)))
}

/// `None` when `lsof` fails or times out: unknown is not the same as "nothing listens".
pub fn read_ports() -> Option<HashMap<u32, Vec<u16>>> {
    let mut cmd = Command::new("/usr/sbin/lsof");
    cmd.args(["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpn"]);
    let (code, out) = run_with_timeout(&mut cmd, LSOF_TIMEOUT)?;
    interpret_lsof(code, &out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsof_output_gives_ports_per_pid() {
        let out = "p44768\nf12\nn*:3001\np15904\nf9\nn127.0.0.1:3845\nf10\nn[::1]:5173\n";
        let p = parse_lsof(out);
        assert_eq!(p[&44768], vec![3001]);
        assert_eq!(p[&15904], vec![3845, 5173]);
    }

    #[test]
    fn a_port_listed_for_ipv4_and_ipv6_appears_once_and_garbage_is_ignored() {
        let p = parse_lsof("p7\nn*:8080\nn[::]:8080\nnlocalhost:http\nn*:3000\n");
        assert_eq!(p[&7], vec![3000, 8080]);
        assert!(parse_lsof("").is_empty());
    }

    #[test]
    fn lsof_exit_1_with_nothing_printed_means_no_listeners() {
        assert_eq!(interpret_lsof(Some(1), ""), Some(HashMap::new()));
        assert_eq!(interpret_lsof(Some(0), "p7\nn*:80\n").unwrap()[&7], vec![80]);
    }

    #[test]
    fn lsof_killed_by_a_signal_or_exiting_with_another_code_is_a_failure() {
        assert_eq!(interpret_lsof(None, "p7\nn*:80\n"), None);
        assert_eq!(interpret_lsof(Some(127), ""), None);
    }

    #[test]
    fn a_hung_command_is_killed_at_the_timeout_and_reported_as_a_failure() {
        let started = std::time::Instant::now();
        let out = run_with_timeout(Command::new("/bin/sleep").arg("30"), Duration::from_millis(300));
        assert_eq!(out, None);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_command_that_cannot_spawn_is_a_failure() {
        assert_eq!(run_with_timeout(&mut Command::new("/nonexistent/lsof"), Duration::from_secs(1)), None);
    }

    /// Prints this machine's ports: `cargo test print_real_ports -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn print_real_ports() {
        let mut ports: Vec<_> = read_ports().expect("lsof failed").into_iter().collect();
        ports.sort();
        for (pid, p) in ports {
            println!("{pid}: {p:?}");
        }
    }

    #[test]
    fn a_grandchild_holding_the_pipe_cannot_outlive_the_timeout() {
        let started = Instant::now();
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 4 & sleep 4"]);
        let result = run_with_timeout(&mut cmd, Duration::from_millis(200));
        assert_eq!(result, None);
        assert!(started.elapsed() < Duration::from_secs(3), "waited {:?}", started.elapsed());
    }

    #[test]
    fn output_that_did_not_arrive_is_unknown_not_empty() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo hi; sleep 3 & exit 0"]);
        assert_eq!(run_with_timeout(&mut cmd, Duration::from_secs(5)), None);
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo hi"]);
        assert_eq!(run_with_timeout(&mut cmd, Duration::from_secs(5)), Some((Some(0), "hi\n".to_string())));
    }
}
