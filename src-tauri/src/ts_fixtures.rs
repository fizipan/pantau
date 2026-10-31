//! Sample DTOs as JSON for the TypeScript contract test (`src/test/fixtures/*.json`). Every path
//! here is made up; nothing in a committed fixture may come from the machine that ran the test.
//! Regenerate: `TS_FIXTURE_DIR=../src/test/fixtures cargo test write_ts_fixtures -- --ignored`.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::commands::{OpState, Operation, PanelData, panel_data};
use crate::process::Snapshot;
use crate::process::kill::ProcId;
use crate::process::group::{ProcessGroup, ProcessRow};
use crate::process::leftover::Leftover;
use crate::process::rules::ToolKind;
use crate::storage::recent::RecentFile;
use crate::storage::scheduler::Snapshot as StorageData;
use crate::storage::{Category, Cleaner, ItemState, StorageItem};
use crate::system::{Pressure, SystemSample};

const GB: u64 = 1024 * 1024 * 1024;
const MB: u64 = 1024 * 1024;

fn sample(i: u64, cpu: f32) -> SystemSample {
    SystemSample {
        at_ms: 1_793_000_000_000 + i * 2000,
        cpu_pct: cpu,
        core_count: 8,
        ram_used: 11 * GB,
        ram_total: 16 * GB,
        swap_used: 2 * GB,
        swap_total: 4 * GB,
        pressure: Pressure::Warning,
        disk_used: 335 * GB,
        disk_total: 460 * GB,
        disk_free: 125 * GB,
    }
}

fn row(pid: u32, label: &str, footprint: Option<u64>, ports: Vec<u16>, killable: bool) -> ProcessRow {
    ProcessRow { pid, start_time: 1_793_000_000 + u64::from(pid), label: label.into(), footprint, cpu_pct: 3.5, ports, killable }
}

fn panel() -> PanelData {
    let snapshot = Snapshot {
        cpu_pending: false,
        groups: vec![
            ProcessGroup {
                id: "agent:Claude Code:made-up-app".into(),
                kind: ToolKind::Agent,
                name: "Claude Code".into(),
                project: Some("made-up-app".into()),
                bundle: None,
                footprint: 900 * MB,
                cpu_pct: 12.0,
                ports: vec![3000],
                processes: vec![row(4101, "claude", Some(600 * MB), vec![], true), row(4102, "node vite", Some(300 * MB), vec![3000], true)],
                session_count: 1,
            },
            ProcessGroup {
                id: "app:Made Up".into(),
                kind: ToolKind::App,
                name: "Made Up".into(),
                project: None,
                bundle: Some(PathBuf::from("/Applications/Made Up.app")),
                footprint: 0,
                cpu_pct: 0.0,
                ports: vec![],
                processes: vec![row(4201, "Made Up Helper", None, vec![], false)],
                session_count: 0,
            },
        ],
        leftovers: vec![Leftover {
            pid: 4301,
            start_time: 1_793_000_301,
            command: "vitest run".into(),
            project: Some("made-up-app".into()),
            age_secs: 5400,
            kind: ToolKind::Test,
            preselected: true,
            footprint: Some(210 * MB),
        }],
    };
    let operations = vec![
        Operation { op_id: "op-running".into(), state: OpState::Running, survivors: vec![] },
        Operation { op_id: "op-cleaned".into(), state: OpState::Done { freed_bytes: Some(2 * GB), skipped: 1 }, survivors: vec![] },
        Operation { op_id: "op-killed".into(), state: OpState::Done { freed_bytes: None, skipped: 0 }, survivors: vec![] },
        Operation { op_id: "op-stuck".into(), state: OpState::StillAlive, survivors: vec![ProcId { pid: 4401, start_time: 1_793_000_401 }] },
        Operation { op_id: "op-failed".into(), state: OpState::Failed("1 dari 2 gagal: PID 9: izin ditolak. Sisanya sudah berhenti.".into()), survivors: vec![] },
    ];
    panel_data(Some(snapshot), vec![sample(0, 20.0), sample(1, 35.5)], operations)
}

fn item(id: &str, path: &str, category: Category, cleaner: Cleaner, bytes: Option<u64>, state: ItemState) -> StorageItem {
    StorageItem {
        id: id.into(),
        title: format!("Item {id}"),
        sub: "Contoh".into(),
        path: PathBuf::from(path),
        category,
        cleaner,
        upper_bound: cleaner == Cleaner::PnpmPrune,
        bytes,
        measured_at_ms: bytes.map(|_| 1_793_000_000_000),
        state,
    }
}

fn storage() -> StorageData {
    StorageData {
        items: vec![
            item("next-1", "/Users/example/Documents/made-up-app/.next", Category::Safe, Cleaner::RemoveDir, Some(1_181_116_006), ItemState::Ready),
            item("pnpm", "/Users/example/Library/pnpm/store", Category::Safe, Cleaner::PnpmPrune, Some(4 * GB), ItemState::Ready),
            item("claude-history", "/Users/example/.claude/projects", Category::Check, Cleaner::None, None, ItemState::Denied),
            item("temp", "/var/folders/xx/made-up/T", Category::Safe, Cleaner::OldTemp, None, ItemState::Pending),
            item("next-2", "/Users/example/Documents/busy-app/.next", Category::Safe, Cleaner::RemoveDir, Some(300 * MB), ItemState::Refused("Masih ada proses yang berjalan di project ini".into())),
        ],
        recent: vec![RecentFile {
            path: PathBuf::from("/Users/example/Downloads/made-up-video.mov"),
            bytes: 3 * GB,
            changed_at_secs: 1_792_900_000,
            owner_guess: Some("Downloads".into()),
        }],
        recent_at_ms: Some(1_793_000_000_000),
        recent_skipped_dirs: 2,
        recent_denied: false,
        denied_roots: vec![PathBuf::from("/Users/example/Documents")],
        paused: false,
        measuring: true,
        scanning: false,
        measuring_auto: true,
        scanning_auto: false,
    }
}

fn dump<T: Serialize>(dir: &Path, name: &str, v: &T) {
    std::fs::write(dir.join(format!("{name}.json")), serde_json::to_string_pretty(v).unwrap() + "\n").unwrap();
}

#[test]
#[ignore = "manual: writes JSON fixtures for the TypeScript contract test"]
fn write_ts_fixtures() {
    let dir = PathBuf::from(std::env::var("TS_FIXTURE_DIR").expect("set TS_FIXTURE_DIR"));
    std::fs::create_dir_all(&dir).unwrap();
    dump(&dir, "panel-data", &panel());
    dump(&dir, "storage-data", &storage());
}

fn committed(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/test/fixtures").join(format!("{name}.json")))
        .unwrap_or_else(|e| panic!("{name}.json is missing ({e}); regenerate the fixtures"))
}

#[test]
fn committed_fixtures_match_what_the_structs_serialize_to() {
    for (name, fresh) in [("panel-data", serde_json::to_value(panel())), ("storage-data", serde_json::to_value(storage()))] {
        let on_disk: serde_json::Value = serde_json::from_str(&committed(name)).unwrap();
        assert_eq!(on_disk, fresh.unwrap(), "{name}.json is out of date; regenerate the fixtures");
    }
}

#[test]
fn fixtures_never_contain_a_real_home_path() {
    let home = std::env::var("HOME").unwrap_or_default();
    for name in ["panel-data", "storage-data"] {
        let text = committed(name);
        assert!(home.len() < 2 || !text.contains(&home), "{name}.json contains the real home path");
    }
}
