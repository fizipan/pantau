//! Builder for `RawProcess` values in tests, shared by the process modules.

use std::path::PathBuf;

use super::raw::RawProcess;

/// A user process (uid 501) with a 1 MB footprint and no command line.
pub fn proc(pid: u32, ppid: u32) -> RawProcess {
    RawProcess {
        pid,
        ppid,
        uid: 501,
        start_time: 0,
        name: "proc".into(),
        exe: None,
        cmd: vec![],
        cwd: None,
        footprint: Some(1_000_000),
        cpu_pct: 0.0,
    }
}

impl RawProcess {
    pub fn name(mut self, v: &str) -> Self {
        self.name = v.into();
        self
    }
    pub fn exe(mut self, v: &str) -> Self {
        self.exe = Some(PathBuf::from(v));
        self
    }
    pub fn cmd(mut self, v: &[&str]) -> Self {
        self.cmd = v.iter().map(|s| s.to_string()).collect();
        self
    }
    pub fn cwd(mut self, v: &str) -> Self {
        self.cwd = Some(PathBuf::from(v));
        self
    }
    pub fn uid(mut self, v: u32) -> Self {
        self.uid = v;
        self
    }
    pub fn mem(mut self, v: u64) -> Self {
        self.footprint = Some(v);
        self
    }
    pub fn no_mem(mut self) -> Self {
        self.footprint = None;
        self
    }
}
