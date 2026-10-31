//! When to measure and scan, and the one background worker that does it.
//!
//! One job at a time, on its own thread at background priority, so it never blocks the sampler or
//! a command. Automatic jobs run only while the panel is open (spec 5.1: only system metrics with
//! the panel closed); what the owner asked for may finish. Nothing here deletes.

use std::fs;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::measure::{Measured, denied_roots, measure_while};
use super::recent::{RecentFile, default_excludes, scan_recent};
use super::rules::storage_items;
use super::{ItemState, Paths, StorageItem};
use crate::system::Pressure;

const MEASURE_EVERY_MS: u64 = 10 * 60 * 1000;
const RECENT_EVERY_MS: u64 = 24 * 60 * 60 * 1000;

/// Critical pressure pauses the automatic runs; a manual request always goes through.
fn due(last_ms: Option<u64>, now_ms: u64, pressure: Pressure, manual: bool, every_ms: u64) -> bool {
    if manual {
        return true;
    }
    pressure != Pressure::Critical && last_ms.is_none_or(|l| now_ms.saturating_sub(l) > every_ms)
}

pub fn should_measure(last_ms: Option<u64>, now_ms: u64, pressure: Pressure, manual: bool) -> bool {
    due(last_ms, now_ms, pressure, manual, MEASURE_EVERY_MS)
}

pub fn should_scan_recent(last_ms: Option<u64>, now_ms: u64, pressure: Pressure, manual: bool) -> bool {
    due(last_ms, now_ms, pressure, manual, RECENT_EVERY_MS)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCache {
    pub items: Vec<StorageItem>,
    pub recent: Vec<RecentFile>,
    pub recent_at_ms: Option<u64>,
    #[serde(default)]
    pub recent_skipped_dirs: usize,
    #[serde(default)]
    pub recent_denied: bool,
    #[serde(default)]
    pub denied_roots: Vec<PathBuf>,
}

/// A missing or corrupt file is an empty cache, never an error. Paths come back from this file to
/// `open`, so one that is not absolute (`-a/...` would read as an option) is dropped.
pub fn load_cache(path: &Path) -> StorageCache {
    let mut cache: StorageCache = fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    cache.items.retain(|i| i.path.is_absolute());
    cache.recent.retain(|r| r.path.is_absolute());
    cache
}

/// Writes a temp file next to `path` and renames it over, so a crash never leaves half a file.
pub fn save_cache(path: &Path, c: &StorageCache) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec(c).map_err(io::Error::other)?)?;
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Fresh items from the rule table, carrying over the last reading of any item seen before,
/// so the old size stays visible with its timestamp until the new one replaces it.
fn merge_items(old: &[StorageItem], fresh: Vec<StorageItem>) -> Vec<StorageItem> {
    fresh
        .into_iter()
        .map(|mut it| {
            if let Some(o) = old.iter().find(|o| o.id == it.id) {
                it.bytes = o.bytes;
                it.measured_at_ms = o.measured_at_ms;
                it.state = o.state.clone();
            }
            it
        })
        .collect()
}

#[derive(Default)]
struct State {
    items: Vec<StorageItem>,
    recent: Vec<RecentFile>,
    recent_at_ms: Option<u64>,
    recent_skipped_dirs: usize,
    recent_denied: bool,
    denied_roots: Vec<PathBuf>,
    measuring: bool,
    scanning: bool,
    /// The running job is the owner's, or was upgraded to it: only the worker's own reads of this
    /// decide whether the panel closing stops it.
    measure_manual: bool,
    scan_manual: bool,
    /// Items whose walk an automatic pass had to abandon. They go last next time, so one folder
    /// that outlasts the panel cannot starve the others. In memory only.
    stopped: Vec<String>,
    /// Cleaned items to measure once the running job ends.
    remeasure: Vec<String>,
    /// A manual measure was asked for while one was running; the worker runs it again at the end,
    /// because the running pass may already have read items that changed since.
    measure_again: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub items: Vec<StorageItem>,
    pub recent: Vec<RecentFile>,
    pub recent_at_ms: Option<u64>,
    /// Folders the last complete scan could not read.
    pub recent_skipped_dirs: usize,
    /// The scan root itself was unreadable: an empty list means "could not look", not "no files".
    pub recent_denied: bool,
    /// Project folders that could not be listed, so their `.next` and `node_modules` rows are missing.
    pub denied_roots: Vec<PathBuf>,
    /// Memory pressure is critical: automatic runs are held back, manual ones still work.
    pub paused: bool,
    pub measuring: bool,
    pub scanning: bool,
    /// The running measure was not asked for by the owner: the panel closing stops it.
    pub measuring_auto: bool,
    /// The running scan was not asked for by the owner: the panel closing stops it.
    pub scanning_auto: bool,
}

#[derive(Clone)]
enum Job {
    Measure,
    Recent,
    /// One item, after the owner cleaned it.
    MeasureItem(String),
    #[cfg(test)]
    Panic,
}

pub struct Scheduler {
    state: Arc<Mutex<State>>,
    pressure: Arc<dyn Fn() -> Pressure + Send + Sync>,
    tx: Sender<Job>,
    /// The project folders the worker reads at the start of every pass.
    scan_folders: Arc<Mutex<Vec<PathBuf>>>,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

/// Background QoS for the calling thread: the kernel yields CPU and disk to everything else.
fn lower_this_thread_priority() {
    unsafe {
        libc::setpriority(libc::PRIO_DARWIN_THREAD, 0, libc::PRIO_DARWIN_BG);
    }
}

impl Scheduler {
    /// Loads the cache so old numbers show at once, then starts the worker thread.
    pub fn start(paths: Paths, cache_path: PathBuf, pressure: Arc<dyn Fn() -> Pressure + Send + Sync>, panel_open: Arc<dyn Fn() -> bool + Send + Sync>) -> Scheduler {
        let cache = load_cache(&cache_path);
        let state = Arc::new(Mutex::new(State {
            items: cache.items,
            recent: cache.recent,
            recent_at_ms: cache.recent_at_ms,
            recent_skipped_dirs: cache.recent_skipped_dirs,
            recent_denied: cache.recent_denied,
            denied_roots: cache.denied_roots,
            ..State::default()
        }));
        let (tx, rx) = channel::<Job>();
        let worker_state = state.clone();
        let worker_pressure = pressure.clone();
        let scan_folders = Arc::new(Mutex::new(paths.scan_folders.clone()));
        let worker_folders = scan_folders.clone();
        std::thread::Builder::new()
            .name("pantau-storage".into())
            .spawn(move || {
                lower_this_thread_priority();
                for mut job in rx {
                    loop {
                        let paths = Paths { scan_folders: worker_folders.lock().unwrap_or_else(|e| e.into_inner()).clone(), ..paths.clone() };
                        // An automatic job gives up as soon as pressure turns Critical or the panel closes.
                        // A manual one ignores both, and a request can upgrade a running job: so the flag is
                        // read again after the checks, which may have waited.
                        let calm = || worker_pressure() != Pressure::Critical;
                        let is_manual = |measure: bool| {
                            let st = lock(&worker_state);
                            if measure { st.measure_manual } else { st.scan_manual }
                        };
                        let go = |measure: bool| is_manual(measure) || (calm() && panel_open()) || is_manual(measure);
                        let _ = catch_unwind(AssertUnwindSafe(|| match &job {
                            Job::Measure => run_measure(&worker_state, &paths, &cache_path, &|| go(true)),
                            Job::Recent => run_recent(&worker_state, &paths, &cache_path, &|| go(false)),
                            // The owner's click asked for this one, so a closed panel does not stop it.
                            Job::MeasureItem(id) => run_measure_item(&worker_state, id, &cache_path, &calm),
                            #[cfg(test)]
                            Job::Panic => panic!("test job failure"),
                        }));
                        let mut st = lock(&worker_state);
                        if matches!(job, Job::Measure | Job::MeasureItem(_)) {
                            if std::mem::take(&mut st.measure_again) {
                                job = Job::Measure;
                                continue;
                            }
                            if let Some(id) = st.remeasure.pop() {
                                job = Job::MeasureItem(id);
                                continue;
                            }
                        }
                        match job {
                            Job::Measure | Job::MeasureItem(_) => st.measuring = false,
                            Job::Recent => st.scanning = false,
                            #[cfg(test)]
                            Job::Panic => {}
                        }
                        break;
                    }
                }
            })
            .expect("spawn storage worker");
        Scheduler { state, pressure, tx, scan_folders }
    }

    /// Panel tab opened (`manual = false`) or the user asked (`true`). False if nothing was queued.
    /// A manual request during a running measure is kept and runs again after it.
    pub fn request_measure(&self, manual: bool) -> bool {
        self.request(Job::Measure, manual)
    }

    /// Daily check (`manual = false`) or the user asked (`true`). False if nothing was queued.
    /// A manual request during a running scan makes that scan the owner's, so it outlives the panel.
    pub fn request_recent_scan(&self, manual: bool) -> bool {
        self.request(Job::Recent, manual)
    }

    fn request(&self, job: Job, manual: bool) -> bool {
        let (now, pressure) = (now_ms(), (self.pressure)());
        let mut st = lock(&self.state);
        let is_measure = matches!(job, Job::Measure);
        let (busy, ok) = match job {
            Job::Measure => (st.measuring, should_measure(last_measured(&st.items), now, pressure, manual)),
            Job::Recent => (st.scanning, should_scan_recent(st.recent_at_ms, now, pressure, manual)),
            Job::MeasureItem(_) => unreachable!(),
            #[cfg(test)]
            Job::Panic => unreachable!(),
        };
        if busy && manual {
            if is_measure {
                st.measure_manual = true;
                st.measure_again = true;
            } else {
                st.scan_manual = true;
            }
            return true;
        }
        if busy || !ok {
            return false;
        }
        if is_measure {
            (st.measuring, st.measure_manual) = (true, manual);
        } else {
            (st.scanning, st.scan_manual) = (true, manual);
        }
        if self.tx.send(job).is_err() {
            if is_measure { st.measuring = false } else { st.scanning = false }
            return false;
        }
        true
    }

    /// The owner cleaned this item: measure it alone, after whatever is running now.
    pub fn request_measure_item(&self, id: &str) {
        let mut st = lock(&self.state);
        if st.measuring {
            st.remeasure.push(id.to_string());
            return;
        }
        st.measuring = true;
        if self.tx.send(Job::MeasureItem(id.to_string())).is_err() {
            st.measuring = false;
        }
    }

    /// Replaces the project folders and measures again, so the list follows the change at once.
    pub fn set_scan_folders(&self, folders: Vec<PathBuf>) {
        *self.scan_folders.lock().unwrap_or_else(|e| e.into_inner()) = folders;
        self.request_measure(true);
    }

    pub fn snapshot(&self) -> Snapshot {
        let paused = (self.pressure)() == Pressure::Critical;
        let st = lock(&self.state);
        Snapshot { items: st.items.clone(), recent: st.recent.clone(), recent_at_ms: st.recent_at_ms, recent_skipped_dirs: st.recent_skipped_dirs, recent_denied: st.recent_denied, denied_roots: st.denied_roots.clone(), paused, measuring: st.measuring, scanning: st.scanning, measuring_auto: st.measuring && !st.measure_manual, scanning_auto: st.scanning && !st.scan_manual }
    }
}

/// The oldest reading decides whether data is stale; any unread item makes it stale now.
fn last_measured(items: &[StorageItem]) -> Option<u64> {
    items.iter().map(|i| i.measured_at_ms).try_fold(u64::MAX, |m, t| t.map(|t| m.min(t))).filter(|_| !items.is_empty())
}

fn save(st: &State, cache_path: &Path) {
    let cache = StorageCache { items: st.items.clone(), recent: st.recent.clone(), recent_at_ms: st.recent_at_ms, recent_skipped_dirs: st.recent_skipped_dirs, recent_denied: st.recent_denied, denied_roots: st.denied_roots.clone() };
    let _ = save_cache(cache_path, &cache);
}

/// A path that does not exist is dropped from the list; a denied one is `Denied` with no size, never zero.
fn record(st: &mut State, id: &str, result: Measured) {
    let Some(pos) = st.items.iter().position(|i| i.id == id) else { return };
    match result {
        Measured::Missing => {
            st.items.remove(pos);
        }
        Measured::Bytes(b) => {
            let it = &mut st.items[pos];
            (it.bytes, it.measured_at_ms, it.state) = (Some(b), Some(now_ms()), ItemState::Ready);
        }
        Measured::Denied => {
            let it = &mut st.items[pos];
            (it.bytes, it.measured_at_ms, it.state) = (None, Some(now_ms()), ItemState::Denied);
        }
    }
}

/// Each item is updated in shared state the moment its measure finishes. Stops, also inside a big
/// folder, when `should_continue` says no; what is done so far is cached. The panel is often open
/// for seconds only, so each pass starts with what has waited longest and the next one gets further.
fn run_measure(state: &Mutex<State>, paths: &Paths, cache_path: &Path, should_continue: &dyn Fn() -> bool) {
    if !should_continue() {
        return;
    }
    let denied = denied_roots(&paths.scan_folders);
    let fresh = storage_items(paths);
    let targets = {
        let mut st = lock(state);
        st.denied_roots = denied;
        st.items = merge_items(&st.items, fresh);
        let mut order: Vec<(Option<u64>, String, PathBuf)> = st.items.iter().map(|i| (i.measured_at_ms, i.id.clone(), i.path.clone())).collect();
        order.sort_by_key(|t| (st.stopped.contains(&t.1), t.0));
        order
    };
    for (_, id, path) in targets {
        if !should_continue() {
            break;
        }
        let result = measure_while(&path, should_continue);
        let mut st = lock(state);
        let Some(result) = result else {
            if !st.stopped.contains(&id) {
                st.stopped.push(id);
            }
            break;
        };
        st.stopped.retain(|s| *s != id);
        record(&mut st, &id, result);
    }
    save(&lock(state), cache_path);
}

/// One item the owner just cleaned. When `should_continue` stops it, the old size goes too: it
/// describes the folder from before the clean.
fn run_measure_item(state: &Mutex<State>, id: &str, cache_path: &Path, should_continue: &dyn Fn() -> bool) {
    let Some(path) = lock(state).items.iter().find(|i| i.id == id).map(|i| i.path.clone()) else { return };
    let result = if should_continue() { measure_while(&path, should_continue) } else { None };
    let mut st = lock(state);
    match result {
        Some(result) => record(&mut st, id, result),
        None if fs::symlink_metadata(&path).is_err_and(|e| e.kind() == io::ErrorKind::NotFound) => record(&mut st, id, Measured::Missing),
        None => {
            if let Some(it) = st.items.iter_mut().find(|i| i.id == id) {
                (it.bytes, it.measured_at_ms, it.state) = (None, None, ItemState::Pending);
            }
        }
    }
    save(&st, cache_path);
}

/// A scan that was stopped early changes nothing: not the list, not the time.
fn run_recent(state: &Mutex<State>, paths: &Paths, cache_path: &Path, should_continue: &dyn Fn() -> bool) {
    let scan = scan_recent(&paths.home, &default_excludes(&paths.home), SystemTime::now(), should_continue);
    if !scan.complete {
        return;
    }
    let mut st = lock(state);
    st.recent = scan.files;
    st.recent_skipped_dirs = scan.skipped_dirs;
    st.recent_denied = scan.root_denied;
    st.recent_at_ms = Some(now_ms());
    save(&st, cache_path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{Category, Cleaner, ItemState};

    const MIN: u64 = 60_000;
    const DAY: u64 = 24 * 60 * MIN;

    #[test]
    fn measuring_follows_the_10_minute_rule_and_pauses_only_at_critical() {
        assert!(!should_measure(Some(0), 9 * MIN, Pressure::Normal, false));
        assert!(should_measure(Some(0), 11 * MIN, Pressure::Warning, false));
        assert!(!should_measure(Some(0), 11 * MIN, Pressure::Critical, false));
        assert!(should_measure(Some(0), 1, Pressure::Critical, true));
        assert!(should_measure(None, 0, Pressure::Normal, false));
    }

    #[test]
    fn recent_scan_runs_at_most_daily_and_pauses_at_critical() {
        assert!(!should_scan_recent(Some(0), DAY - 1, Pressure::Normal, false));
        assert!(should_scan_recent(Some(0), DAY + 1, Pressure::Warning, false));
        assert!(!should_scan_recent(Some(0), DAY + 1, Pressure::Critical, false));
        assert!(should_scan_recent(Some(0), 1, Pressure::Critical, true));
        assert!(should_scan_recent(None, 0, Pressure::Normal, false));
    }

    fn cache() -> StorageCache {
        StorageCache {
            items: vec![StorageItem {
                id: "pnpm".into(),
                title: "Cache pnpm".into(),
                sub: "s".into(),
                path: "/x".into(),
                category: Category::Safe,
                cleaner: Cleaner::PnpmPrune,
                upper_bound: true,
                bytes: Some(5),
                measured_at_ms: Some(7),
                state: ItemState::Ready,
            }],
            recent: vec![RecentFile { path: "/a".into(), bytes: 1, changed_at_secs: 2, owner_guess: None }],
            recent_at_ms: Some(9),
            recent_skipped_dirs: 2,
            recent_denied: true,
            denied_roots: vec!["/made/up/Documents".into()],
        }
    }

    #[test]
    fn a_cache_written_before_the_status_fields_existed_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("cache.json");
        std::fs::write(&p, r#"{"items":[],"recent":[{"path":"/a","bytes":1,"changedAtSecs":2,"ownerGuess":null}],"recentAtMs":9}"#).unwrap();
        let c = load_cache(&p);
        assert_eq!((c.recent.len(), c.recent_at_ms, c.recent_skipped_dirs, c.recent_denied), (1, Some(9), 0, false));
        assert!(c.denied_roots.is_empty());
    }

    #[test]
    fn a_corrupt_cache_loads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("cache.json");
        std::fs::write(&p, "{").unwrap();
        assert_eq!(load_cache(&p), StorageCache::default());
        assert_eq!(load_cache(&dir.path().join("missing.json")), StorageCache::default());
    }

    #[test]
    fn a_saved_cache_round_trips_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub/cache.json");
        save_cache(&p, &cache()).unwrap();
        assert_eq!(load_cache(&p), cache());
        let names: Vec<_> = std::fs::read_dir(p.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["cache.json"]);
    }

    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn paths(root: &Path) -> Paths {
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        std::fs::create_dir_all(root.join("ptmp")).unwrap();
        Paths { home: root.to_path_buf(), tmpdir: root.join("tmp"), private_tmp: root.join("ptmp"), scan_folders: vec![root.join("Documents")] }
    }

    fn wait_idle(s: &Scheduler) -> Snapshot {
        let end = Instant::now() + Duration::from_secs(20);
        loop {
            let snap = s.snapshot();
            if !snap.measuring && !snap.scanning {
                return snap;
            }
            assert!(Instant::now() < end, "worker did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn normal() -> Arc<dyn Fn() -> Pressure + Send + Sync> {
        Arc::new(|| Pressure::Normal)
    }

    fn open() -> Arc<dyn Fn() -> bool + Send + Sync> {
        Arc::new(|| true)
    }

    fn closed() -> Arc<dyn Fn() -> bool + Send + Sync> {
        Arc::new(|| false)
    }

    /// The panel closes after the worker asked `n` times.
    fn closes_after(n: usize) -> Arc<dyn Fn() -> bool + Send + Sync> {
        let calls = Arc::new(AtomicUsize::new(0));
        Arc::new(move || calls.fetch_add(1, Ordering::SeqCst) < n)
    }

    #[test]
    fn a_measure_pass_fills_items_marks_denied_and_drops_missing() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        std::fs::create_dir_all(h.join("Library/pnpm/store")).unwrap();
        std::fs::write(h.join("Library/pnpm/store/f"), vec![1u8; 10_000]).unwrap();
        let locked = h.join(".claude/projects");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let cache = h.join("cache/storage.json");
        let s = Scheduler::start(paths(h), cache.clone(), normal(), open());
        assert!(s.request_measure(true));
        let snap = wait_idle(&s);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

        let get = |id: &str| snap.items.iter().find(|i| i.id == id);
        let pnpm = get("pnpm").unwrap();
        assert_eq!(pnpm.state, ItemState::Ready);
        assert!(pnpm.bytes.unwrap() >= 10_000 && pnpm.measured_at_ms.is_some());
        let denied = get("claude-history").unwrap();
        assert_eq!((denied.state.clone(), denied.bytes), (ItemState::Denied, None));
        assert!(get("cursor-db").is_none(), "a missing path is dropped");
        assert_eq!(load_cache(&cache).items, snap.items);
    }

    #[test]
    fn fresh_data_is_not_remeasured_and_critical_pauses_only_the_automatic_requests() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        let critical = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let c = critical.clone();
        let s = Scheduler::start(paths(h), h.join("c.json"), Arc::new(move || if c.load(std::sync::atomic::Ordering::SeqCst) { Pressure::Critical } else { Pressure::Normal }), open());
        assert!(s.snapshot().paused);
        assert!(!s.request_measure(false));
        assert!(!s.request_recent_scan(false));
        assert!(s.request_recent_scan(true));
        let snap = wait_idle(&s);
        assert!(snap.recent_at_ms.is_some());
        critical.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(!s.snapshot().paused);
        assert!(!s.request_recent_scan(false), "scanned moments ago");
        assert!(s.request_measure(false), "never measured");
    }

    #[test]
    fn a_manual_measure_requested_while_busy_runs_again_when_the_job_ends() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        let next = h.join("Documents/proj/.next");
        std::fs::create_dir_all(&next).unwrap();
        std::fs::write(next.join("a"), vec![1u8; 1_000]).unwrap();
        // The automatic pass asks `pressure` before each item, on the worker thread: hold it
        // after the first item (.next) is measured, so the pass is provably busy.
        let (calls, blocked, release) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (c, b, r) = (calls.clone(), blocked.clone(), release.clone());
        let pressure: Arc<dyn Fn() -> Pressure + Send + Sync> = Arc::new(move || {
            if std::thread::current().name() == Some("pantau-storage") && c.fetch_add(1, SeqCst) == 2 {
                b.store(true, SeqCst);
                while !r.load(SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            Pressure::Normal
        });
        let s = Scheduler::start(paths(h), h.join("c.json"), pressure, open());
        assert!(s.request_measure(false));
        while !blocked.load(SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let size_of = |s: &Scheduler| s.snapshot().items.iter().find(|i| i.id.starts_with("next-")).unwrap().bytes.unwrap();
        let first = size_of(&s);
        // The owner cleans/changes .next after it was measured, then asks for a fresh measure.
        std::fs::write(next.join("b"), vec![1u8; 50_000]).unwrap();
        assert!(s.request_measure(true), "a manual request while busy is kept, not dropped");
        release.store(true, SeqCst);
        wait_idle(&s);
        assert!(size_of(&s) >= first + 50_000, "the repeat pass must replace the old number");
    }

    #[test]
    fn changing_the_scan_folders_remeasures_with_the_new_roots_and_empty_means_none() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        std::fs::create_dir_all(h.join("Documents/old/.next")).unwrap();
        std::fs::create_dir_all(h.join("Work/new/.next")).unwrap();
        let s = Scheduler::start(paths(h), h.join("c.json"), normal(), open());
        assert!(s.request_measure(true));
        let next_ids = |s: &Scheduler| -> Vec<String> { s.snapshot().items.into_iter().map(|i| i.id).filter(|id| id.starts_with("next-")).collect() };
        assert_eq!(wait_idle(&s).items.iter().filter(|i| i.id.starts_with("next-")).count(), 1);
        assert!(next_ids(&s)[0].ends_with("Documents/old/.next"));

        s.set_scan_folders(vec![h.join("Work")]);
        wait_idle(&s);
        let ids = next_ids(&s);
        assert_eq!(ids.len(), 1, "{ids:?}");
        assert!(ids[0].ends_with("Work/new/.next"), "{ids:?}");

        s.set_scan_folders(vec![]);
        wait_idle(&s);
        assert!(next_ids(&s).is_empty());
    }

    #[test]
    fn merging_keeps_the_old_reading_until_the_new_one_arrives() {
        let old = cache().items;
        let mut fresh = old.clone();
        fresh[0].bytes = None;
        fresh[0].measured_at_ms = None;
        fresh[0].state = ItemState::Pending;
        let merged = merge_items(&old, fresh);
        assert_eq!((merged[0].bytes, merged[0].measured_at_ms, merged[0].state.clone()), (Some(5), Some(7), ItemState::Ready));
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Normal for the first `n` calls, Critical after. Models pressure rising mid-job.
    fn critical_after(n: usize) -> Arc<dyn Fn() -> Pressure + Send + Sync> {
        let calls = Arc::new(AtomicUsize::new(0));
        Arc::new(move || if calls.fetch_add(1, Ordering::SeqCst) < n { Pressure::Normal } else { Pressure::Critical })
    }

    /// Like `critical_after`, but only the worker's own checks are counted, so the test thread's
    /// `request` and the `snapshot` polls of `wait_idle` cannot move the flip. Those calls just
    /// report the state the worker has reached.
    fn worker_critical_after(n: usize) -> Arc<dyn Fn() -> Pressure + Send + Sync> {
        let calls = Arc::new(AtomicUsize::new(0));
        Arc::new(move || {
            let on_worker = std::thread::current().name() == Some("pantau-storage");
            let seen = if on_worker { calls.fetch_add(1, Ordering::SeqCst) } else { calls.load(Ordering::SeqCst) };
            if seen < n { Pressure::Normal } else { Pressure::Critical }
        })
    }

    fn dirs_for_measuring(h: &Path) {
        for d in ["Library/pnpm/store", ".local/share/claude/versions", ".claude/projects"] {
            std::fs::create_dir_all(h.join(d)).unwrap();
            std::fs::write(h.join(d).join("f"), vec![1u8; 5_000]).unwrap();
        }
    }

    #[test]
    fn an_automatic_measure_stops_at_critical_and_caches_the_partial_pass_but_a_manual_one_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        dirs_for_measuring(h);
        let cache = h.join("c/s.json");
        // The worker checks once at the start and once before each item: pnpm gets through, then Critical.
        let s = Scheduler::start(paths(h), cache.clone(), worker_critical_after(2), open());
        assert!(s.request_measure(false));
        let snap = wait_idle(&s);
        let state_of = |id: &str| snap.items.iter().find(|i| i.id == id).unwrap().state.clone();
        assert_eq!(state_of("pnpm"), ItemState::Ready);
        assert_eq!(state_of("claude-history"), ItemState::Pending);
        assert_eq!(load_cache(&cache).items, snap.items, "partial result is cached");

        let dir = tempfile::tempdir().unwrap();
        dirs_for_measuring(dir.path());
        let s = Scheduler::start(paths(dir.path()), dir.path().join("c.json"), critical_after(0), open());
        assert!(s.request_measure(true));
        let snap = wait_idle(&s);
        assert!(snap.items.iter().all(|i| i.state != ItemState::Pending), "{:?}", snap.items.iter().map(|i| (&i.id, &i.state)).collect::<Vec<_>>());
    }

    fn seeded_cache(path: &Path) {
        let c = StorageCache { recent: vec![RecentFile { path: "/old".into(), bytes: 1, changed_at_secs: 1, owner_guess: None }], recent_at_ms: Some(1), ..StorageCache::default() };
        save_cache(path, &c).unwrap();
    }

    #[test]
    fn a_recent_scan_stopped_by_critical_keeps_the_previous_list_and_time() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        std::fs::create_dir_all(h.join("a")).unwrap();
        let cache = h.join("c/s.json");
        seeded_cache(&cache);
        let s = Scheduler::start(paths(h), cache.clone(), critical_after(1), open());
        assert!(s.request_recent_scan(false));
        let snap = wait_idle(&s);
        assert_eq!((snap.recent_at_ms, snap.recent.len()), (Some(1), 1));
        assert_eq!(load_cache(&cache).recent_at_ms, Some(1));

        let cache = h.join("c/s2.json");
        seeded_cache(&cache);
        let s = Scheduler::start(paths(h), cache, critical_after(0), open());
        assert!(s.request_recent_scan(true));
        let snap = wait_idle(&s);
        assert!(snap.recent_at_ms.unwrap() > 1);
        assert!(snap.recent.is_empty());
    }

    #[test]
    fn a_recent_scan_reports_folders_it_could_not_read() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        std::fs::create_dir_all(h.join("locked")).unwrap();
        std::fs::set_permissions(h.join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let s = Scheduler::start(paths(h), h.join("c.json"), normal(), open());
        assert!(s.request_recent_scan(true));
        let snap = wait_idle(&s);
        std::fs::set_permissions(h.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!((snap.recent_skipped_dirs, snap.recent_denied), (1, false));
    }

    // panel closed (I3)

    #[test]
    fn an_automatic_measure_stops_when_the_panel_closes_and_keeps_what_it_finished() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        dirs_for_measuring(h);
        let cache = h.join("c/s.json");
        let s = Scheduler::start(paths(h), cache.clone(), normal(), closes_after(2));
        assert!(s.request_measure(false));
        let snap = wait_idle(&s);
        let state_of = |id: &str| snap.items.iter().find(|i| i.id == id).unwrap().state.clone();
        assert_eq!(state_of("pnpm"), ItemState::Ready);
        assert_eq!(state_of("claude-history"), ItemState::Pending);
        assert_eq!(load_cache(&cache).items, snap.items, "partial result is cached");
    }

    #[test]
    fn a_manual_measure_finishes_with_the_panel_closed() {
        let dir = tempfile::tempdir().unwrap();
        dirs_for_measuring(dir.path());
        let s = Scheduler::start(paths(dir.path()), dir.path().join("c.json"), normal(), closed());
        assert!(s.request_measure(true));
        assert!(wait_idle(&s).items.iter().all(|i| i.state != ItemState::Pending));
    }

    #[test]
    fn an_automatic_recent_scan_stops_when_the_panel_closes_but_a_manual_one_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        std::fs::create_dir_all(h.join("a")).unwrap();
        let cache = h.join("c/s.json");
        seeded_cache(&cache);
        let s = Scheduler::start(paths(h), cache.clone(), normal(), closed());
        assert!(s.request_recent_scan(false));
        assert_eq!(wait_idle(&s).recent_at_ms, Some(1), "stopped: the old list and time stay");
        assert!(s.request_recent_scan(true));
        assert!(wait_idle(&s).recent_at_ms.unwrap() > 1);
    }

    #[test]
    fn short_panel_openings_still_reach_every_item() {
        // The panel is often open for seconds, so a new pass starts with what it never read.
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        dirs_for_measuring(h);
        let cache = h.join("c/s.json");
        let s = Scheduler::start(paths(h), cache.clone(), normal(), closes_after(3));
        assert!(s.request_measure(false));
        let first = wait_idle(&s);
        let ready = |snap: &Snapshot| snap.items.iter().filter(|i| i.state == ItemState::Ready).count();
        assert_eq!(ready(&first), 2);
        let s = Scheduler::start(paths(h), cache, normal(), closes_after(2));
        assert!(s.request_measure(false));
        assert_eq!(ready(&wait_idle(&s)), 3, "the second opening must read a new item, not the first one again");
    }

    // inside a big folder (I4)

    #[test]
    fn an_automatic_measure_stops_inside_a_big_folder() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        std::fs::create_dir_all(h.join("Library/pnpm/store")).unwrap();
        for i in 0..1100 {
            std::fs::write(h.join("Library/pnpm/store").join(format!("f{i}")), b"x").unwrap();
        }
        let s = Scheduler::start(paths(h), h.join("c.json"), worker_critical_after(2), open());
        assert!(s.request_measure(false));
        let pnpm = wait_idle(&s).items.into_iter().find(|i| i.id == "pnpm").unwrap();
        assert_eq!((pnpm.state, pnpm.bytes), (ItemState::Pending, None));
    }

    // one cleaned item (I4)

    fn measured_all(h: &Path) -> Scheduler {
        dirs_for_measuring(h);
        let s = Scheduler::start(paths(h), h.join("c.json"), normal(), open());
        assert!(s.request_measure(true));
        wait_idle(&s);
        s
    }

    fn get(s: &Scheduler, id: &str) -> Option<StorageItem> {
        s.snapshot().items.into_iter().find(|i| i.id == id)
    }

    #[test]
    fn remeasuring_a_cleaned_item_reads_only_that_item() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        let s = measured_all(h);
        let history = get(&s, "claude-history").unwrap();
        std::fs::write(h.join(".claude/projects/more"), vec![1u8; 50_000]).unwrap();
        let before = get(&s, "pnpm").unwrap().bytes.unwrap();
        std::fs::write(h.join("Library/pnpm/store/more"), vec![1u8; 50_000]).unwrap();
        s.request_measure_item("pnpm");
        wait_idle(&s);
        assert!(get(&s, "pnpm").unwrap().bytes.unwrap() >= before + 50_000);
        assert_eq!(get(&s, "claude-history").unwrap(), history, "other items are not read");
    }

    #[test]
    fn a_cleaned_item_that_is_gone_leaves_the_list_and_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        let s = measured_all(h);
        std::fs::remove_dir_all(h.join("Library/pnpm/store")).unwrap();
        s.request_measure_item("pnpm");
        wait_idle(&s);
        assert!(get(&s, "pnpm").is_none());
        assert!(load_cache(&h.join("c.json")).items.iter().all(|i| i.id != "pnpm"));
    }

    #[test]
    fn a_cleaned_item_is_remeasured_with_the_panel_closed() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        dirs_for_measuring(h);
        let cache = h.join("c.json");
        let first = Scheduler::start(paths(h), cache.clone(), normal(), open());
        assert!(first.request_measure(true));
        wait_idle(&first);
        let before = get(&first, "pnpm").unwrap().bytes.unwrap();
        std::fs::write(h.join("Library/pnpm/store/more"), vec![1u8; 50_000]).unwrap();
        let s = Scheduler::start(paths(h), cache, normal(), closed());
        s.request_measure_item("pnpm");
        wait_idle(&s);
        assert!(get(&s, "pnpm").unwrap().bytes.unwrap() >= before + 50_000);
    }

    #[test]
    fn at_critical_a_cleaned_item_is_not_walked_and_its_old_size_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        dirs_for_measuring(h);
        let cache = h.join("c.json");
        let first = Scheduler::start(paths(h), cache.clone(), normal(), open());
        assert!(first.request_measure(true));
        wait_idle(&first);
        // The size from before the clean no longer describes the folder.
        let s = Scheduler::start(paths(h), cache, Arc::new(|| Pressure::Critical), open());
        s.request_measure_item("pnpm");
        let snap = wait_idle(&s);
        let pnpm = snap.items.iter().find(|i| i.id == "pnpm").unwrap();
        assert_eq!((pnpm.bytes, pnpm.measured_at_ms, pnpm.state.clone()), (None, None, ItemState::Pending));
        let history = snap.items.iter().find(|i| i.id == "claude-history").unwrap();
        assert_eq!(history.state, ItemState::Ready);
    }

    #[test]
    fn a_clean_that_ends_during_a_pass_is_measured_after_it() {
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        dirs_for_measuring(h);
        let (blocked, release) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (b, r) = (blocked.clone(), release.clone());
        let calls = Arc::new(AtomicUsize::new(0));
        // Holds the pass after pnpm was read.
        let pressure: Arc<dyn Fn() -> Pressure + Send + Sync> = Arc::new(move || {
            if std::thread::current().name() == Some("pantau-storage") && calls.fetch_add(1, SeqCst) == 2 {
                b.store(true, SeqCst);
                while !r.load(SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            Pressure::Normal
        });
        let s = Scheduler::start(paths(h), h.join("c.json"), pressure, open());
        assert!(s.request_measure(false));
        while !blocked.load(SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let before = get(&s, "pnpm").unwrap().bytes.unwrap();
        std::fs::write(h.join("Library/pnpm/store/more"), vec![1u8; 50_000]).unwrap();
        s.request_measure_item("pnpm");
        release.store(true, SeqCst);
        wait_idle(&s);
        assert!(get(&s, "pnpm").unwrap().bytes.unwrap() >= before + 50_000);
    }

    // status that must survive a restart (I2)

    #[test]
    fn a_scan_root_that_cannot_be_listed_is_reported_and_survives_a_restart() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        let docs = h.join("Documents");
        std::fs::create_dir_all(docs.join("web/.next")).unwrap();
        std::fs::set_permissions(&docs, std::fs::Permissions::from_mode(0o000)).unwrap();
        let cache = h.join("c.json");
        let s = Scheduler::start(paths(h), cache.clone(), normal(), open());
        assert!(s.request_measure(true));
        let snap = wait_idle(&s);
        assert_eq!(snap.denied_roots, vec![docs.clone()]);
        assert!(snap.items.iter().all(|i| !i.id.starts_with("next-")), "this is why the status is needed");
        let again = Scheduler::start(paths(h), cache, normal(), open()).snapshot();
        assert_eq!(again.denied_roots, vec![docs.clone()], "read from the cache at startup");

        // Permission granted: the next pass takes the note away.
        std::fs::set_permissions(&docs, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(s.request_measure(true));
        let snap = wait_idle(&s);
        assert!(snap.denied_roots.is_empty());
        assert!(snap.items.iter().any(|i| i.id.starts_with("next-")));
    }

    #[test]
    fn what_a_recent_scan_could_not_read_survives_a_restart() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        std::fs::create_dir_all(h.join("locked")).unwrap();
        std::fs::set_permissions(h.join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let cache = h.join("c.json");
        let s = Scheduler::start(paths(h), cache.clone(), normal(), open());
        assert!(s.request_recent_scan(true));
        wait_idle(&s);
        std::fs::set_permissions(h.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let again = Scheduler::start(paths(h), cache, normal(), open()).snapshot();
        assert_eq!((again.recent_skipped_dirs, again.recent_denied), (1, false));
    }

    // review fixes: the owner's click must not be lost to a closing panel (A1, A2)

    /// The worker's checks wait here from the `at`-th one until `release` is set.
    fn held_at(at: usize) -> (Arc<dyn Fn() -> Pressure + Send + Sync>, Arc<AtomicBool>, Arc<AtomicBool>) {
        let (calls, blocked, release) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (b, r) = (blocked.clone(), release.clone());
        let pressure: Arc<dyn Fn() -> Pressure + Send + Sync> = Arc::new(move || {
            if std::thread::current().name() == Some("pantau-storage") && calls.fetch_add(1, Ordering::SeqCst) == at {
                b.store(true, Ordering::SeqCst);
                while !r.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            Pressure::Normal
        });
        (pressure, blocked, release)
    }

    fn wait_for(flag: &AtomicBool) {
        let end = Instant::now() + Duration::from_secs(20);
        while !flag.load(Ordering::SeqCst) {
            assert!(Instant::now() < end, "flag never set");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    use std::sync::atomic::AtomicBool;

    #[test]
    fn a_manual_scan_request_during_an_automatic_scan_makes_it_survive_the_panel_closing() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        std::fs::create_dir_all(h.join("a")).unwrap();
        let (pressure, blocked, release) = held_at(0);
        let panel_open = Arc::new(AtomicBool::new(true));
        let po = panel_open.clone();
        let s = Scheduler::start(paths(h), h.join("c.json"), pressure, Arc::new(move || po.load(Ordering::SeqCst)));
        assert!(s.request_recent_scan(false));
        wait_for(&blocked);
        let snap = s.snapshot();
        assert_eq!((snap.scanning, snap.scanning_auto), (true, true), "the panel needs to know the button may be used");
        panel_open.store(false, Ordering::SeqCst);
        assert!(s.request_recent_scan(true), "kept, not dropped, while the automatic scan runs");
        assert!(!s.snapshot().scanning_auto);
        release.store(true, Ordering::SeqCst);
        let snap = wait_idle(&s);
        assert!(snap.recent_at_ms.is_some(), "the scan the owner asked for finished with the panel closed");
    }

    #[test]
    fn an_automatic_request_during_a_running_scan_is_still_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (pressure, blocked, release) = held_at(0);
        let s = Scheduler::start(paths(dir.path()), dir.path().join("c.json"), pressure, open());
        assert!(s.request_recent_scan(false));
        wait_for(&blocked);
        assert!(!s.request_recent_scan(false));
        release.store(true, Ordering::SeqCst);
        wait_idle(&s);
    }

    #[test]
    fn a_manual_measure_request_during_an_automatic_pass_upgrades_it() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        dirs_for_measuring(h);
        let (pressure, blocked, release) = held_at(1);
        let panel_open = Arc::new(AtomicBool::new(true));
        let po = panel_open.clone();
        let s = Scheduler::start(paths(h), h.join("c.json"), pressure, Arc::new(move || po.load(Ordering::SeqCst)));
        assert!(s.request_measure(false));
        wait_for(&blocked);
        assert!(s.snapshot().measuring_auto);
        panel_open.store(false, Ordering::SeqCst);
        assert!(s.request_measure(true));
        assert!(!s.snapshot().measuring_auto, "the button for this is usable and the pass is the owner's now");
        release.store(true, Ordering::SeqCst);
        assert!(wait_idle(&s).items.iter().all(|i| i.state != ItemState::Pending));
    }

    #[test]
    fn an_item_whose_walk_was_stopped_goes_last_on_the_next_automatic_pass() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        std::fs::create_dir_all(h.join("Library/pnpm/store")).unwrap();
        for i in 0..1100 {
            std::fs::write(h.join("Library/pnpm/store").join(format!("f{i}")), b"x").unwrap();
        }
        // The panel allows this many checks, then reports closed.
        let budget = Arc::new(AtomicUsize::new(0));
        let b = budget.clone();
        let panel_open: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || b.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok());
        let s = Scheduler::start(paths(h), h.join("c.json"), normal(), panel_open);
        let state_of = |s: &Scheduler, id: &str| s.snapshot().items.into_iter().find(|i| i.id == id).unwrap().state;
        // start check, the check before pnpm, then the check inside pnpm's walk says stop
        budget.store(2, Ordering::SeqCst);
        assert!(s.request_measure(false));
        wait_idle(&s);
        assert_eq!(state_of(&s, "pnpm"), ItemState::Pending);
        // Without the fix pnpm is oldest again and eats the one item this opening can afford.
        budget.store(2, Ordering::SeqCst);
        assert!(s.request_measure(false));
        wait_idle(&s);
        assert_eq!(state_of(&s, "temp"), ItemState::Ready, "a small item got through ahead of the big one");
        assert_eq!(state_of(&s, "pnpm"), ItemState::Pending);
        // The owner asks: the big one is measured and is no longer remembered as stopped.
        assert!(s.request_measure(true));
        wait_idle(&s);
        assert_eq!(state_of(&s, "pnpm"), ItemState::Ready);
    }

    #[test]
    fn a_closed_panel_stops_an_automatic_pass_before_it_reads_the_rule_table() {
        let dir = tempfile::tempdir().unwrap();
        let s = Scheduler::start(paths(dir.path()), dir.path().join("c.json"), normal(), closed());
        assert!(s.request_measure(false));
        let snap = wait_idle(&s);
        assert!(snap.items.is_empty(), "no rows were built: {:?}", snap.items.iter().map(|i| &i.id).collect::<Vec<_>>());
    }

    #[test]
    fn a_cleaned_item_whose_folder_is_gone_leaves_the_list_even_when_the_measure_is_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let h = dir.path();
        let first = Scheduler::start(paths(h), h.join("c.json"), normal(), open());
        dirs_for_measuring(h);
        assert!(first.request_measure(true));
        wait_idle(&first);
        std::fs::remove_dir_all(h.join("Library/pnpm/store")).unwrap();
        let s = Scheduler::start(paths(h), h.join("c.json"), Arc::new(|| Pressure::Critical), open());
        s.request_measure_item("pnpm");
        let snap = wait_idle(&s);
        assert!(snap.items.iter().all(|i| i.id != "pnpm"), "not left as 'Belum diukur'");
    }

    #[test]
    fn a_cache_with_a_relative_path_is_dropped_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("cache.json");
        let mut c = cache();
        let mut bad = c.items[0].clone();
        (bad.id, bad.path) = ("evil".into(), "-a/Applications".into());
        c.items.push(bad);
        c.recent.push(RecentFile { path: "--help".into(), bytes: 1, changed_at_secs: 2, owner_guess: None });
        save_cache(&p, &c).unwrap();
        let loaded = load_cache(&p);
        assert_eq!(loaded.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["pnpm"]);
        assert_eq!(loaded.recent.len(), 1);
    }

    #[test]
    fn a_panicking_job_does_not_stop_the_worker() {
        let dir = tempfile::tempdir().unwrap();
        let s = Scheduler::start(paths(dir.path()), dir.path().join("c.json"), normal(), open());
        s.tx.send(Job::Panic).unwrap();
        assert!(s.request_measure(true));
        let snap = wait_idle(&s);
        assert!(snap.items.iter().any(|i| i.id == "temp" && i.state == ItemState::Ready));
    }
}
