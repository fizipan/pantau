//! Owner settings kept in `settings.json`. Launch at login is not stored here: the autostart
//! plugin is the source of truth, so the file can never disagree with it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

pub const DEFAULT_SHORTCUT: &str = "Ctrl+Alt+M";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub shortcut: String,
    pub notifications: bool,
    /// Deletion roots: `.next` folders are only cleaned under these. Validated, canonical paths.
    pub scan_folders: Vec<PathBuf>,
}

/// The home folder, or `/` when it cannot be read. `/` is refused by `validate_scan_folder`, so an
/// unreadable home means no folder is accepted, never the whole disk.
pub fn home() -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// `~/Documents`, if it passes the same checks as a chosen folder; so the window never shows a
/// folder that is not really scanned.
pub fn default_scan_folders(home: &Path) -> Vec<PathBuf> {
    revalidate(&[home.join("Documents")], home)
}

impl Default for Settings {
    fn default() -> Self {
        Settings { shortcut: DEFAULT_SHORTCUT.into(), notifications: true, scan_folders: default_scan_folders(&home()) }
    }
}

/// A missing or corrupt file is the default settings, never an error. Folders that no longer
/// pass validation (deleted, or edited by hand to something unsafe) are dropped.
pub fn load(path: &Path) -> Settings {
    load_in(path, &home())
}

pub fn load_in(path: &Path, home: &Path) -> Settings {
    let Some(mut s) = fs::read(path).ok().and_then(|b| serde_json::from_slice::<Settings>(&b).ok()) else {
        return Settings { scan_folders: default_scan_folders(home), ..Settings::default() };
    };
    s.scan_folders = revalidate(&s.scan_folders, home);
    s
}

/// Writes next to `path` and renames over, so a crash never leaves half a file.
pub fn save(path: &Path, s: &Settings) -> io::Result<()> {
    // Saves share one temp name, so they must not overlap.
    static SAVING: Mutex<()> = Mutex::new(());
    let _one_at_a_time = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(s).map_err(io::Error::other)?)?;
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// Folders are deletion roots, so each one must be a real folder inside the user's home and
/// must not reach the home folder itself, `~/Library`, or `~/.claude`. Returns the canonical path.
/// `home` is passed in so tests can use a fake one.
pub fn validate_scan_folder(path: &Path, home: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("Folder harus berupa path absolut".into());
    }
    let home = fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    if !home.is_absolute() || home == Path::new("/") {
        return Err("Folder Home tidak terbaca, jadi folder tidak bisa dipilih".into());
    }
    let real = fs::canonicalize(path).map_err(|_| "Folder tidak ditemukan".to_string())?;
    if !real.is_dir() {
        return Err("Ini bukan folder".into());
    }
    if real == home {
        return Err("Folder Home tidak boleh dipindai seluruhnya, pilih folder project di dalamnya".into());
    }
    if !real.starts_with(&home) {
        return Err("Folder harus berada di dalam folder Home".into());
    }
    if real.starts_with(home.join("Library")) {
        return Err("Folder Library tidak boleh dipindai".into());
    }
    let claude = home.join(".claude");
    // `~/.claude` and its `projects` folder may be links, so their real locations count too.
    let touches_claude = [claude.clone(), claude.join("projects")]
        .into_iter()
        .flat_map(|p| [Some(p.clone()), fs::canonicalize(&p).ok()])
        .flatten()
        .any(|c| real.starts_with(&c) || c.starts_with(&real));
    if touches_claude {
        return Err("Folder .claude dan folder di sekitarnya tidak boleh dipindai".into());
    }
    Ok(real)
}

/// Validates every folder, drops repeats, and refuses the whole list on the first bad one.
pub fn validate_scan_folders(paths: &[PathBuf], home: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out: Vec<PathBuf> = Vec::new();
    for p in paths {
        let real = validate_scan_folder(p, home).map_err(|why| format!("{}: {why}", p.display()))?;
        if !out.contains(&real) {
            out.push(real);
        }
    }
    Ok(out)
}

/// The still-valid part of a list, for files and state that may be stale.
fn revalidate(paths: &[PathBuf], home: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for real in paths.iter().filter_map(|p| validate_scan_folder(p, home).ok()) {
        if !out.contains(&real) {
            out.push(real);
        }
    }
    out
}

/// A shortcut must have a modifier other than Shift, or it would swallow ordinary typing.
pub fn validate_shortcut(s: &str) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{Modifiers, Shortcut};
    let shortcut: Shortcut = s.parse().map_err(|_| "Pintasan tidak dikenali".to_string())?;
    if !shortcut.mods.intersects(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SUPER) {
        return Err("Pintasan harus memakai Control, Option, atau Command".into());
    }
    Ok(())
}

// ---------------------------------------------------------------- live values
// What the running app uses. Changing them needs no restart.

static SCAN_FOLDERS: Mutex<Option<Vec<PathBuf>>> = Mutex::new(None);
static NOTIFICATIONS: AtomicBool = AtomicBool::new(true);

/// The saved folders, checked again now; `None` until settings were loaded, which means the default.
pub fn active_scan_folders(home: &Path) -> Option<Vec<PathBuf>> {
    SCAN_FOLDERS.lock().unwrap_or_else(|e| e.into_inner()).as_deref().map(|v| revalidate(v, home))
}

pub fn set_active(s: &Settings) {
    *SCAN_FOLDERS.lock().unwrap_or_else(|e| e.into_inner()) = Some(s.scan_folders.clone());
    NOTIFICATIONS.store(s.notifications, Ordering::Relaxed);
}

pub fn notifications_enabled() -> bool {
    NOTIFICATIONS.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    struct Fake {
        _dir: tempfile::TempDir,
        home: PathBuf,
    }

    fn fake_home() -> Fake {
        let dir = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(dir.path()).unwrap().join("home");
        fs::create_dir_all(home.join("Documents/app")).unwrap();
        fs::create_dir_all(home.join("Library/Caches")).unwrap();
        fs::create_dir_all(home.join(".claude/projects")).unwrap();
        Fake { _dir: dir, home }
    }

    fn refused(f: &Fake, p: &Path) -> String {
        validate_scan_folder(p, &f.home).expect_err(&format!("{} must be refused", p.display()))
    }

    #[test]
    fn settings_default_and_corrupt_file_fall_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("none.json");
        assert_eq!(load(&missing).shortcut, "Ctrl+Alt+M");
        let d = load(&missing);
        assert!(d.notifications);
        assert_eq!(d.scan_folders, vec![home().join("Documents")]);
        let bad = dir.path().join("bad.json");
        fs::write(&bad, b"{not json").unwrap();
        assert_eq!(load(&bad), Settings::default());
    }

    #[test]
    fn settings_round_trip_and_keep_an_empty_folder_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/settings.json");
        let s = Settings { shortcut: "Ctrl+Alt+P".into(), notifications: false, scan_folders: vec![] };
        save(&path, &s).unwrap();
        assert_eq!(load(&path), s, "an empty list means no folders, not the default");
    }

    #[test]
    fn an_old_file_with_launch_at_login_and_no_folders_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, br#"{"launch_at_login":true,"shortcut":"Ctrl+Alt+P","notifications":false}"#).unwrap();
        let s = load(&path);
        assert_eq!((s.shortcut.as_str(), s.notifications), ("Ctrl+Alt+P", false));
        assert_eq!(s.scan_folders, Settings::default().scan_folders);
    }

    #[test]
    fn a_folder_inside_home_is_accepted_and_canonicalised() {
        let f = fake_home();
        let link = f.home.join("Documents/alias");
        symlink(f.home.join("Documents/app"), &link).unwrap();
        assert_eq!(validate_scan_folder(&f.home.join("Documents"), &f.home).unwrap(), f.home.join("Documents"));
        assert_eq!(validate_scan_folder(&link, &f.home).unwrap(), f.home.join("Documents/app"));
        assert_eq!(validate_scan_folder(&f.home.join("Documents/app/../app"), &f.home).unwrap(), f.home.join("Documents/app"));
    }

    #[test]
    fn a_relative_path_is_refused() {
        let f = fake_home();
        assert!(refused(&f, Path::new("Documents")).contains("absolut"));
    }

    #[test]
    fn a_missing_folder_or_a_file_is_refused() {
        let f = fake_home();
        assert!(refused(&f, &f.home.join("Documents/ghost")).contains("tidak ditemukan"));
        let file = f.home.join("Documents/a.txt");
        fs::write(&file, "x").unwrap();
        assert!(refused(&f, &file).contains("bukan folder"));
    }

    #[test]
    fn a_folder_outside_home_is_refused() {
        let f = fake_home();
        let other = tempfile::tempdir().unwrap();
        assert!(refused(&f, other.path()).contains("di dalam folder Home"));
        assert!(refused(&f, Path::new("/")).contains("di dalam folder Home"));
        assert!(refused(&f, Path::new("/tmp")).contains("di dalam folder Home"));
    }

    #[test]
    fn a_symlink_that_leads_out_of_home_is_refused() {
        let f = fake_home();
        let other = tempfile::tempdir().unwrap();
        let link = f.home.join("Documents/out");
        symlink(other.path(), &link).unwrap();
        assert!(refused(&f, &link).contains("di dalam folder Home"));
    }

    #[test]
    fn the_home_folder_itself_is_refused() {
        let f = fake_home();
        assert!(refused(&f, &f.home).contains("Folder Home"));
        assert!(refused(&f, &f.home.join("Documents/..")).contains("Folder Home"));
        let link = f.home.join("Documents/home-link");
        symlink(&f.home, &link).unwrap();
        assert!(refused(&f, &link).contains("Folder Home"));
    }

    #[test]
    fn library_and_everything_in_it_is_refused() {
        let f = fake_home();
        assert!(refused(&f, &f.home.join("Library")).contains("Library"));
        assert!(refused(&f, &f.home.join("Library/Caches")).contains("Library"));
    }

    #[test]
    fn dot_claude_and_its_descendants_are_refused() {
        let f = fake_home();
        assert!(refused(&f, &f.home.join(".claude")).contains(".claude"));
        assert!(refused(&f, &f.home.join(".claude/projects")).contains(".claude"));
        let link = f.home.join("Documents/to-claude");
        symlink(f.home.join(".claude/projects"), &link).unwrap();
        assert!(refused(&f, &link).contains(".claude"));
    }

    #[test]
    fn an_ancestor_of_dot_claude_is_refused_even_when_it_is_a_link_elsewhere() {
        let f = fake_home();
        // ~/.claude is a link into ~/dotfiles/claude, so ~/dotfiles is its real ancestor.
        let dotfiles = f.home.join("dotfiles");
        fs::create_dir_all(dotfiles.join("claude")).unwrap();
        fs::remove_dir_all(f.home.join(".claude")).unwrap();
        symlink(dotfiles.join("claude"), f.home.join(".claude")).unwrap();
        assert!(refused(&f, &dotfiles).contains(".claude"));
        assert!(refused(&f, &dotfiles.join("claude")).contains(".claude"));
    }

    #[test]
    fn a_list_is_deduplicated_and_one_bad_folder_refuses_the_whole_list() {
        let f = fake_home();
        let ok = f.home.join("Documents");
        let list = vec![ok.clone(), f.home.join("Documents/app/.."), f.home.join("Documents/app")];
        assert_eq!(validate_scan_folders(&list, &f.home).unwrap(), vec![ok.clone(), f.home.join("Documents/app")]);
        let err = validate_scan_folders(&[ok, f.home.clone()], &f.home).unwrap_err();
        assert!(err.contains("Folder Home"));
        assert_eq!(validate_scan_folders(&[], &f.home).unwrap(), Vec::<PathBuf>::new());
    }

    #[test]
    fn load_drops_folders_that_no_longer_validate() {
        let f = fake_home();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let s = Settings { shortcut: "Ctrl+Alt+M".into(), notifications: true, scan_folders: vec![f.home.join("Documents"), "/".into(), f.home.join("gone")] };
        save(&path, &s).unwrap();
        assert_eq!(load_in(&path, &f.home).scan_folders, vec![f.home.join("Documents")]);
    }

    #[test]
    fn an_unreadable_home_accepts_no_folder() {
        let f = fake_home();
        assert!(validate_scan_folder(&f.home, Path::new("/")).unwrap_err().contains("Home tidak terbaca"));
        assert!(validate_scan_folder(Path::new("/tmp"), Path::new("/")).is_err());
        assert!(validate_scan_folders(&[f.home.join("Documents")], Path::new("/")).is_err());
        assert!(default_scan_folders(Path::new("/")).is_empty());
    }

    #[test]
    fn the_default_folder_is_only_listed_when_it_passes_validation() {
        let f = fake_home();
        assert_eq!(default_scan_folders(&f.home), vec![f.home.join("Documents")]);
        fs::remove_dir_all(f.home.join("Documents")).unwrap();
        assert!(default_scan_folders(&f.home).is_empty(), "missing");
        let other = tempfile::tempdir().unwrap();
        symlink(other.path(), f.home.join("Documents")).unwrap();
        assert!(default_scan_folders(&f.home).is_empty(), "a link that leaves home");
        // No file at all gives the same defaults.
        assert!(load_in(&f.home.join("none.json"), &f.home).scan_folders.is_empty());
    }

    #[test]
    fn the_real_target_of_a_linked_claude_projects_cannot_be_chosen() {
        let f = fake_home();
        let store = f.home.join("elsewhere/store");
        fs::create_dir_all(&store).unwrap();
        fs::remove_dir_all(f.home.join(".claude/projects")).unwrap();
        symlink(&store, f.home.join(".claude/projects")).unwrap();
        assert!(refused(&f, &store).contains(".claude"));
        assert!(refused(&f, &f.home.join("elsewhere")).contains(".claude"));
    }

    #[test]
    fn a_shortcut_needs_a_modifier_and_a_real_key() {
        for ok in ["Ctrl+Alt+M", "Ctrl+Shift+Space", "Cmd+1", "Ctrl+Alt+F5", "Alt+Up"] {
            assert!(validate_shortcut(ok).is_ok(), "{ok}");
        }
        for bad in ["", "M", "F5", "Ctrl", "Ctrl+", "Ctrl+Alt+Bogus", "Ctrl+M+K"] {
            let e = validate_shortcut(bad).expect_err(bad);
            assert!(e.starts_with("Pintasan"), "{e}");
        }
    }

    #[test]
    fn concurrent_settings_saves_never_corrupt_the_file() {
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let path = Arc::new(dir.path().join("settings.json"));
        let threads: Vec<_> = (0..8)
            .map(|n| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for i in 0..60 {
                        let s = Settings { shortcut: format!("Ctrl+Alt+{}", "X".repeat(200 + i)), notifications: n % 2 == 0, scan_folders: vec![] };
                        save(&path, &s).expect("save must not fail because of another save");
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let text = fs::read_to_string(&*path).unwrap();
        assert!(serde_json::from_str::<Settings>(&text).is_ok(), "settings.json is corrupt");
    }
}
