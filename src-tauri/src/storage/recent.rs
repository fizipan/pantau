//! Large files that changed recently. Read-only; nothing here deletes.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const MIN_BYTES: u64 = 100 * 1024 * 1024;
const WINDOW_SECS: u64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentFile {
    pub path: PathBuf,
    /// Allocated bytes (`st_blocks * 512`), so a sparse file does not look large.
    pub bytes: u64,
    pub changed_at_secs: u64,
    /// A guess from the folder name only. macOS does not record which app made a file.
    pub owner_guess: Option<String>,
}

/// Spec 6.4 excludes, relative to home.
pub fn default_excludes(home: &Path) -> Vec<PathBuf> {
    ["Library/Containers", "Library/Group Containers", "Library/Mobile Documents", "Library/CloudStorage", ".Trash"]
        .iter()
        .map(|p| home.join(p))
        .collect()
}

/// What a scan found, and how much it could not see. An empty `files` with `root_denied` or
/// `skipped_dirs` is "could not look", not "nothing there".
#[derive(Debug, Default)]
pub struct RecentScan {
    pub files: Vec<RecentFile>,
    /// Folders below the root that could not be read (permission or macOS privacy protection).
    pub skipped_dirs: usize,
    pub root_denied: bool,
    /// False when `should_continue` stopped the walk early; `files` is then partial.
    pub complete: bool,
}

/// `should_continue` is polled before each folder, so the caller can stop a long walk.
pub fn scan_recent(root: &Path, excludes: &[PathBuf], now: SystemTime, should_continue: &dyn Fn() -> bool) -> RecentScan {
    scan_with_min(root, excludes, now, MIN_BYTES, should_continue)
}

/// Walks `root` without following symlinks. Skips `excludes`, `node_modules` and `.git`.
/// Sizes are allocated bytes (not logical length) on purpose: the question is what fills the disk.
/// `min_bytes` is a parameter only so tests can stay small.
fn scan_with_min(root: &Path, excludes: &[PathBuf], now: SystemTime, min_bytes: u64, should_continue: &dyn Fn() -> bool) -> RecentScan {
    let now_secs = now.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut scan = RecentScan { complete: true, ..RecentScan::default() };
    let found = &mut scan.files;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if !should_continue() {
            scan.complete = false;
            scan.files.clear();
            return scan;
        }
        let Ok(rd) = fs::read_dir(&dir) else {
            if dir == root { scan.root_denied = true } else { scan.skipped_dirs += 1 }
            continue;
        };
        for entry in rd.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            let path = entry.path();
            if meta.is_dir() {
                let name = entry.file_name();
                if name == "node_modules" || name == ".git" || excludes.iter().any(|x| path.starts_with(x)) {
                    continue;
                }
                stack.push(path);
            } else if meta.is_file() {
                let bytes = meta.blocks() * 512;
                let changed = meta.mtime().max(0) as u64;
                if bytes >= min_bytes && now_secs.saturating_sub(changed) <= WINDOW_SECS && !excludes.iter().any(|x| path.starts_with(x)) {
                    found.push(RecentFile { owner_guess: guess_owner(&path), path, bytes, changed_at_secs: changed });
                }
            }
        }
    }
    found.sort_by(|a, b| b.changed_at_secs.cmp(&a.changed_at_secs).then_with(|| a.path.cmp(&b.path)));
    scan
}

/// Folder fragment (lowercase) to name. First match wins, so the longer fragments come first.
const OWNERS: &[(&str, &str)] = &[
    ("application support/claude", "Claude Desktop"),
    ("application support/cursor", "Cursor"),
    ("/.cursor/", "Cursor"),
    (".local/share/claude", "Claude Code"),
    ("/.claude/", "Claude Code"),
    ("/.codex/", "Codex"),
    ("/.gemini/", "Gemini"),
    ("pnpm", "pnpm"),
    ("/.next/", "Next.js"),
    ("docker", "Docker"),
    ("googleupdater", "Google Updater"),
];

/// A guess from the folder only; the UI must label it as one.
pub fn guess_owner(path: &Path) -> Option<String> {
    let p = format!("{}/", path.to_string_lossy().to_lowercase());
    OWNERS.iter().find(|(frag, _)| p.contains(frag)).map(|(_, name)| (*name).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
    use std::time::Duration;

    const DAY: u64 = 24 * 60 * 60;

    fn real_file(path: &Path, bytes: usize, age_days: u64, now: SystemTime) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = File::create(path).unwrap();
        f.write_all(&vec![1u8; bytes]).unwrap();
        f.set_modified(now - Duration::from_secs(age_days * DAY)).unwrap();
    }

    #[test]
    fn only_big_recent_files_outside_excludes_are_listed_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let now = SystemTime::now();
        // Threshold is a parameter here: 200 KB stands in for 100 MB.
        real_file(&r.join("a/old-enough.bin"), 300_000, 3, now);
        real_file(&r.join("a/newest.bin"), 300_000, 1, now);
        real_file(&r.join("a/eight-days.bin"), 300_000, 8, now);
        real_file(&r.join("a/small.bin"), 1_000, 0, now);
        real_file(&r.join("Library/Containers/app/big.bin"), 300_000, 0, now);
        real_file(&r.join("proj/node_modules/x/big.bin"), 300_000, 0, now);
        real_file(&r.join("proj/.git/objects/big.pack"), 300_000, 0, now);
        let excludes = vec![r.join("Library/Containers")];

        let found = scan_with_min(r, &excludes, now, 200_000, &|| true).files;
        let names: Vec<_> = found.iter().map(|f| f.path.file_name().unwrap().to_str().unwrap()).collect();
        assert_eq!(names, ["newest.bin", "old-enough.bin"]);
        assert!(found[0].changed_at_secs > found[1].changed_at_secs);
        assert!(found[0].bytes >= 300_000);
    }

    #[test]
    fn sparse_files_count_by_allocated_bytes_and_symlinks_are_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let now = SystemTime::now();
        // 50 MB logical, almost nothing allocated.
        let sparse = File::create(r.join("sparse.img")).unwrap();
        sparse.set_len(50_000_000).unwrap();
        sparse.set_modified(now).unwrap();
        real_file(&r.join("real/big.bin"), 300_000, 0, now);
        std::os::unix::fs::symlink(r.join("real"), r.join("link-dir")).unwrap();
        std::os::unix::fs::symlink(r.join("real/big.bin"), r.join("link-file")).unwrap();

        let found = scan_with_min(r, &[], now, 200_000, &|| true).files;
        let paths: Vec<_> = found.iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, [r.join("real/big.bin")]);
    }

    #[test]
    fn owner_is_guessed_from_the_folder() {
        assert_eq!(guess_owner(Path::new("/u/.local/share/claude/versions/2.1.316")), Some("Claude Code".into()));
        assert_eq!(guess_owner(Path::new("/u/Library/Application Support/Cursor/x.bin")), Some("Cursor".into()));
        assert_eq!(guess_owner(Path::new("/u/Library/Application Support/Claude/vm_bundles/a")), Some("Claude Desktop".into()));
        assert_eq!(guess_owner(Path::new("/u/.codex/x")), Some("Codex".into()));
        assert_eq!(guess_owner(Path::new("/u/.gemini/x")), Some("Gemini".into()));
        assert_eq!(guess_owner(Path::new("/u/Library/pnpm/store/v3/x")), Some("pnpm".into()));
        assert_eq!(guess_owner(Path::new("/u/web/.next/cache/x")), Some("Next.js".into()));
        assert_eq!(guess_owner(Path::new("/u/Library/Application Support/com.docker.install/x")), Some("Docker".into()));
        assert_eq!(guess_owner(Path::new("/u/Library/Caches/com.google.GoogleUpdater/x")), Some("Google Updater".into()));
        assert_eq!(guess_owner(Path::new("/u/film.mov")), None);
    }

    #[test]
    fn unreadable_folders_are_counted_and_a_denied_root_is_flagged() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let now = SystemTime::now();
        real_file(&r.join("ok/big.bin"), 300_000, 0, now);
        fs::create_dir_all(r.join("locked")).unwrap();
        fs::set_permissions(r.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
        let scan = scan_with_min(r, &[], now, 200_000, &|| true);
        assert_eq!((scan.files.len(), scan.skipped_dirs, scan.root_denied, scan.complete), (1, 1, false, true));
        fs::set_permissions(r.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();

        fs::set_permissions(r, fs::Permissions::from_mode(0o000)).unwrap();
        let scan = scan_with_min(r, &[], now, 200_000, &|| true);
        fs::set_permissions(r, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!((scan.files.len(), scan.root_denied), (0, true));
    }

    #[test]
    fn a_scan_stops_when_asked_and_says_it_is_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        real_file(&dir.path().join("a/big.bin"), 300_000, 0, now);
        let scan = scan_with_min(dir.path(), &[], now, 200_000, &|| false);
        assert!(!scan.complete);
        assert!(scan.files.is_empty());
    }
}
