//! Groups processes by tool and project, in the order of spec 5.2: Agent tools, app bundles,
//! other tools, the nearest tool ancestor (never an app), then the process's own name.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::kill::kill_block;
use super::raw::RawProcess;
use super::rules::{ToolKind, is_agent_session, match_tool, outer_app_bundle};

#[derive(Debug, Clone, Serialize)]
pub struct ProcessRow {
    pub pid: u32,
    pub start_time: u64,
    pub label: String,
    /// `None` when macOS would not tell us. Not the same as 0.
    pub footprint: Option<u64>,
    pub cpu_pct: f32,
    pub ports: Vec<u16>,
    pub killable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessGroup {
    pub id: String,
    pub kind: ToolKind,
    pub name: String,
    pub project: Option<String>,
    pub bundle: Option<PathBuf>,
    /// Sum of the known footprints; 0 when none could be read.
    pub footprint: u64,
    pub cpu_pct: f32,
    pub ports: Vec<u16>,
    pub processes: Vec<ProcessRow>,
    pub session_count: usize,
}

/// Name of the nearest folder up the tree that holds `.git` or `package.json`.
pub fn project_name(cwd: &Path) -> Option<String> {
    cwd.ancestors()
        .find(|d| d.join(".git").exists() || d.join("package.json").exists())
        .and_then(|d| d.file_name())
        .map(|n| n.to_string_lossy().into_owned())
}

struct Membership {
    kind: ToolKind,
    name: String,
    project: Option<String>,
    bundle: Option<PathBuf>,
}

fn nearest_tool_ancestor<'a>(
    p: &RawProcess,
    by_pid: &HashMap<u32, &'a RawProcess>,
) -> Option<(&'a RawProcess, ToolKind, &'static str)> {
    let mut seen = vec![p.pid];
    let mut parent = p.ppid;
    while let Some(a) = by_pid.get(&parent) {
        if seen.contains(&a.pid) {
            return None;
        }
        if let Some((kind, label)) = match_tool(a) {
            return Some((a, kind, label));
        }
        seen.push(a.pid);
        parent = a.ppid;
    }
    None
}

fn classify(
    p: &RawProcess,
    by_pid: &HashMap<u32, &RawProcess>,
    project_of: &dyn Fn(&Path) -> Option<String>,
) -> Membership {
    let project = |q: &RawProcess| q.cwd.as_deref().and_then(project_of);
    let tool = |q: &RawProcess, kind, label: &str| Membership {
        kind,
        name: label.to_string(),
        project: project(q),
        bundle: None,
    };
    let own_tool = match_tool(p);
    if let Some((ToolKind::Agent, label)) = own_tool {
        return tool(p, ToolKind::Agent, label);
    }
    if let Some(bundle) = p.exe.as_deref().and_then(outer_app_bundle) {
        let name = bundle.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        return Membership { kind: ToolKind::App, name, project: None, bundle: Some(bundle) };
    }
    if let Some((kind, label)) = own_tool {
        return tool(p, kind, label);
    }
    if let Some((ancestor, kind, label)) = nearest_tool_ancestor(p, by_pid) {
        return tool(ancestor, kind, label);
    }
    Membership { kind: ToolKind::Other, name: p.name.clone(), project: project(p), bundle: None }
}

pub fn group_processes(
    raw: &[RawProcess],
    ports: &HashMap<u32, Vec<u16>>,
    project_of: &dyn Fn(&Path) -> Option<String>,
    me_uid: u32,
    self_pid: u32,
) -> Vec<ProcessGroup> {
    let by_pid: HashMap<u32, &RawProcess> = raw.iter().map(|p| (p.pid, p)).collect();
    let mut groups: HashMap<String, ProcessGroup> = HashMap::new();
    // Hundreds of processes share a few folders, and each lookup walks up the file system.
    let projects: RefCell<HashMap<PathBuf, Option<String>>> = RefCell::default();
    let cached_project = |dir: &Path| projects.borrow_mut().entry(dir.to_path_buf()).or_insert_with(|| project_of(dir)).clone();

    for p in raw {
        let m = classify(p, &by_pid, &cached_project);
        let id = format!("{}|{}", m.name, m.project.as_deref().unwrap_or(""));
        let g = groups.entry(id.clone()).or_insert_with(|| ProcessGroup {
            id,
            kind: m.kind,
            name: m.name,
            project: m.project,
            bundle: m.bundle,
            footprint: 0,
            cpu_pct: 0.0,
            ports: vec![],
            processes: vec![],
            session_count: 0,
        });
        let row_ports = ports.get(&p.pid).cloned().unwrap_or_default();
        g.footprint += p.footprint.unwrap_or(0);
        g.cpu_pct += p.cpu_pct;
        g.session_count += usize::from(is_agent_session(p));
        g.ports.extend(&row_ports);
        g.processes.push(ProcessRow {
            pid: p.pid,
            start_time: p.start_time,
            label: p.name.clone(),
            footprint: p.footprint,
            cpu_pct: p.cpu_pct,
            ports: row_ports,
            killable: kill_block(p, me_uid, self_pid).is_none(),
        });
    }

    let mut out: Vec<ProcessGroup> = groups.into_values().collect();
    for g in &mut out {
        g.ports = g.ports.iter().copied().collect::<BTreeSet<_>>().into_iter().collect();
        g.processes.sort_by(|a, b| b.footprint.cmp(&a.footprint).then(a.pid.cmp(&b.pid)));
    }
    out.sort_by(|a, b| b.footprint.cmp(&a.footprint).then_with(|| a.id.cmp(&b.id)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::test_support::proc;

    fn last_folder(p: &Path) -> Option<String> {
        p.file_name().map(|n| n.to_string_lossy().into_owned())
    }

    fn groups(raw: &[RawProcess]) -> Vec<ProcessGroup> {
        group_processes(raw, &HashMap::new(), &last_folder, 501, 99999)
    }

    fn pids(g: &ProcessGroup) -> Vec<u32> {
        let mut v: Vec<u32> = g.processes.iter().map(|r| r.pid).collect();
        v.sort();
        v
    }

    #[test]
    fn vitest_workers_join_vitest_even_under_claude() {
        let raw = [
            proc(1, 0).name("claude").cmd(&["claude"]).cwd("/w/a"),
            proc(2, 1).name("node").cmd(&["node", "/w/a/node_modules/.bin/vitest"]).cwd("/w/a"),
            proc(3, 2).name("node").cmd(&["node", "worker.js"]).cwd("/w/a").mem(100),
        ];
        let g = groups(&raw);
        let v = g.iter().find(|g| g.name == "Vitest").unwrap();
        assert_eq!(pids(v), vec![2, 3]);
        assert_eq!(pids(g.iter().find(|g| g.name == "Claude Code").unwrap()), vec![1]);
    }

    #[test]
    fn descendants_take_the_ancestors_project_even_when_it_has_none() {
        let raw = [
            proc(1, 0).name("node").cmd(&["node", "/n/node_modules/.bin/vitest"]).cwd("/nowhere"),
            proc(2, 1).name("node").cmd(&["node", "worker.js"]).cwd("/w/proj"),
        ];
        let with_marker = |p: &Path| (p != Path::new("/nowhere")).then(|| last_folder(p)).flatten();
        let g = group_processes(&raw, &HashMap::new(), &with_marker, 501, 0);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].id, "Vitest|");
        assert_eq!(pids(&g[0]), vec![1, 2]);
    }

    #[test]
    fn same_tool_in_two_projects_is_two_groups() {
        let raw = [
            proc(1, 0).cmd(&["node", "/a/node_modules/.bin/vitest"]).cwd("/a"),
            proc(2, 0).cmd(&["node", "/b/node_modules/.bin/vitest"]).cwd("/b"),
        ];
        let g = groups(&raw);
        assert_eq!(g.len(), 2);
        assert!(g.iter().any(|g| g.id == "Vitest|a"));
    }

    #[test]
    fn children_of_a_terminal_app_do_not_join_the_app() {
        let raw = [
            proc(1, 0).exe("/Applications/Ghostty.app/Contents/MacOS/ghostty").name("ghostty"),
            proc(2, 1).name("zsh").cmd(&["zsh"]),
            proc(3, 2).name("node").cmd(&["node", "server.js"]),
        ];
        let g = groups(&raw);
        let app = g.iter().find(|g| g.name == "Ghostty").unwrap();
        assert_eq!(pids(app), vec![1]);
        assert_eq!(app.kind, ToolKind::App);
        assert_eq!(app.bundle, Some(PathBuf::from("/Applications/Ghostty.app")));
        assert!(g.iter().any(|g| g.name == "node"));
    }

    #[test]
    fn plain_processes_of_the_same_name_in_two_projects_are_two_groups() {
        let raw = [
            proc(1, 0).name("node").cmd(&["node", "server.js"]).cwd("/w/shop"),
            proc(2, 0).name("node").cmd(&["node", "server.js"]).cwd("/w/blog"),
            proc(3, 0).name("node").cmd(&["node", "x.js"]),
        ];
        let g = groups(&raw);
        assert_eq!(g.len(), 3, "{:?}", g.iter().map(|g| &g.id).collect::<Vec<_>>());
        let shop = g.iter().find(|g| g.id == "node|shop").unwrap();
        assert_eq!((shop.kind, shop.project.as_deref(), pids(shop)), (ToolKind::Other, Some("shop"), vec![1]));
        assert_eq!(pids(g.iter().find(|g| g.id == "node|").unwrap()), vec![3]);
    }

    #[test]
    fn a_folder_is_looked_up_once_per_tick_however_many_processes_sit_in_it() {
        use std::cell::Cell;
        let raw = [
            proc(1, 0).name("a").cwd("/w/shop"),
            proc(2, 0).name("b").cwd("/w/shop"),
            proc(3, 0).name("c").cwd("/w/shop"),
            proc(4, 0).name("d").cwd("/w/blog"),
        ];
        let looked_up = Cell::new(0);
        let counting = |p: &Path| {
            looked_up.set(looked_up.get() + 1);
            last_folder(p)
        };
        group_processes(&raw, &HashMap::new(), &counting, 501, 0);
        assert_eq!(looked_up.get(), 2);
    }

    #[test]
    fn claude_from_an_app_bundle_is_the_agent_not_an_app() {
        let raw = [proc(1, 0)
            .exe("/Users/x/.local/share/claude/ClaudeCode.app/Contents/MacOS/claude")
            .cmd(&["claude"])
            .cwd("/w/a")];
        let g = groups(&raw);
        assert_eq!((g[0].kind, g[0].name.as_str()), (ToolKind::Agent, "Claude Code"));
    }

    #[test]
    fn chrome_native_host_is_not_counted_as_a_session() {
        let raw = [
            proc(1, 0).name("claude").cmd(&["claude", "--resume", "x"]).cwd("/w/a"),
            proc(2, 0).name("claude").cmd(&["/u/.local/bin/claude", "--chrome-native-host"]).cwd("/w/a"),
        ];
        let g = groups(&raw);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].session_count, 1);
        assert_eq!(g[0].processes.len(), 2);
    }

    #[test]
    fn root_processes_are_not_killable_and_have_no_amount() {
        let raw = [proc(1, 0).uid(0).name("WindowServer").no_mem()];
        let g = groups(&raw);
        let r = &g[0].processes[0];
        assert!(!r.killable);
        assert_eq!(r.footprint, None);
        assert_eq!(g[0].footprint, 0);
    }

    #[test]
    fn own_user_processes_are_killable_except_system_paths_and_pantau_itself() {
        let raw = [
            proc(1, 0).name("a"),
            proc(2, 0).name("b").exe("/System/Library/x"),
            proc(99999, 0).name("pantau"),
        ];
        let g = groups(&raw);
        let killable = |name: &str| g.iter().find(|g| g.name == name).unwrap().processes[0].killable;
        assert!(killable("a"));
        assert!(!killable("b"));
        assert!(!killable("pantau"));
    }

    #[test]
    fn groups_sort_by_footprint_descending_and_unreadable_sorts_last() {
        let raw = [
            proc(1, 0).name("small").mem(10),
            proc(2, 0).name("unreadable").no_mem(),
            proc(3, 0).name("big").mem(300),
            proc(4, 0).name("big").mem(50),
            proc(5, 0).name("mid").mem(100),
        ];
        let g = groups(&raw);
        let names: Vec<&str> = g.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, vec!["big", "mid", "small", "unreadable"]);
        assert_eq!(g[0].footprint, 350);
    }

    #[test]
    fn ports_are_merged_sorted_and_deduplicated_per_group() {
        let raw = [
            proc(1, 0).cmd(&["node", "/a/node_modules/.bin/vite"]).cwd("/a"),
            proc(2, 1).name("node").cwd("/a"),
        ];
        let ports = HashMap::from([(1, vec![5173, 3000]), (2, vec![3000])]);
        let g = group_processes(&raw, &ports, &last_folder, 501, 0);
        assert_eq!(g[0].ports, vec![3000, 5173]);
    }

    #[test]
    fn project_name_walks_up_to_git_or_package_json() {
        let root = std::env::temp_dir().join(format!("pantau-project-test-{}", std::process::id()));
        let deep = root.join("app/src/x");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(root.join("app/.git")).unwrap();
        std::fs::create_dir_all(root.join("bare/y")).unwrap();
        assert_eq!(project_name(&deep), Some("app".to_string()));
        assert_eq!(project_name(&root.join("bare/y")), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Prints this machine's groups: `cargo test print_real_groups -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn print_real_groups() {
        let mut reader = crate::process::raw::ProcessReader::new();
        reader.read();
        std::thread::sleep(std::time::Duration::from_millis(500));
        let raw = reader.read();
        let me = unsafe { libc::getuid() };
        for g in group_processes(&raw, &HashMap::new(), &project_name, me, std::process::id()) {
            println!(
                "{:>9.1} MB  {:>5.1}%  {:?} {} [{}] x{} sessions={}",
                g.footprint as f64 / 1e6,
                g.cpu_pct,
                g.kind,
                g.name,
                g.project.as_deref().unwrap_or("-"),
                g.processes.len(),
                g.session_count
            );
        }
    }
}
