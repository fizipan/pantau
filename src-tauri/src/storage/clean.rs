//! The only code in Pantau that deletes. Every path is checked again right before removal,
//! and nothing here follows a symlink out of the folder it was given.

use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use super::{Cleaner, Paths, StorageItem};
use crate::errors::os_error;
use crate::process::ports::{run_with_timeout, spawn_with_timeout};
use crate::process::raw::RawProcess;

const TEMP_MIN_AGE: Duration = Duration::from_secs(3 * 24 * 60 * 60);
const PNPM_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const LSOF_TIMEOUT: Duration = Duration::from_secs(15);

/// Everything `clean` reads from the machine, so tests can point it at a temp directory.
pub struct CleanContext {
    pub paths: Paths,
    /// `None` when the `lsof` read failed: temp and Claude version cleanup then refuse instead of
    /// assuming that nothing is open.
    pub open_paths: Option<HashSet<PathBuf>>,
    /// The clock `clean` judges age by; tests move it instead of faking file times.
    pub now: SystemTime,
    pub me_uid: u32,
    pub disk_free: fn() -> u64,
    pub run_pnpm: Box<dyn Fn() -> Result<(), String> + Send + Sync>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanResult {
    pub freed_bytes: u64,
    /// Entries that were offered but not removed, so a partial cleanup is visible.
    pub skipped: usize,
}

/// Why this item must not be cleaned right now, if there is a reason.
pub fn refusal(item: &StorageItem, processes: &[RawProcess]) -> Option<String> {
    if item.cleaner != Cleaner::RemoveDir {
        return None;
    }
    let project = item.path.parent()?;
    // The kernel reports real working directories, so a symlinked project is also matched by
    // its real path.
    let real = fs::canonicalize(project).ok();
    processes
        .iter()
        .any(|p| p.cwd.as_deref().is_some_and(|cwd| cwd.starts_with(project) || real.as_deref().is_some_and(|r| cwd.starts_with(r))))
        .then(|| "Masih ada proses yang berjalan di project ini".to_string())
}

fn parse_version(name: &str) -> Option<Vec<u64>> {
    name.split('.').map(|part| part.parse().ok()).collect()
}

/// Version entries of `dir` other than the active one and the newest one, oldest first.
/// Entries whose name is not a version number are never offered.
pub fn old_versions(dir: &Path, active: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else { return vec![] };
    let mut versions: Vec<(Vec<u64>, PathBuf)> = rd
        .flatten()
        .filter_map(|e| Some((parse_version(e.file_name().to_str()?)?, e.path())))
        .collect();
    let newest = versions.iter().map(|(v, _)| v.clone()).max();
    versions.retain(|(v, p)| Some(v) != newest.as_ref() && p.file_name() != active.file_name());
    versions.sort();
    versions.into_iter().map(|(_, p)| p).collect()
}

/// A live process runs from this version or has it open. `~/.local/bin/claude` only says what
/// new sessions start from; older sessions keep running from the file they started with.
fn version_in_use(version: &Path, open_paths: &HashSet<PathBuf>) -> bool {
    // lsof prints real paths.
    let real = fs::canonicalize(version).unwrap_or_else(|_| version.to_path_buf());
    open_paths.iter().any(|o| o.starts_with(version) || o.starts_with(&real))
}

struct Scan {
    newest: SystemTime,
    bytes: u64,
    has_special: bool,
}

fn at(secs: i64, nanos: i64) -> SystemTime {
    match u64::try_from(secs) {
        Ok(s) => SystemTime::UNIX_EPOCH + Duration::new(s, nanos.clamp(0, 999_999_999) as u32),
        Err(_) => SystemTime::UNIX_EPOCH,
    }
}

/// Newest change (mtime or ctime, since `tar x` and `rsync -a` keep an old mtime), total file
/// bytes, and whether a socket or FIFO exists, over `path` and everything under it. Symlinks
/// are not followed. `None` when anything cannot be read or lies on another device than `dev`:
/// unknown is not "old", and a mounted image is not ours to clean.
fn scan_entry(path: &Path, dev: u64) -> Option<Scan> {
    let mut scan = Scan { newest: SystemTime::UNIX_EPOCH, bytes: 0, has_special: false };
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        let md = fs::symlink_metadata(&p).ok()?;
        if md.dev() != dev {
            return None;
        }
        scan.newest = scan.newest.max(md.modified().ok()?).max(at(md.ctime(), md.ctime_nsec()));
        let ft = md.file_type();
        scan.has_special |= ft.is_socket() || ft.is_fifo();
        if ft.is_file() {
            scan.bytes += md.len();
        }
        if ft.is_dir() {
            for e in fs::read_dir(&p).ok()? {
                stack.push(e.ok()?.path());
            }
        }
    }
    Some(scan)
}

/// Top-level entries of `root` that are mine, idle over 3 days, not open, and not something a
/// live program may own: `com.apple.*`, Claude's own scratch, empty entries, sockets and FIFOs.
pub fn temp_candidates(root: &Path, now: SystemTime, open_paths: &HashSet<PathBuf>, me_uid: u32) -> Vec<PathBuf> {
    let Ok(root_md) = fs::metadata(root) else { return vec![] };
    temp_candidates_on(root, root_md.dev(), now, open_paths, me_uid)
}

/// `root_dev` is the device of the root folder itself: an entry on any other device is a
/// mounted volume, even when the mount point is the entry.
fn temp_candidates_on(root: &Path, root_dev: u64, now: SystemTime, open_paths: &HashSet<PathBuf>, me_uid: u32) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(root) else { return vec![] };
    // lsof prints real paths, while `$TMPDIR` goes through the `/var` symlink.
    let real_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let scratch = format!("claude-{me_uid}");
    let mut found: Vec<PathBuf> = rd
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.starts_with("com.apple.") || name == scratch {
                return None;
            }
            let md = fs::symlink_metadata(&path).ok()?;
            if md.uid() != me_uid {
                return None;
            }
            let scan = scan_entry(&path, root_dev)?;
            if scan.bytes == 0 || scan.has_special {
                return None;
            }
            let idle = now.duration_since(scan.newest).ok()?;
            let real = real_root.join(e.file_name());
            let in_use = open_paths.iter().any(|o| o.starts_with(&path) || o.starts_with(&real));
            (idle > TEMP_MIN_AGE && !in_use).then_some(path)
        })
        .collect();
    found.sort();
    found
}

/// Removes `path` without following symlinks, and only when it sits inside `allowed_root`
/// (judged by the real parent folder, so a symlinked parent cannot lead out).
pub fn remove_entry(path: &Path, allowed_root: &Path) -> Result<(), String> {
    let name = path.file_name().ok_or("Lokasi tidak valid")?;
    let parent = path.parent().ok_or("Lokasi tidak valid")?;
    let root = fs::canonicalize(allowed_root).map_err(|e| os_error("Folder tujuan tidak bisa dibaca", &e))?;
    let parent = fs::canonicalize(parent).map_err(|e| os_error("Lokasi tidak bisa dibaca", &e))?;
    if !parent.starts_with(&root) {
        return Err("Lokasi ini di luar folder yang boleh dibersihkan".into());
    }
    let target = parent.join(name);
    let md = fs::symlink_metadata(&target).map_err(|e| os_error("Lokasi tidak bisa dibaca", &e))?;
    let result = if md.file_type().is_dir() { fs::remove_dir_all(&target) } else { fs::remove_file(&target) };
    result.map_err(|e| os_error(&format!("Gagal menghapus {}", name.to_string_lossy()), &e))
}

/// Canonical form of `path` even when its tail does not exist: the longest existing prefix is
/// resolved through symlinks, the rest is normalised lexically.
fn resolve(path: &Path) -> PathBuf {
    let mut base = path.to_path_buf();
    let mut tail = Vec::new();
    loop {
        if let Ok(real) = fs::canonicalize(&base) {
            return tail.iter().rev().fold(real, |acc, c: &std::ffi::OsString| acc.join(c));
        }
        match (base.file_name().map(|n| n.to_owned()), base.parent().map(Path::to_path_buf)) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                base = parent;
            }
            _ => return lexical(path),
        }
    }
}

fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn named(name: Option<&OsStr>, want: &str) -> bool {
    name.and_then(OsStr::to_str).is_some_and(|n| n.eq_ignore_ascii_case(want))
}

fn is_history_dir(path: &Path) -> bool {
    named(path.file_name(), "projects") && named(path.parent().and_then(Path::file_name), ".claude")
}

/// `path` is, contains, or sits inside the Claude session history, or is the Cursor database.
fn is_protected(path: &Path, home: &Path) -> bool {
    let real = resolve(path);
    let history = resolve(&home.join(".claude/projects"));
    let names: Vec<_> = real.components().map(|c| c.as_os_str()).collect();
    named(real.file_name(), "state.vscdb")
        || names.windows(2).any(|w| named(Some(w[0]), ".claude") && named(Some(w[1]), "projects"))
        || real.starts_with(&history)
        || history.starts_with(&real)
}

/// A protected file or folder anywhere under `dir`. Symlinks are not entered.
fn holds_protected(dir: &Path) -> bool {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        if named(p.file_name(), "state.vscdb") || is_history_dir(&p) {
            return true;
        }
        match fs::symlink_metadata(&p) {
            Ok(md) if md.file_type().is_dir() => match fs::read_dir(&p) {
                Ok(rd) => stack.extend(rd.flatten().map(|e| e.path())),
                Err(_) => return true,
            },
            Ok(_) => {}
            Err(_) => return true,
        }
    }
    false
}

/// Nothing under `.next` may sit on another device than its project folder.
fn next_dir_device_ok(path: &Path, parent_dev: u64) -> bool {
    scan_entry(path, parent_dev).is_some()
}

/// A `.next` folder is only deleted when it still is one: right name, a real folder, in a
/// project, under a scanned folder. Returns the project folder.
fn check_next_dir(path: &Path, scan_folders: &[PathBuf]) -> Result<PathBuf, String> {
    let refuse = |why: &str| Err(format!("Folder ini tidak dihapus: {why}"));
    if path.file_name().is_none_or(|n| n != ".next") {
        return refuse("namanya bukan .next");
    }
    match fs::symlink_metadata(path) {
        Ok(md) if md.file_type().is_dir() => {}
        _ => return refuse(".next bukan folder asli"),
    }
    let project = path.parent().ok_or("Lokasi tidak valid")?.to_path_buf();
    let is_project = fs::read_dir(&project)
        .map(|rd| rd.flatten().any(|e| e.file_name() == "package.json" || e.file_name().to_string_lossy().starts_with("next.config.")))
        .unwrap_or(false);
    if !is_project {
        return refuse("tidak ada package.json atau next.config di sampingnya");
    }
    let parent_dev = fs::metadata(&project).map_err(|e| os_error("Lokasi tidak bisa dibaca", &e))?.dev();
    if !next_dir_device_ok(path, parent_dev) {
        return refuse("isinya melintasi volume lain atau tidak bisa dibaca");
    }
    let real = fs::canonicalize(path).map_err(|e| os_error("Lokasi tidak bisa dibaca", &e))?;
    let scanned = scan_folders.iter().filter_map(|s| fs::canonicalize(s).ok()).any(|s| real.starts_with(s));
    if !scanned {
        return refuse("lokasinya di luar folder project yang dipindai");
    }
    Ok(project)
}

fn active_version(home: &Path, versions: &Path) -> Result<PathBuf, String> {
    let unreadable = || "Versi Claude yang aktif tidak bisa dibaca, jadi tidak ada yang dihapus".to_string();
    let link = home.join(".local/bin/claude");
    // Follows every hop, so a chain through `versions/current` still ends at the real version.
    let target = fs::canonicalize(&link).map_err(|_| unreadable())?;
    let in_versions = target.parent().is_some_and(|p| Some(p) == fs::canonicalize(versions).ok().as_deref());
    if !in_versions || target.file_name().is_none() {
        return Err(unreadable());
    }
    Ok(target)
}

pub fn clean(item: &StorageItem, processes: &[RawProcess], ctx: &CleanContext) -> Result<CleanResult, String> {
    let home = &ctx.paths.home;
    if is_protected(&item.path, home) {
        return Err("Lokasi ini dilindungi dan tidak pernah dibersihkan".into());
    }
    if let Some(why) = refusal(item, processes) {
        return Err(why);
    }
    let before = (ctx.disk_free)();
    let mut skipped = 0;
    match item.cleaner {
        Cleaner::None => return Err("Item ini tidak bisa dibersihkan oleh Pantau".into()),
        Cleaner::RemoveDir => {
            let project = check_next_dir(&item.path, &ctx.paths.scan_folders)?;
            if holds_protected(&item.path) {
                return Err("Folder ini berisi data yang dilindungi dan tidak dihapus".into());
            }
            remove_entry(&item.path, &project)?;
        }
        Cleaner::PnpmPrune => (ctx.run_pnpm)()?,
        Cleaner::OldTemp => {
            let root = &item.path;
            if *root != ctx.paths.tmpdir && *root != ctx.paths.private_tmp {
                return Err("Lokasi ini bukan folder sementara yang dikenal".into());
            }
            let Some(open_paths) = &ctx.open_paths else {
                return Err("Daftar file yang sedang dipakai tidak bisa dibaca, jadi file sementara tidak dihapus".into());
            };
            for entry in temp_candidates(root, ctx.now, open_paths, ctx.me_uid) {
                if is_protected(&entry, home) || holds_protected(&entry) || remove_entry(&entry, root).is_err() {
                    skipped += 1;
                }
            }
        }
        Cleaner::OldClaudeVersions => {
            if item.path != home.join(".local/share/claude/versions") {
                return Err("Lokasi ini bukan folder versi Claude yang dikenal".into());
            }
            let Some(open_paths) = &ctx.open_paths else {
                return Err("Daftar file yang sedang dipakai tidak bisa dibaca, jadi versi Claude tidak dihapus".into());
            };
            let active = active_version(home, &item.path)?;
            for version in old_versions(&item.path, &active) {
                if version_in_use(&version, open_paths) || remove_entry(&version, &item.path).is_err() {
                    skipped += 1;
                }
            }
        }
    }
    Ok(CleanResult { freed_bytes: (ctx.disk_free)().saturating_sub(before), skipped })
}

/// Paths open anywhere on the machine, from one `lsof -Fn` run. `None` when the read fails
/// or comes back empty: unknown is not the same as "nothing is open".
pub fn read_open_paths() -> Option<HashSet<PathBuf>> {
    let mut cmd = Command::new("/usr/sbin/lsof");
    // -n -P: no reverse DNS and no port-name lookups, which are slow and use the network.
    cmd.args(["-nP", "-Fn"]);
    let (code, out) = run_with_timeout(&mut cmd, LSOF_TIMEOUT)?;
    parse_open_paths(code, &out)
}

/// Undoes lsof's escaping in a single left-to-right pass: `\\`, `\t \n \r \b \f`, `\xNN`
/// for bytes it will not print, and, when `carets` is set, `^X` for control characters.
fn unescape_lsof(name: &str, carets: bool) -> OsString {
    let b = name.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let (byte, used) = match (b[i], b.get(i + 1).copied()) {
            (b'\\', Some(b'\\')) => (b'\\', 2),
            (b'\\', Some(b't')) => (b'\t', 2),
            (b'\\', Some(b'n')) => (b'\n', 2),
            (b'\\', Some(b'r')) => (b'\r', 2),
            (b'\\', Some(b'b')) => (0x08, 2),
            (b'\\', Some(b'f')) => (0x0c, 2),
            (b'\\', Some(b'x')) => match name.get(i + 2..i + 4).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(v) => (v, 4),
                None => (b'\\', 1),
            },
            (b'^', Some(c @ b'@'..=b'_')) if carets => (c ^ 0x40, 2),
            (c, _) => (c, 1),
        };
        out.push(byte);
        i += used;
    }
    OsString::from_vec(out)
}

/// lsof shows a Unix socket as it was bound (`/tmp/x`, `/var/x`), but the filesystem and the
/// rest of Pantau use the real `/private/...` form.
fn normalise_lsof_path(name: &str, carets: bool) -> PathBuf {
    let path = PathBuf::from(unescape_lsof(name, carets));
    for dir in ["tmp", "var"] {
        if let Ok(rest) = path.strip_prefix(Path::new("/").join(dir)) {
            return Path::new("/private").join(dir).join(rest);
        }
    }
    path
}

fn parse_open_paths(code: Option<i32>, output: &str) -> Option<HashSet<PathBuf>> {
    // An unfiltered lsof only counts as a full read when it exits 0; 1 means some part failed.
    if code != Some(0) {
        return None;
    }
    // lsof does not escape a literal `^`, so `^A` may be a control character or just the text.
    // Both readings are kept: an extra entry only makes deletion more careful.
    let paths: HashSet<PathBuf> = output
        .lines()
        .filter_map(|l| l.strip_prefix('n'))
        .filter(|n| n.starts_with('/'))
        .flat_map(|n| [normalise_lsof_path(n, false), normalise_lsof_path(n, true)])
        .collect();
    (!paths.is_empty()).then_some(paths)
}

/// `pnpm store prune` as Pantau runs it. pnpm loads `.pnpmfile.cjs` from the working folder and
/// reads `npm_config_*`, `NODE_OPTIONS` and `PNPM_HOME`, so none of that may come from wherever
/// Pantau was started: it runs from `cwd`, a folder of the owner's that is not a project (pnpm
/// writes a temp file there, so `/` fails), with those removed. `inherited` names the variables the
/// process has. `path_env` is the `PATH` the child gets: a GUI app's own has neither `pnpm` nor the
/// `node` its shim needs.
fn pnpm_command(pnpm: &Path, path_env: &OsStr, cwd: &Path, inherited: impl IntoIterator<Item = OsString>) -> Command {
    let mut cmd = Command::new(pnpm);
    cmd.args(["store", "prune"]).current_dir(cwd).env("PATH", path_env).env_remove("NODE_OPTIONS").env_remove("PNPM_HOME");
    for key in inherited {
        if key.to_string_lossy().to_ascii_lowercase().starts_with("npm_config_") {
            cmd.env_remove(key);
        }
    }
    cmd
}

pub fn run_pnpm_at(pnpm: &Path, path_env: &OsStr, cwd: &Path) -> Result<(), String> {
    run_command("pnpm", &mut pnpm_command(pnpm, path_env, cwd, std::env::vars_os().map(|(k, _)| k)), PNPM_TIMEOUT)
}

fn run_command(program: &str, cmd: &mut Command, timeout: Duration) -> Result<(), String> {
    match spawn_with_timeout(cmd, timeout) {
        Ok(Some((Some(0), _))) => Ok(()),
        Ok(Some((code, _))) => Err(format!("{program} gagal dijalankan (kode {})", code.map_or("?".to_string(), |c| c.to_string()))),
        Ok(None) => Err(format!("{program} terlalu lama dan dihentikan")),
        Err(e) if e.kind() == ErrorKind::NotFound => Err(format!("{program} tidak ditemukan di komputer ini")),
        Err(e) => Err(os_error(&format!("{program} tidak bisa dijalankan"), &e)),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Category, Cleaner, ItemState};
    use super::*;
    use crate::process::test_support::proc;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::os::unix::net::UnixListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use std::time::Duration;
    use std::os::unix::fs::MetadataExt;

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    #[test]
    fn pnpm_runs_from_the_given_folder_without_the_variables_that_load_code() {
        let inherited = ["NODE_OPTIONS", "PNPM_HOME", "npm_config_registry", "NPM_CONFIG_USERCONFIG", "npm_config_", "HOME", "NPM_TOKEN"].map(OsString::from);
        let cmd = pnpm_command(Path::new("/x/pnpm"), "/usr/bin".as_ref(), Path::new("/work"), inherited);
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/work")), "pnpm reads .pnpmfile.cjs from the working folder");
        let removed: Vec<String> = cmd.get_envs().filter(|(_, v)| v.is_none()).map(|(k, _)| k.to_string_lossy().into_owned()).collect();
        for gone in ["NODE_OPTIONS", "PNPM_HOME", "npm_config_registry", "NPM_CONFIG_USERCONFIG", "npm_config_"] {
            assert!(removed.iter().any(|k| k == gone), "{gone} still passed on: {removed:?}");
        }
        assert!(!removed.iter().any(|k| k == "HOME" || k == "NPM_TOKEN"), "{removed:?}");
        assert!(cmd.get_envs().any(|(k, v)| k == "PATH" && v == Some("/usr/bin".as_ref())));
    }

    #[test]
    fn a_real_pnpm_run_starts_in_the_given_folder() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("seen.txt");
        let fake = dir.path().join("pnpm");
        fs::write(&fake, format!("#!/bin/sh\npwd > '{}'\n", out.display())).unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        let work = fs::canonicalize(dir.path()).unwrap();
        run_pnpm_at(&fake, "/usr/bin:/bin".as_ref(), &work).unwrap();
        assert_eq!(fs::read_to_string(&out).unwrap().trim_end(), work.to_str().unwrap());
    }

    fn me() -> u32 {
        unsafe { libc::getuid() }
    }

    fn item(path: &Path, cleaner: Cleaner) -> StorageItem {
        StorageItem {
            id: "x".into(),
            title: "x".into(),
            sub: "x".into(),
            path: path.to_path_buf(),
            category: Category::Safe,
            cleaner,
            upper_bound: false,
            bytes: None,
            measured_at_ms: None,
            state: ItemState::Pending,
        }
    }

    fn zero() -> u64 {
        0
    }

    /// Every path lives under `root`; pnpm is a counter instead of a real command.
    fn ctx(root: &Path) -> (CleanContext, Arc<AtomicU32>) {
        let pnpm_runs = Arc::new(AtomicU32::new(0));
        let seen = pnpm_runs.clone();
        let c = CleanContext {
            paths: Paths {
                home: root.join("home"),
                tmpdir: root.join("var-T"),
                private_tmp: root.join("private-tmp"),
                scan_folders: vec![root.join("work")],
            },
            open_paths: Some(HashSet::new()),
            now: SystemTime::now(),
            me_uid: me(),
            disk_free: zero,
            run_pnpm: Box::new(move || {
                seen.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
        };
        (c, pnpm_runs)
    }

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"data").unwrap();
    }

    /// A clock for which everything made by `old` is over 3 days idle and everything made
    /// by `new` is not. Age is judged by ctime, which a test cannot fake, so time is shifted.
    fn clock_between(old: impl FnOnce(), new: impl FnOnce()) -> SystemTime {
        old();
        let after_old = SystemTime::now();
        std::thread::sleep(Duration::from_millis(1200));
        new();
        after_old + 3 * DAY + Duration::from_millis(300)
    }

    fn all_old() -> SystemTime {
        SystemTime::now() + 4 * DAY
    }

    struct Unlock(PathBuf);
    impl Drop for Unlock {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
    }

    fn names(paths: Vec<PathBuf>) -> Vec<String> {
        paths.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect()
    }

    fn next_item(path: &str) -> StorageItem {
        item(Path::new(path), Cleaner::RemoveDir)
    }

    fn project(root: &Path) -> PathBuf {
        let p = root.join("work/cms");
        touch(&p.join("package.json"));
        touch(&p.join(".next/cache/a.bin"));
        p
    }

    // refusal

    #[test]
    fn next_cache_is_refused_while_a_process_runs_from_that_project() {
        let it = next_item("/w/cms/.next");
        let procs = [proc(1, 0).cwd("/w/cms/src")];
        assert_eq!(refusal(&it, &procs), Some("Masih ada proses yang berjalan di project ini".into()));
        assert_eq!(refusal(&it, &[proc(1, 0).cwd("/w/lain")]), None);
        assert_eq!(refusal(&it, &[proc(1, 0).cwd("/w/cms-lama")]), None);
        assert_eq!(refusal(&it, &[proc(1, 0)]), None);
    }

    #[test]
    fn clean_checks_the_running_process_again_before_deleting() {
        let dir = tempfile::tempdir().unwrap();
        let (c, _) = ctx(dir.path());
        let p = project(dir.path());
        let procs = [proc(1, 0).cwd(p.join("src").to_str().unwrap())];
        let err = clean(&item(&p.join(".next"), Cleaner::RemoveDir), &procs, &c).unwrap_err();
        assert_eq!(err, "Masih ada proses yang berjalan di project ini");
        assert!(p.join(".next/cache/a.bin").exists());
    }

    // old Claude versions

    #[test]
    fn the_active_and_newest_claude_versions_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        for v in ["2.1.308", "2.1.314", "2.1.316"] {
            touch(&dir.path().join(v));
        }
        assert_eq!(names(old_versions(dir.path(), &dir.path().join("2.1.314"))), vec!["2.1.308"]);
    }

    #[test]
    fn versions_compare_numerically_and_odd_names_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        for v in ["2.1.98", "2.1.99", "2.1.100", "notes.txt", "2.1.x"] {
            touch(&dir.path().join(v));
        }
        assert_eq!(names(old_versions(dir.path(), &dir.path().join("2.1.98"))), vec!["2.1.99"]);
    }

    // temp candidates

    #[test]
    fn temp_keeps_recent_files_files_in_use_and_other_users_files() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let now = clock_between(
            || {
                touch(&r.join("old.log"));
                touch(&r.join("busy/db.sqlite"));
                touch(&r.join("mixed/old.txt"));
            },
            || {
                touch(&r.join("recent.log"));
                touch(&r.join("mixed/new.txt"));
            },
        );
        let real = fs::canonicalize(r).unwrap();
        let open: HashSet<PathBuf> = [real.join("busy/db.sqlite")].into();
        assert_eq!(names(temp_candidates(r, now, &open, me())), vec!["old.log"]);
        assert!(temp_candidates(r, now, &HashSet::new(), me() + 1).is_empty());
        assert!(temp_candidates(r, SystemTime::now(), &HashSet::new(), me()).is_empty(), "fresh files are not 3 days idle");
    }

    #[test]
    fn age_counts_the_inode_change_time_not_only_the_modification_time() {
        // `tar x`, `cp -p` and `rsync -a` keep an old mtime on a file that was just created.
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("restored.bin"));
        let f = fs::File::open(dir.path().join("restored.bin")).unwrap();
        f.set_modified(SystemTime::now() - 30 * DAY).unwrap();
        assert!(temp_candidates(dir.path(), SystemTime::now(), &HashSet::new(), me()).is_empty());
    }

    #[test]
    fn temp_never_offers_daemon_folders_claude_scratch_empty_entries_or_sockets() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        touch(&r.join("com.apple.useractivityd/x.db"));
        touch(&r.join("COM.APPLE.other"));
        touch(&r.join(format!("claude-{}/scratch.txt", me())));
        fs::create_dir_all(r.join("kosong/sub")).unwrap();
        fs::write(r.join("nol.bin"), b"").unwrap();
        fs::create_dir_all(r.join("daemon")).unwrap();
        fs::write(r.join("daemon/state"), b"data").unwrap();
        let _sock = UnixListener::bind(r.join("daemon/s")).unwrap();
        fs::create_dir_all(r.join("pipa")).unwrap();
        fs::write(r.join("pipa/state"), b"data").unwrap();
        let fifo = std::ffi::CString::new(r.join("pipa/f").to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        touch(&r.join("claude-lain/scratch.txt"));
        touch(&r.join("biasa.log"));
        let found = names(temp_candidates(r, all_old(), &HashSet::new(), me()));
        assert_eq!(found, vec!["biasa.log", "claude-lain"]);
    }

    #[test]
    fn an_entry_that_crosses_onto_another_device_is_not_scanned() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("vol/inside.bin"));
        let dev = fs::symlink_metadata(dir.path().join("vol")).unwrap().dev();
        assert!(scan_entry(&dir.path().join("vol"), dev).is_some());
        assert!(scan_entry(&dir.path().join("vol"), dev + 1).is_none());
    }

    // remove_entry

    #[test]
    fn a_symlink_is_removed_without_touching_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let luar = dir.path().join("luar");
        touch(&luar.join("penting.txt"));
        fs::create_dir_all(&root).unwrap();
        symlink(luar.join("penting.txt"), root.join("link")).unwrap();
        symlink(&luar, root.join("dirlink")).unwrap();
        remove_entry(&root.join("link"), &root).unwrap();
        remove_entry(&root.join("dirlink"), &root).unwrap();
        assert!(luar.join("penting.txt").exists());
        assert!(fs::symlink_metadata(root.join("link")).is_err());
        assert!(fs::symlink_metadata(root.join("dirlink")).is_err());
    }

    #[test]
    fn nothing_outside_the_allowed_root_can_be_removed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let outside_file = dir.path().join("outside.txt");
        touch(&outside_file);
        assert!(remove_entry(&outside_file, &root).is_err());
        assert!(remove_entry(&root.join("../outside.txt"), &root).is_err());
        assert!(remove_entry(&root, &root).is_err());
        assert!(outside_file.exists() && root.exists());
    }

    #[test]
    fn a_symlinked_parent_cannot_lead_out_of_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let luar = dir.path().join("luar");
        touch(&luar.join("penting.txt"));
        fs::create_dir_all(&root).unwrap();
        symlink(&luar, root.join("pintu")).unwrap();
        assert!(remove_entry(&root.join("pintu/penting.txt"), &root).is_err());
        assert!(luar.join("penting.txt").exists());
    }

    #[test]
    fn a_real_folder_inside_the_root_is_removed_with_its_contents() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        touch(&root.join("sub/deep/a.txt"));
        remove_entry(&root.join("sub"), &root).unwrap();
        assert!(!root.join("sub").exists() && root.exists());
    }

    #[test]
    fn a_failed_removal_says_why_in_indonesian() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        touch(&root.join("sub/a.txt"));
        let _unlock = Unlock(root.clone());
        fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).unwrap();
        let why = remove_entry(&root.join("sub"), &root).unwrap_err();
        assert_eq!(why, "Gagal menghapus sub: izin ditolak");
    }

    #[test]
    fn an_unreadable_location_says_why_in_indonesian() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let why = remove_entry(&root.join("hilang/x"), &root).unwrap_err();
        assert_eq!(why, "Lokasi tidak bisa dibaca: lokasinya tidak ada");
        let why = remove_entry(&root.join("x"), &dir.path().join("tidak-ada")).unwrap_err();
        assert_eq!(why, "Folder tujuan tidak bisa dibaca: lokasinya tidak ada");
    }

    // deny list

    #[test]
    fn history_and_cursor_database_can_never_be_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let (c, _) = ctx(dir.path());
        let home = &c.paths.home;
        let history = home.join(".claude/projects");
        touch(&history.join("sesi/chat.jsonl"));
        let cursor = home.join("Library/Application Support/Cursor/User/globalStorage/state.vscdb");
        touch(&cursor);
        let fake = dir.path().join("fake/.claude/projects");
        touch(&fake.join("x.jsonl"));
        symlink(&fake, dir.path().join("pintas")).unwrap();
        touch(&dir.path().join("work/ada/b.txt"));

        let targets = [
            history.clone(),
            history.join("sesi"),
            history.join("sesi/chat.jsonl"),
            home.join(".claude"),
            home.clone(),
            cursor.clone(),
            fake.clone(),
            dir.path().join("pintas"),
            dir.path().join("pintas/x.jsonl"),
            dir.path().join("work/ada/../../fake/.claude/projects"),
            dir.path().join("work/../fake/.claude/projects/x.jsonl"),
            dir.path().join("fake/.claude/projects/belum-ada"),
        ];
        let cleaners = [Cleaner::RemoveDir, Cleaner::PnpmPrune, Cleaner::OldTemp, Cleaner::OldClaudeVersions, Cleaner::None];
        for t in &targets {
            for cl in cleaners {
                assert!(clean(&item(t, cl), &[], &c).is_err(), "{} / {:?}", t.display(), cl);
            }
        }
        assert!(history.join("sesi/chat.jsonl").exists());
        assert!(cursor.exists());
        assert!(fake.join("x.jsonl").exists());
    }

    #[test]
    fn a_temp_entry_holding_history_is_skipped_but_its_neighbours_are_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let (c, _) = ctx(dir.path());
        let t = &c.paths.tmpdir.clone();
        touch(&t.join("kotor/.claude/projects/s.jsonl"));
        touch(&t.join("lama/a.txt"));
        let mut c = c;
        c.now = all_old();
        let done = clean(&item(t, Cleaner::OldTemp), &[], &c).unwrap();
        assert_eq!(done.skipped, 1);
        assert!(t.join("kotor/.claude/projects/s.jsonl").exists());
        assert!(!t.join("lama").exists());
    }

    // .next

    #[test]
    fn a_valid_next_cache_is_removed_and_the_project_stays() {
        let dir = tempfile::tempdir().unwrap();
        let (c, _) = ctx(dir.path());
        let p = project(dir.path());
        clean(&item(&p.join(".next"), Cleaner::RemoveDir), &[], &c).unwrap();
        assert!(!p.join(".next").exists());
        assert!(p.join("package.json").exists());
    }

    #[test]
    fn next_config_alone_marks_a_next_project() {
        let dir = tempfile::tempdir().unwrap();
        let (c, _) = ctx(dir.path());
        let p = dir.path().join("work/app");
        touch(&p.join("next.config.mjs"));
        touch(&p.join(".next/a.bin"));
        clean(&item(&p.join(".next"), Cleaner::RemoveDir), &[], &c).unwrap();
        assert!(!p.join(".next").exists());
    }

    #[test]
    fn a_next_target_is_revalidated_before_removal() {
        let dir = tempfile::tempdir().unwrap();
        let (c, _) = ctx(dir.path());
        let r = dir.path();

        // wrong name
        let p = project(r);
        touch(&p.join("src/keep.txt"));
        let err = clean(&item(&p.join("src"), Cleaner::RemoveDir), &[], &c).unwrap_err();
        assert!(err.contains("bukan .next"), "{err}");
        assert!(p.join("src/keep.txt").exists());

        // a symlink named .next
        let luar = r.join("luar");
        touch(&luar.join("penting.txt"));
        let q = r.join("work/link-proj");
        touch(&q.join("package.json"));
        symlink(&luar, q.join(".next")).unwrap();
        assert!(clean(&item(&q.join(".next"), Cleaner::RemoveDir), &[], &c).is_err());
        assert!(luar.join("penting.txt").exists());

        // a .next with no project beside it
        let s = r.join("work/yatim");
        touch(&s.join(".next/a.bin"));
        let err = clean(&item(&s.join(".next"), Cleaner::RemoveDir), &[], &c).unwrap_err();
        assert!(err.contains("package.json"), "{err}");
        assert!(s.join(".next/a.bin").exists());

        // outside the scanned folders
        let o = r.join("lain/proj");
        touch(&o.join("package.json"));
        touch(&o.join(".next/a.bin"));
        assert!(clean(&item(&o.join(".next"), Cleaner::RemoveDir), &[], &c).is_err());
        assert!(o.join(".next/a.bin").exists());

        // a missing path
        assert!(clean(&item(&p.join("tidak/.next"), Cleaner::RemoveDir), &[], &c).is_err());
    }

    // pnpm

    #[test]
    fn pnpm_prune_goes_through_the_injected_runner_only() {
        let dir = tempfile::tempdir().unwrap();
        let (c, runs) = ctx(dir.path());
        clean(&item(&c.paths.home.join("Library/pnpm/store"), Cleaner::PnpmPrune), &[], &c).unwrap();
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failing_pnpm_runner_surfaces_its_message() {
        let dir = tempfile::tempdir().unwrap();
        let (mut c, _) = ctx(dir.path());
        c.run_pnpm = Box::new(|| Err("pnpm tidak ditemukan di komputer ini".into()));
        let err = clean(&item(&c.paths.home.join("Library/pnpm/store"), Cleaner::PnpmPrune), &[], &c).unwrap_err();
        assert_eq!(err, "pnpm tidak ditemukan di komputer ini");
    }

    fn run(program: &str, args: &[&str], timeout: Duration) -> Result<(), String> {
        run_command(program, Command::new(program).args(args), timeout)
    }

    #[test]
    fn a_missing_program_is_reported_in_words() {
        assert_eq!(run("pantau-program-yang-tidak-ada", &[], Duration::from_secs(5)), Err("pantau-program-yang-tidak-ada tidak ditemukan di komputer ini".into()));
    }

    #[test]
    fn a_program_that_cannot_start_says_why_in_indonesian() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("bukan-program");
        fs::write(&plain, b"data").unwrap();
        let why = run_command("pnpm", &mut Command::new(&plain), Duration::from_secs(5)).unwrap_err();
        assert_eq!(why, "pnpm tidak bisa dijalankan: izin ditolak");
    }

    // temp cleanup

    #[test]
    fn temp_cleanup_removes_only_old_unused_entries() {
        let dir = tempfile::tempdir().unwrap();
        let (mut c, _) = ctx(dir.path());
        let t = c.paths.tmpdir.clone();
        let now = clock_between(
            || {
                touch(&t.join("old.log"));
                touch(&t.join("busy.db"));
            },
            || touch(&t.join("recent.log")),
        );
        c.open_paths = Some([fs::canonicalize(&t).unwrap().join("busy.db")].into());
        c.now = now;
        let done = clean(&item(&t, Cleaner::OldTemp), &[], &c).unwrap();
        assert_eq!(done.skipped, 0);
        assert!(!t.join("old.log").exists());
        assert!(t.join("recent.log").exists() && t.join("busy.db").exists());
    }

    #[test]
    fn temp_cleanup_refuses_when_open_files_are_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let (mut c, _) = ctx(dir.path());
        let t = c.paths.tmpdir.clone();
        touch(&t.join("old.log"));
        c.now = all_old();
        c.open_paths = None;
        assert!(clean(&item(&t, Cleaner::OldTemp), &[], &c).is_err());
        assert!(t.join("old.log").exists());
    }

    #[test]
    fn temp_cleanup_only_runs_on_the_two_known_roots() {
        let dir = tempfile::tempdir().unwrap();
        let (c, _) = ctx(dir.path());
        let other = dir.path().join("lain");
        touch(&other.join("old.log"));
        let mut c = c;
        c.now = all_old();
        assert!(clean(&item(&other, Cleaner::OldTemp), &[], &c).is_err());
        assert!(other.join("old.log").exists());
    }

    // old Claude versions, end to end

    fn versions_setup(c: &CleanContext) -> (PathBuf, PathBuf) {
        let versions = c.paths.home.join(".local/share/claude/versions");
        for v in ["2.1.99", "2.1.100", "2.1.314", "2.1.316"] {
            touch(&versions.join(v));
        }
        let link = c.paths.home.join(".local/bin/claude");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        (versions, link)
    }

    #[test]
    fn old_claude_versions_are_removed_but_active_and_newest_stay() {
        let dir = tempfile::tempdir().unwrap();
        let (c, _) = ctx(dir.path());
        let (versions, link) = versions_setup(&c);
        symlink(versions.join("2.1.314"), &link).unwrap();
        clean(&item(&versions, Cleaner::OldClaudeVersions), &[], &c).unwrap();
        let left: Vec<String> = {
            let mut v = names(fs::read_dir(&versions).unwrap().map(|e| e.unwrap().path()).collect());
            v.sort();
            v
        };
        assert_eq!(left, vec!["2.1.314", "2.1.316"]);
    }

    #[test]
    fn a_version_a_live_session_runs_from_is_kept_and_counted() {
        // Real case: the link points at 2.1.316 while sessions still run from 2.1.314.
        let dir = tempfile::tempdir().unwrap();
        let (mut c, _) = ctx(dir.path());
        let (versions, link) = versions_setup(&c);
        symlink(versions.join("2.1.316"), &link).unwrap();
        // lsof prints real paths, and the temp folder sits behind the `/var` symlink.
        c.open_paths = Some([fs::canonicalize(&versions).unwrap().join("2.1.314")].into());
        let done = clean(&item(&versions, Cleaner::OldClaudeVersions), &[], &c).unwrap();
        assert!(versions.join("2.1.314").exists(), "a live session runs from it");
        assert!(versions.join("2.1.316").exists());
        assert!(!versions.join("2.1.99").exists() && !versions.join("2.1.100").exists());
        assert_eq!(done.skipped, 1);
    }

    #[test]
    fn old_versions_refuse_when_open_files_are_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let (mut c, _) = ctx(dir.path());
        let (versions, link) = versions_setup(&c);
        symlink(versions.join("2.1.316"), &link).unwrap();
        c.open_paths = None;
        assert!(clean(&item(&versions, Cleaner::OldClaudeVersions), &[], &c).is_err());
        assert_eq!(fs::read_dir(&versions).unwrap().count(), 4);
    }

    #[test]
    #[ignore = "helper: only sleeps, started by the test below"]
    fn helper_that_sleeps() {
        std::thread::sleep(Duration::from_secs(60));
    }

    #[test]
    fn a_version_that_is_really_running_is_seen_by_the_real_lsof() {
        // A real process, started here, running from a version file in a temp folder: a copy of this
        // test binary (a copied /bin/sleep will not start, macOS kills it).
        let dir = tempfile::tempdir().unwrap();
        let (mut c, _) = ctx(dir.path());
        let (versions, link) = versions_setup(&c);
        symlink(versions.join("2.1.316"), &link).unwrap();
        fs::remove_file(versions.join("2.1.314")).unwrap();
        fs::copy(std::env::current_exe().unwrap(), versions.join("2.1.314")).unwrap();
        struct Stop(std::process::Child);
        impl Drop for Stop {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut child = Command::new(versions.join("2.1.314"))
            .args(["--ignored", "--exact", "storage::clean::tests::helper_that_sleeps"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(child.try_wait().unwrap(), None, "the copy must be running for this test to mean anything");
        let _running = Stop(child);
        c.open_paths = read_open_paths();
        let got = clean(&item(&versions, Cleaner::OldClaudeVersions), &[], &c);
        assert!(versions.join("2.1.314").exists());
        // A failed lsof refuses instead, which also keeps the file.
        if c.open_paths.is_some() {
            assert_eq!(got.unwrap().skipped, 1);
        } else {
            assert!(got.is_err());
        }
    }

    #[test]
    fn an_unreadable_active_link_deletes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (c, _) = ctx(dir.path());
        let (versions, link) = versions_setup(&c);
        // no link at all
        let err = clean(&item(&versions, Cleaner::OldClaudeVersions), &[], &c).unwrap_err();
        assert!(err.contains("tidak bisa dibaca"), "{err}");
        // a plain file instead of a symlink
        touch(&link);
        assert!(clean(&item(&versions, Cleaner::OldClaudeVersions), &[], &c).is_err());
        fs::remove_file(&link).unwrap();
        // a link that points outside the versions folder
        symlink(dir.path().join("work"), &link).unwrap();
        assert!(clean(&item(&versions, Cleaner::OldClaudeVersions), &[], &c).is_err());
        assert_eq!(fs::read_dir(&versions).unwrap().count(), 4);
    }

    // refusal through symlinks

    #[test]
    fn a_process_in_the_real_folder_of_a_symlinked_project_still_blocks_next_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let real = fs::canonicalize(dir.path()).unwrap();
        touch(&real.join("ssd/cms/.next/a.bin"));
        fs::create_dir_all(real.join("work")).unwrap();
        symlink(real.join("ssd"), real.join("work/link")).unwrap();
        let it = next_item(real.join("work/link/cms/.next").to_str().unwrap());
        let procs = [proc(1, 0).cwd(real.join("ssd/cms/src").to_str().unwrap())];
        assert!(refusal(&it, &procs).is_some());
        assert_eq!(refusal(&it, &[proc(1, 0).cwd(real.join("ssd/lain").to_str().unwrap())]), None);
    }

    // active version through a chain of links

    #[test]
    fn the_active_version_is_found_through_a_chain_of_links() {
        let dir = tempfile::tempdir().unwrap();
        let (c, _) = ctx(dir.path());
        let (versions, link) = versions_setup(&c);
        symlink(versions.join("2.1.99"), versions.join("current")).unwrap();
        symlink(versions.join("current"), &link).unwrap();
        clean(&item(&versions, Cleaner::OldClaudeVersions), &[], &c).unwrap();
        assert!(versions.join("2.1.99").exists(), "the true active version must stay");
        assert!(!versions.join("2.1.100").exists() && !versions.join("2.1.314").exists());
    }

    #[test]
    fn skipped_entries_are_counted() {
        let dir = tempfile::tempdir().unwrap();
        let (c, _) = ctx(dir.path());
        let (versions, link) = versions_setup(&c);
        symlink(versions.join("2.1.314"), &link).unwrap();
        let _unlock = Unlock(versions.clone());
        fs::set_permissions(&versions, fs::Permissions::from_mode(0o555)).unwrap();
        let done = clean(&item(&versions, Cleaner::OldClaudeVersions), &[], &c).unwrap();
        assert_eq!(done.skipped, 2);
        assert!(versions.join("2.1.99").exists());
    }

    // deny list ignores case

    #[test]
    fn the_deny_list_ignores_letter_case() {
        let dir = tempfile::tempdir().unwrap();
        let (c, _) = ctx(dir.path());
        let r = dir.path();
        for t in [r.join("x/.CLAUDE/Projects"), r.join("x/.Claude/projects/a"), r.join("y/STATE.VSCDB")] {
            assert!(is_protected(&t, &c.paths.home), "{}", t.display());
        }
    }

    // plumbing

    #[test]
    fn the_context_can_cross_threads() {
        fn needs<T: Send + Sync>() {}
        needs::<CleanContext>();
    }

    #[test]
    fn a_hung_or_failing_program_is_reported() {
        let slow = run("sleep", &["5"], Duration::from_millis(200)).unwrap_err();
        assert!(slow.contains("terlalu lama"), "{slow}");
        assert!(run("false", &[], Duration::from_secs(5)).unwrap_err().contains("gagal"));
        assert!(run("true", &[], Duration::from_secs(5)).is_ok());
    }

    // freed bytes

    fn grows() -> u64 {
        static N: AtomicU64 = AtomicU64::new(0);
        1000 + 4000 * N.fetch_add(1, Ordering::SeqCst)
    }

    fn shrinks() -> u64 {
        static N: AtomicU64 = AtomicU64::new(0);
        5000 - 4000 * N.fetch_add(1, Ordering::SeqCst)
    }

    #[test]
    fn freed_bytes_is_the_free_space_gained_and_never_negative() {
        let dir = tempfile::tempdir().unwrap();
        let (mut c, _) = ctx(dir.path());
        let it = item(&c.paths.home.join("Library/pnpm/store"), Cleaner::PnpmPrune);
        c.disk_free = grows;
        assert_eq!(clean(&it, &[], &c).unwrap().freed_bytes, 4000);
        c.disk_free = shrinks;
        assert_eq!(clean(&it, &[], &c).unwrap().freed_bytes, 0);
    }

    #[test]
    fn an_item_without_a_cleaner_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (c, _) = ctx(dir.path());
        assert!(clean(&item(&c.paths.home.join("Library/Application Support/Claude/vm_bundles"), Cleaner::None), &[], &c).is_err());
    }

    // lsof

    #[test]
    fn lsof_output_gives_open_paths_and_a_failed_run_gives_none() {
        let out = "p1\nfcwd\nn/private/tmp/a\nf3\nn*:8080\nn/Users/x/b c.txt\nf4\nn(deleted)\n";
        let paths = parse_open_paths(Some(0), out).unwrap();
        assert_eq!(paths, HashSet::from([PathBuf::from("/private/tmp/a"), PathBuf::from("/Users/x/b c.txt")]));
        assert!(parse_open_paths(Some(0), "").is_none());
        assert!(parse_open_paths(None, out).is_none());
        assert!(parse_open_paths(Some(2), out).is_none());
        assert!(parse_open_paths(Some(1), out).is_none(), "exit 1 is a failure for an unfiltered lsof");
    }

    #[test]
    fn lsof_paths_are_normalised_to_the_form_the_filesystem_uses() {
        let out = "n/tmp/codex/live.sock\nn/var/folders/ab/T/x\nn/tmp\nn/variant/x\nn/tmpx/y\nn/Users/x/na\\xc3\\xafve dir/f\n";
        let paths = parse_open_paths(Some(0), out).unwrap();
        for want in ["/private/tmp/codex/live.sock", "/private/var/folders/ab/T/x", "/private/tmp", "/variant/x", "/tmpx/y", "/Users/x/naïve dir/f"] {
            assert!(paths.contains(Path::new(want)), "{want} in {paths:?}");
        }
    }

    #[test]
    fn a_socket_open_under_its_tmp_alias_keeps_its_folder_out_of_the_candidates() {
        // lsof says `/tmp/...` for a socket whose real path is `/private/tmp/...`.
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let real = fs::canonicalize(dir.path()).unwrap();
        assert!(real.starts_with("/private/tmp"), "{}", real.display());
        touch(&real.join("live/state"));
        let alias = format!("n/tmp/{}/live/s.sock\n", dir.path().file_name().unwrap().to_string_lossy());
        let open = parse_open_paths(Some(0), &alias).unwrap();
        assert!(temp_candidates(&real, all_old(), &open, me()).is_empty());
        assert_eq!(temp_candidates(&real, all_old(), &HashSet::new(), me()), vec![real.join("live")]);
    }

    #[test]
    fn the_device_to_compare_against_is_the_roots_not_the_entrys() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("vol/data.bin"));
        let dev = fs::metadata(dir.path()).unwrap().dev();
        let all = all_old();
        // An entry that is itself a mount point has its own device; modelled by a root device
        // that differs from the one the entry reports.
        assert!(temp_candidates_on(dir.path(), dev + 1, all, &HashSet::new(), me()).is_empty());
        assert_eq!(names(temp_candidates_on(dir.path(), dev, all, &HashSet::new(), me())), vec!["vol"]);
        assert_eq!(names(temp_candidates(dir.path(), all, &HashSet::new(), me())), vec!["vol"]);
    }

    #[test]
    fn a_next_folder_on_another_device_than_its_project_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("p/.next/a.bin"));
        let dev = fs::metadata(dir.path().join("p")).unwrap().dev();
        assert!(next_dir_device_ok(&dir.path().join("p/.next"), dev));
        assert!(!next_dir_device_ok(&dir.path().join("p/.next"), dev + 1));
    }

    #[test]
    fn lsof_escapes_are_decoded_in_one_left_to_right_pass() {
        let out = "n/a/back\\\\slash\nn/a/tab\\there\nn/a/lit\\\\x41dir\nn/a/nl\\nx\nn/a/cr\\rx\nn/a/bs\\bx\nn/a/ff\\fx\nn/a/ctl^Ix\nn/a/del\\x7fx\nn/a/hi^bx\n";
        let got = parse_open_paths(Some(0), out).unwrap();
        for want in ["/a/back\\slash", "/a/tab\there", "/a/lit\\x41dir", "/a/nl\nx", "/a/cr\rx", "/a/bs\x08x", "/a/ff\x0cx", "/a/ctl\tx", "/a/ctl^Ix", "/a/del\x7fx", "/a/hi^bx"] {
            assert!(got.contains(Path::new(want)), "{want:?} in {got:?}");
        }
    }

    #[test]
    #[ignore = "reads the real open files with lsof; read-only"]
    fn print_real_open_paths() {
        let open = read_open_paths();
        println!("open paths: {:?}", open.as_ref().map(|s| s.len()));
    }

    /// Prints which real Claude versions a cleanup would take. Deletes nothing:
    /// `cargo test print_real_old_versions -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads the real versions folder and open files; deletes nothing"]
    fn print_real_old_versions() {
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let dir = home.join(".local/share/claude/versions");
        let active = active_version(&home, &dir).unwrap();
        println!("active: {}", active.display());
        let open = read_open_paths();
        println!("lsof: {:?} paths", open.as_ref().map(HashSet::len));
        for v in old_versions(&dir, &active) {
            let live = open.as_ref().is_some_and(|o| version_in_use(&v, o));
            println!("{}: {}", v.display(), if live { "kept, open" } else { "would be removed" });
        }
    }
}
