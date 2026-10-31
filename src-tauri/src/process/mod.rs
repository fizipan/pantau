//! Process data: raw reads from the OS, grouped by tool and project by later stages.

use serde::Serialize;

use self::group::ProcessGroup;
use self::leftover::Leftover;

pub mod group;
pub mod kill;
pub mod leftover;
pub mod ports;
pub mod raw;
pub mod rules;
#[cfg(test)]
pub mod test_support;

/// What the panel shows on its Proses tab.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub groups: Vec<ProcessGroup>,
    pub leftovers: Vec<Leftover>,
    /// This is the first read of a reader: memory is real, but CPU has nothing to compare with yet.
    pub cpu_pending: bool,
}
