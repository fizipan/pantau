//! Leftover candidates: orphaned test runners and dev tools that nothing is listening on.
//! These are only candidates; the owner confirms before anything is killed.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Serialize;

use super::raw::RawProcess;
use super::rules::{ToolKind, match_tool, outer_app_bundle};

#[derive(Debug, Clone, Serialize)]
pub struct Leftover {
    pub pid: u32,
    pub start_time: u64,
    pub command: String,
    pub project: Option<String>,
    pub age_secs: u64,
    pub kind: ToolKind,
    /// Test runners start ticked; a dev tool might be a watcher still in use.
    pub preselected: bool,
    pub footprint: Option<u64>,
}

/// True when `pid` or any process below it listens on a port.
fn tree_listens(pid: u32, children: &HashMap<u32, Vec<u32>>, ports: &HashMap<u32, Vec<u16>>) -> bool {
    let mut seen = HashSet::new();
    let mut stack = vec![pid];
    while let Some(p) = stack.pop() {
        if !seen.insert(p) {
            continue;
        }
        if ports.get(&p).is_some_and(|v| !v.is_empty()) {
            return true;
        }
        stack.extend(children.get(&p).into_iter().flatten().copied());
    }
    false
}

pub fn find_leftovers(
    raw: &[RawProcess],
    ports: &HashMap<u32, Vec<u16>>,
    now_secs: u64,
    me_uid: u32,
    project_of: &dyn Fn(&Path) -> Option<String>,
) -> Vec<Leftover> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for p in raw {
        children.entry(p.ppid).or_default().push(p.pid);
    }
    raw.iter()
        .filter(|p| p.ppid == 1 && p.uid == me_uid)
        .filter(|p| !p.exe.as_deref().and_then(outer_app_bundle).is_some())
        .filter_map(|p| {
            let (kind, _) = match_tool(p).filter(|(k, _)| matches!(k, ToolKind::Test | ToolKind::Dev))?;
            (!tree_listens(p.pid, &children, ports)).then(|| Leftover {
                pid: p.pid,
                start_time: p.start_time,
                command: p.cmd.join(" "),
                project: p.cwd.as_deref().and_then(project_of),
                age_secs: now_secs.saturating_sub(p.start_time),
                kind,
                preselected: kind == ToolKind::Test,
                footprint: p.footprint,
            })
        })
        .collect()
}

/// Without a port reading, absence of data is not "nothing listens", so nothing is a candidate.
pub fn leftovers_if_ports_known(
    raw: &[RawProcess],
    ports: Option<&HashMap<u32, Vec<u16>>>,
    now_secs: u64,
    me_uid: u32,
    project_of: &dyn Fn(&Path) -> Option<String>,
) -> Vec<Leftover> {
    ports.map_or_else(Vec::new, |p| find_leftovers(raw, p, now_secs, me_uid, project_of))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::test_support::proc;

    const NOW: u64 = 10_000;
    const ME: u32 = 501;

    fn name_of(p: &Path) -> Option<String> {
        p.file_name().map(|n| n.to_string_lossy().into_owned())
    }

    #[test]
    fn unknown_ports_mean_no_leftovers_rather_than_nothing_listens() {
        let raw = [proc(9, 1).cmd(&["node", "/a/node_modules/.bin/vitest"]).cwd("/a")];
        assert!(leftovers_if_ports_known(&raw, None, NOW, ME, &name_of).is_empty());
        assert_eq!(leftovers_if_ports_known(&raw, Some(&HashMap::new()), NOW, ME, &name_of).len(), 1);
    }

    #[test]
    fn an_orphan_test_runner_is_a_preselected_candidate() {
        let raw = [proc(9, 1).cmd(&["node", "/a/node_modules/.bin/vitest"]).cwd("/a")];
        let l = find_leftovers(&raw, &HashMap::new(), NOW, ME, &name_of);
        assert_eq!(l.len(), 1);
        assert!(l[0].preselected);
        assert_eq!(l[0].project.as_deref(), Some("a"));
        assert_eq!(l[0].age_secs, NOW);
        assert_eq!(l[0].command, "node /a/node_modules/.bin/vitest");
    }

    #[test]
    fn an_orphan_dev_tool_is_a_candidate_but_not_preselected() {
        let raw = [proc(9, 1).cmd(&["node", "/a/node_modules/.bin/vite"]).cwd("/a")];
        let l = find_leftovers(&raw, &HashMap::new(), NOW, ME, &name_of);
        assert_eq!(l.len(), 1);
        assert!(!l[0].preselected);
    }

    #[test]
    fn another_users_or_roots_processes_are_never_leftovers() {
        // Since root processes keep their real parent, `launchd`'s children include other accounts' tools.
        let raw = [
            proc(8, 1).uid(0).cmd(&["node", "/a/node_modules/.bin/vitest"]),
            proc(9, 1).uid(502).cmd(&["node", "/a/node_modules/.bin/vitest"]),
            proc(10, 1).cmd(&["node", "/a/node_modules/.bin/vitest"]),
        ];
        let l = find_leftovers(&raw, &HashMap::new(), NOW, ME, &name_of);
        assert_eq!(l.iter().map(|l| l.pid).collect::<Vec<_>>(), [10]);
    }

    #[test]
    fn a_tool_whose_parent_is_alive_is_not_a_leftover() {
        let raw = [proc(2, 100).cmd(&["node", "/a/node_modules/.bin/vitest"])];
        assert!(find_leftovers(&raw, &HashMap::new(), NOW, ME, &name_of).is_empty());
    }

    #[test]
    fn a_process_that_listens_is_not_a_leftover() {
        let raw = [proc(9, 1).cmd(&["node", "/a/node_modules/.bin/vite"])];
        let ports = HashMap::from([(9, vec![5173])]);
        assert!(find_leftovers(&raw, &ports, NOW, ME, &name_of).is_empty());
    }

    #[test]
    fn a_process_whose_child_listens_is_not_a_leftover() {
        // Real case: `bun run api:dev` listens on nothing, its child listens on :3001.
        let raw = [proc(44766, 1).name("bun").cmd(&["bun", "run", "api:dev"]), proc(44768, 44766).name("bun")];
        let ports = HashMap::from([(44768, vec![3001])]);
        assert!(find_leftovers(&raw, &ports, NOW, ME, &name_of).is_empty());
    }

    #[test]
    fn a_tool_inside_an_app_bundle_is_never_a_leftover() {
        let raw = [proc(1, 1)
            .exe("/Applications/Foo.app/Contents/MacOS/foo")
            .cmd(&["node", "/a/node_modules/.bin/vite"])];
        assert!(find_leftovers(&raw, &HashMap::new(), NOW, ME, &name_of).is_empty());
    }

    #[test]
    fn agents_apps_and_unknown_tools_are_never_leftovers() {
        let raw = [
            proc(1, 1).name("claude").cmd(&["claude"]),
            proc(2, 1).exe("/Applications/Buku Token.app/Contents/MacOS/buku-token"),
            proc(3, 1).name("node").cmd(&["node", "server.js"]),
        ];
        assert!(find_leftovers(&raw, &HashMap::new(), NOW, ME, &name_of).is_empty());
    }
}
