//! The registered locations. Pantau offers nothing outside this table.

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use super::{Category, Cleaner, ItemState, Paths, StorageItem, find_named_dirs};

const SCAN_DEPTH: usize = 5;

fn item(id: String, title: &str, sub: &str, path: PathBuf, category: Category, cleaner: Cleaner, upper_bound: bool) -> StorageItem {
    StorageItem {
        id,
        title: title.into(),
        sub: sub.into(),
        path,
        category,
        cleaner,
        upper_bound,
        bytes: None,
        measured_at_ms: None,
        state: ItemState::Pending,
    }
}

fn project_of(dir: &Path) -> String {
    dir.parent().map(|p| p.display().to_string()).unwrap_or_default()
}

impl Paths {
    /// The real locations. `tmpdir` comes from `confstr`, not `$TMPDIR`, because an app started
    /// from a shell can have that variable pointing at a user folder, which would then be treated
    /// as temp space. `scan_folders` are the saved settings, checked again here, and `~/Documents`
    /// until settings have been loaded.
    pub fn current() -> Paths {
        Paths::for_home(crate::settings::home())
    }

    /// `current()` for a given home, so tests never touch the real one.
    pub fn for_home(home: PathBuf) -> Paths {
        let scan_folders = crate::settings::active_scan_folders(&home).unwrap_or_else(|| crate::settings::default_scan_folders(&home));
        Paths { tmpdir: user_temp_dir(), private_tmp: "/private/tmp".into(), scan_folders, home }
    }
}

fn user_temp_dir() -> PathBuf {
    let mut buf = [0u8; 1024];
    let n = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buf.as_mut_ptr().cast(), buf.len()) };
    if n == 0 || n > buf.len() {
        return PathBuf::from("/private/tmp");
    }
    let bytes = &buf[..n - 1];
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes.strip_suffix(b"/").unwrap_or(bytes)))
}

/// `$TMPDIR` and `/private/tmp` are two items, `temp` and `temp-private`, so each root
/// is measured on its own and a denied one is not summed into a smaller number.
pub fn storage_items(paths: &Paths) -> Vec<StorageItem> {
    let support = paths.home.join("Library/Application Support");
    let mut items = Vec::new();

    for dir in find_named_dirs(&paths.scan_folders, ".next", SCAN_DEPTH) {
        let sub = project_of(&dir);
        items.push(item(format!("next-{}", dir.display()), "Cache build Next.js", &sub, dir, Category::Safe, Cleaner::RemoveDir, false));
    }
    items.push(item("pnpm".into(), "Cache pnpm", "Store pnpm", paths.home.join("Library/pnpm/store"), Category::Safe, Cleaner::PnpmPrune, true));
    items.push(item("temp".into(), "File sementara", "Folder sementara pengguna", paths.tmpdir.clone(), Category::Safe, Cleaner::OldTemp, true));
    if paths.private_tmp != paths.tmpdir {
        items.push(item("temp-private".into(), "File sementara", "/private/tmp", paths.private_tmp.clone(), Category::Safe, Cleaner::OldTemp, true));
    }
    items.push(item("claude-versions".into(), "Versi lama Claude Code", "Versi yang tidak aktif", paths.home.join(".local/share/claude/versions"), Category::Safe, Cleaner::OldClaudeVersions, true));

    items.push(item("cursor-db".into(), "Database Cursor", "Bisa berisi riwayat chat", support.join("Cursor/User/globalStorage/state.vscdb"), Category::Check, Cleaner::None, false));
    items.push(item("claude-vm".into(), "VM Claude Desktop", "Perlu diunduh ulang, dan mungkin sedang dipakai", support.join("Claude/vm_bundles"), Category::Check, Cleaner::None, false));
    items.push(item("claude-history".into(), "Riwayat sesi Claude Code", "Berisi riwayat sesi, tidak boleh ikut dibersihkan", paths.home.join(".claude/projects"), Category::Check, Cleaner::None, false));
    items.push(item("docker-install".into(), "Sisa instalasi Docker", "Instalasi belum selesai, statusnya belum jelas", support.join("com.docker.install"), Category::Check, Cleaner::None, false));

    for dir in find_named_dirs(&paths.scan_folders, "node_modules", SCAN_DEPTH) {
        let sub = project_of(&dir);
        items.push(item(format!("node-modules-{}", dir.display()), "node_modules", &sub, dir, Category::Check, Cleaner::None, false));
    }
    // An id names one row in the window, so it must be unique.
    let mut seen = std::collections::HashSet::new();
    items.retain(|i| seen.insert(i.id.clone()));
    items
}

#[cfg(test)]
mod tests {
    use super::super::{Category, Cleaner, ItemState};
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn paths(root: &std::path::Path) -> Paths {
        let projects = root.join("Documents");
        fs::create_dir_all(projects.join("web/.next")).unwrap();
        fs::create_dir_all(projects.join("web/node_modules/x")).unwrap();
        Paths {
            home: root.to_path_buf(),
            tmpdir: root.join("var-T"),
            private_tmp: root.join("private-tmp"),
            scan_folders: vec![projects],
        }
    }

    #[test]
    fn work_holding_data_is_check_only_and_has_no_cleaner() {
        let dir = tempfile::tempdir().unwrap();
        let items = storage_items(&paths(dir.path()));
        for id in ["cursor-db", "claude-vm", "claude-history", "docker-install"] {
            let it = items.iter().find(|i| i.id == id).unwrap();
            assert_eq!(it.category, Category::Check);
            assert_eq!(it.cleaner, Cleaner::None);
        }
    }

    #[test]
    fn safe_items_are_the_four_from_the_spec() {
        let dir = tempfile::tempdir().unwrap();
        let items = storage_items(&paths(dir.path()));
        for it in &items {
            let allowed = it.id.starts_with("next-") || it.id.starts_with("temp") || it.id == "pnpm" || it.id == "claude-versions";
            assert_eq!(it.cleaner != Cleaner::None, allowed, "{}", it.id);
            assert_eq!(it.category == Category::Safe, allowed, "{}", it.id);
        }
        let flagged: Vec<&str> = items.iter().filter(|i| i.upper_bound).map(|i| i.id.as_str()).collect();
        assert_eq!(flagged, ["pnpm", "temp", "temp-private", "claude-versions"]);
        let cleaner_of = |id: &str| items.iter().find(|i| i.id == id).unwrap().cleaner;
        assert_eq!(cleaner_of("pnpm"), Cleaner::PnpmPrune);
        assert_eq!(cleaner_of("temp"), Cleaner::OldTemp);
        assert_eq!(cleaner_of("temp-private"), Cleaner::OldTemp);
        assert_eq!(cleaner_of("claude-versions"), Cleaner::OldClaudeVersions);
    }

    #[test]
    fn locations_come_from_the_spec_table() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        let items = storage_items(&paths(h));
        let path_of = |id: &str| items.iter().find(|i| i.id == id).unwrap().path.clone();
        assert_eq!(path_of("pnpm"), h.join("Library/pnpm/store"));
        assert_eq!(path_of("claude-versions"), h.join(".local/share/claude/versions"));
        assert_eq!(path_of("claude-history"), h.join(".claude/projects"));
        assert_eq!(path_of("cursor-db"), h.join("Library/Application Support/Cursor/User/globalStorage/state.vscdb"));
        assert_eq!(path_of("claude-vm"), h.join("Library/Application Support/Claude/vm_bundles"));
        assert_eq!(path_of("docker-install"), h.join("Library/Application Support/com.docker.install"));
        assert_eq!(path_of("temp"), h.join("var-T"));
        assert_eq!(path_of("temp-private"), h.join("private-tmp"));
    }

    #[test]
    fn current_paths_use_the_darwin_temp_dir_not_the_environment() {
        let p = Paths::current();
        assert!(p.tmpdir.starts_with("/var/folders") || p.tmpdir.starts_with("/private/var/folders"), "{:?}", p.tmpdir);
        assert_eq!(p.private_tmp, PathBuf::from("/private/tmp"));
    }

    /// One test, because the live settings are process-wide and tests run in parallel. It uses a
    /// temporary home, so nothing is written to the real one.
    #[test]
    fn saved_scan_folders_apply_to_the_next_paths_without_a_restart() {
        use crate::settings::{Settings, set_active};
        let dir = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(dir.path()).unwrap().join("home");
        fs::create_dir_all(home.join("Documents")).unwrap();
        fs::create_dir_all(home.join("Work")).unwrap();
        // Before any settings were loaded: the default folder, if it passes validation.
        assert_eq!(Paths::for_home(home.clone()).scan_folders, vec![home.join("Documents")]);
        set_active(&Settings { scan_folders: vec![home.join("Work")], ..Settings::default() });
        assert_eq!(Paths::for_home(home.clone()).scan_folders, vec![home.join("Work")]);
        set_active(&Settings { scan_folders: vec![], ..Settings::default() });
        assert!(Paths::for_home(home.clone()).scan_folders.is_empty(), "an empty list means no project folders");
    }

    #[test]
    fn nested_scan_folders_list_each_cache_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = paths(dir.path());
        fs::create_dir_all(dir.path().join("Documents/web/sub/.next")).unwrap();
        p.scan_folders = vec![dir.path().join("Documents"), dir.path().join("Documents/web")];
        let items = storage_items(&p);
        let mut ids: Vec<_> = items.iter().map(|i| i.id.clone()).collect();
        let before = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), before, "every id is unique");
        assert_eq!(items.iter().filter(|i| i.id.starts_with("next-")).count(), 2);
    }

    #[test]
    fn items_start_pending_and_unmeasured() {
        let dir = tempfile::tempdir().unwrap();
        for it in storage_items(&paths(dir.path())) {
            assert_eq!((it.state, it.bytes, it.measured_at_ms), (ItemState::Pending, None, None));
        }
    }

    #[test]
    fn a_shared_temp_root_is_listed_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = paths(dir.path());
        p.tmpdir = p.private_tmp.clone();
        let items = storage_items(&p);
        assert_eq!(items.iter().filter(|i| i.id.starts_with("temp")).count(), 1);
    }

    #[test]
    fn next_caches_are_safe_per_project() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        let items = storage_items(&p);
        let next = format!("next-{}", dir.path().join("Documents/web/.next").display());
        let it = items.iter().find(|i| i.id == next).unwrap();
        assert_eq!((it.category, it.cleaner), (Category::Safe, Cleaner::RemoveDir));
    }

    #[test]
    fn node_modules_are_found_per_project_as_check_items() {
        let dir = tempfile::tempdir().unwrap();
        let items = storage_items(&paths(dir.path()));
        let id = format!("node-modules-{}", dir.path().join("Documents/web/node_modules").display());
        let it = items.iter().find(|i| i.id == id).unwrap();
        assert_eq!((it.category, it.cleaner), (Category::Check, Cleaner::None));
    }

    #[test]
    #[ignore = "reads the real home folder; run with --ignored --nocapture"]
    fn print_real_items() {
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let p = Paths {
            tmpdir: PathBuf::from(std::env::var("TMPDIR").unwrap()),
            private_tmp: "/private/tmp".into(),
            scan_folders: vec![home.join("Documents")],
            home,
        };
        for it in storage_items(&p) {
            println!("{:>12?} {:<14} {}", crate::storage::measure(&it.path), it.id, it.path.display());
        }
    }
}
