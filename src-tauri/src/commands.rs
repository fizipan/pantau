//! The Tauri commands: the only path from a webview click to a kill or a delete. The webview
//! sends ids and flags, never paths, and every safety rule is enforced here on fresh data.

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::errors::{os_error, other_error};
use crate::panel;
use crate::process::Snapshot;
use crate::process::group::{ProcessGroup, project_name};
use crate::process::kill::{self, KillOutcome, KillPlan, ProcId, TERM_WAIT, authorize, read_start, send_force, send_term, wait_exit};
use crate::process::leftover::Leftover;
use crate::process::ports::{run_with_timeout, spawn_with_timeout};
use crate::process::raw::{ProcessReader, RawProcess};
use crate::sampler;
use crate::settings::{self, Settings};
use crate::shortcut;
use crate::storage::clean::{CleanContext, CleanResult, clean, read_open_paths, refusal, run_pnpm_at};
use crate::storage::scheduler::{Scheduler, Snapshot as StorageData};
use crate::storage::{Cleaner, ItemState, Paths, StorageItem};
use crate::system::SystemSample;

/// Finished operations stay this long so a panel opened later still shows the result.
const KEEP_FINISHED: Duration = Duration::from_secs(60);
/// How long a forced kill is awaited; SIGKILL cannot be ignored, so this is generous.
const FORCE_WAIT: Duration = Duration::from_secs(3);
/// Longer than `PATH_MAX`, so `clean:<item id>` fits for any path.
const MAX_OP_ID: usize = 4096;
const ICON_SIZE: &str = "64";
const TOOL_TIMEOUT: Duration = Duration::from_secs(5);
const PRIVACY_PANE: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------- data contract

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum OpState {
    Running,
    /// `freed_bytes` is `None` for a kill. `skipped` counts entries a cleanup offered but did not remove.
    Done { freed_bytes: Option<u64>, skipped: usize },
    StillAlive,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Operation {
    pub op_id: String,
    pub state: OpState,
    /// Targets a finished kill sent TERM to that are still alive: the only ones `force_kill` may reach.
    pub survivors: Vec<ProcId>,
}

/// What the panel gets when it opens, before the first `snapshot` event arrives.
#[derive(Debug, Clone, Serialize)]
pub struct PanelData {
    pub history: Vec<SystemSample>,
    pub groups: Vec<ProcessGroup>,
    pub leftovers: Vec<Leftover>,
    pub operations: Vec<Operation>,
    /// CPU numbers in `groups` are not real yet; show them as pending, not 0.
    pub cpu_pending: bool,
}

pub fn panel_data(snapshot: Option<Snapshot>, history: Vec<SystemSample>, operations: Vec<Operation>) -> PanelData {
    let (groups, leftovers, cpu_pending) = snapshot.map(|s| (s.groups, s.leftovers, s.cpu_pending)).unwrap_or_default();
    PanelData { history, groups, leftovers, operations, cpu_pending }
}

// ---------------------------------------------------------------- operations

struct Entry {
    op: Operation,
    finished: Option<Instant>,
}

#[derive(Default)]
struct Inner {
    ops: Mutex<Vec<Entry>>,
    /// Item ids with a cleanup in flight; one cleanup per item at a time.
    cleaning: Mutex<HashSet<String>>,
}

/// Operations live in Rust state, not in the webview, so they survive the panel closing.
#[derive(Clone, Default)]
pub struct OpStore(Arc<Inner>);

/// Holds an item's cleanup slot; dropping it, even by panic, frees the slot.
pub struct ItemGuard {
    inner: Arc<Inner>,
    id: String,
}

impl Drop for ItemGuard {
    fn drop(&mut self) {
        lock(&self.inner.cleaning).remove(&self.id);
    }
}

impl OpStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `op_id` as running. A finished op with the same id is replaced (a retry).
    pub fn begin(&self, op_id: &str) -> Result<(), String> {
        if op_id.is_empty() || op_id.len() > MAX_OP_ID {
            return Err("ID operasi tidak valid".into());
        }
        self.prune_at(Instant::now());
        let mut all = lock(&self.0.ops);
        match all.iter().position(|e| e.op.op_id == op_id) {
            Some(i) if all[i].finished.is_none() => return Err("Operasi ini sedang berjalan".into()),
            Some(i) => {
                all.remove(i);
            }
            None => {}
        }
        all.push(Entry { op: Operation { op_id: op_id.into(), state: OpState::Running, survivors: Vec::new() }, finished: None });
        Ok(())
    }

    pub fn finish(&self, op_id: &str, state: OpState, survivors: Vec<ProcId>) {
        if let Some(e) = lock(&self.0.ops).iter_mut().find(|e| e.op.op_id == op_id) {
            e.op.state = state;
            e.op.survivors = survivors;
            e.finished = Some(Instant::now());
        }
    }

    /// The survivors of a finished op, `None` for an unknown or still-running one.
    fn survivors_of(&self, op_id: &str) -> Option<Vec<ProcId>> {
        lock(&self.0.ops).iter().find(|e| e.op.op_id == op_id && e.finished.is_some()).map(|e| e.op.survivors.clone())
    }

    fn claim_item(&self, id: &str) -> Result<ItemGuard, String> {
        if !lock(&self.0.cleaning).insert(id.to_string()) {
            return Err("Item ini sedang dibersihkan".into());
        }
        Ok(ItemGuard { inner: self.0.clone(), id: id.to_string() })
    }

    pub fn prune_at(&self, now: Instant) {
        lock(&self.0.ops).retain(|e| e.finished.is_none_or(|t| now.saturating_duration_since(t) <= KEEP_FINISHED));
    }

    pub fn operations(&self) -> Vec<Operation> {
        self.prune_at(Instant::now());
        lock(&self.0.ops).iter().map(|e| e.op.clone()).collect()
    }
}

/// Runs `job` on its own thread. A panic or a thread that cannot start still ends the op as `Failed`.
fn run_op(store: &OpStore, op_id: String, job: impl FnOnce() -> (OpState, Vec<ProcId>) + Send + 'static) -> Result<(), String> {
    let (s, id) = (store.clone(), op_id.clone());
    let spawned = std::thread::Builder::new().name("pantau-op".into()).spawn(move || {
        let (state, survivors) = catch_unwind(AssertUnwindSafe(job))
            .unwrap_or_else(|_| (OpState::Failed("Operasi berhenti karena kesalahan tak terduga".into()), Vec::new()));
        s.finish(&id, state, survivors);
    });
    spawned.map(drop).map_err(|e| {
        store.finish(&op_id, OpState::Failed("Operasi tidak bisa dimulai".into()), Vec::new());
        os_error("Operasi tidak bisa dimulai", &e)
    })
}

// ---------------------------------------------------------------- kill

/// The honest end state: how many targets failed and whether the rest actually stopped.
fn kill_state(total: usize, outcome: Option<KillOutcome>, failures: &[String]) -> OpState {
    match (failures.is_empty(), outcome) {
        (true, Some(KillOutcome::StillAlive)) => OpState::StillAlive,
        (true, _) => OpState::Done { freed_bytes: None, skipped: 0 },
        (false, _) if failures.len() >= total => OpState::Failed(format!("Gagal dihentikan: {}", failures.join("; "))),
        (false, outcome) => {
            let rest = if outcome == Some(KillOutcome::StillAlive) { "belum berhenti" } else { "sudah berhenti" };
            OpState::Failed(format!("{} dari {} gagal: {}. Sisanya {rest}.", failures.len(), total, failures.join("; ")))
        }
    }
}

/// Safe targets first: every target is authorized against `raw`, which the caller read just now.
/// Blocked targets are dropped and reported; the rest go on.
fn authorize_all(targets: &[ProcId], raw: &[RawProcess]) -> Result<(Vec<ProcId>, Vec<String>), String> {
    let me = unsafe { libc::getuid() };
    let (mut allowed, mut blocked) = (Vec::new(), Vec::new());
    for t in targets {
        // `authorize` lets an absent row through (gone or reused pid). A pid that is alive with
        // the target's start time but missing from the list means the list cannot be trusted.
        let listed = raw.iter().any(|p| p.pid == t.pid && p.start_time == t.start_time);
        if !listed && read_start(t.pid) == Some(t.start_time) {
            blocked.push(format!("PID {}: proses tidak ada di daftar terbaru, tidak dihentikan", t.pid));
            continue;
        }
        match authorize(t, raw, me, std::process::id()) {
            Ok(()) => allowed.push(*t),
            Err(why) => blocked.push(format!("PID {}: {why}", t.pid)),
        }
    }
    if allowed.is_empty() {
        return Err(if blocked.is_empty() { "Tidak ada proses yang dipilih".into() } else { blocked.join("; ") });
    }
    Ok((allowed, blocked))
}

enum Signal {
    Term,
    Force,
}

fn kill_core(store: &OpStore, raw: &[RawProcess], targets: Vec<ProcId>, op_id: String, signal: Signal, wait: Duration) -> Result<(), String> {
    let total = targets.len();
    let (allowed, mut failures) = authorize_all(&targets, raw)?;
    store.begin(&op_id)?;
    run_op(store, op_id, move || {
        let mut sent = Vec::new();
        for t in allowed {
            let result = match signal {
                Signal::Term => send_term(t, &read_start),
                Signal::Force => send_force(t, &read_start),
            };
            match result {
                Ok(()) => sent.push(t),
                // A refused signal on a pid that now belongs to another process (or none) means our
                // target is gone; only a still-matching process is a real failure.
                Err(why) if read_start(t.pid) == Some(t.start_time) => failures.push(format!("PID {}: {why}", t.pid)),
                Err(_) => {}
            }
        }
        let outcome = (!sent.is_empty()).then(|| wait_exit(&sent, &read_start, wait));
        let survivors = sent.into_iter().filter(|t| read_start(t.pid) == Some(t.start_time)).collect();
        (kill_state(total, outcome, &failures), survivors)
    })
}

/// `confirmed` is the owner's answer to `plan_kill`; without it a plan that needs confirmation sends nothing.
pub fn start_kill_with(store: &OpStore, raw: &[RawProcess], targets: Vec<ProcId>, op_id: String, confirmed: bool, wait: Duration) -> Result<(), String> {
    let (allowed, _) = authorize_all(&targets, raw)?;
    let plan = kill::plan_kill(&allowed, raw, &project_name);
    if plan.needs_confirm && !confirmed {
        return Err(format!("Perlu konfirmasi sebelum menghentikan. {}", plan.warnings.join("; ")).trim().to_string());
    }
    kill_core(store, raw, targets, op_id, Signal::Term, wait)
}

pub fn start_kill_inner(store: &OpStore, raw: &[RawProcess], targets: Vec<ProcId>, op_id: String, confirmed: bool) -> Result<(), String> {
    start_kill_with(store, raw, targets, op_id, confirmed, TERM_WAIT)
}

/// Only for targets that an earlier kill (`op_id`) sent TERM to and that are still alive.
pub fn force_kill_inner(store: &OpStore, raw: &[RawProcess], targets: Vec<ProcId>, op_id: String) -> Result<(), String> {
    authorize_all(&targets, raw)?;
    let survivors = store.survivors_of(&op_id).ok_or("Belum ada percobaan menghentikan proses ini, jadi tidak bisa dipaksa")?;
    if !targets.iter().all(|t| survivors.contains(t)) {
        return Err("Hanya proses yang sudah dicoba dihentikan dan masih hidup yang bisa dipaksa".into());
    }
    kill_core(store, raw, targets, op_id, Signal::Force, FORCE_WAIT)
}

// ---------------------------------------------------------------- clean

fn clean_state(result: Result<CleanResult, String>) -> OpState {
    match result {
        Ok(r) => OpState::Done { freed_bytes: Some(r.freed_bytes), skipped: r.skipped },
        Err(why) => OpState::Failed(why),
    }
}

/// One cleanup per item at a time. `after` runs once the cleanup ended (the command uses it to
/// ask for a new measurement). `build` runs on the worker thread and returns a fresh process list and the context, with
/// `now` set last, so nothing it reads is older than the click.
pub fn clean_item_inner(
    store: &OpStore,
    item: StorageItem,
    op_id: String,
    build: impl FnOnce() -> (Vec<RawProcess>, CleanContext) + Send + 'static,
    after: impl FnOnce() + Send + 'static,
) -> Result<(), String> {
    let guard = store.claim_item(&item.id)?;
    store.begin(&op_id)?;
    run_op(store, op_id, move || {
        let _guard = guard;
        let (processes, ctx) = build();
        let state = clean_state(clean(&item, &processes, &ctx));
        after();
        (state, Vec::new())
    })
}

fn find_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(name)).find(|p| fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

fn version_key(name: &str) -> Option<Vec<u64>> {
    name.strip_prefix('v')?.split('.').map(|p| p.parse().ok()).collect()
}

/// Where `pnpm` and `node` live when they are not on a GUI app's minimal `PATH`, in search order.
/// nvm versions come newest first; a volta install keeps its shims in `~/.volta/bin`.
fn tool_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![home.join("Library/pnpm"), "/opt/homebrew/bin".into(), "/usr/local/bin".into(), home.join(".local/bin"), home.join(".volta/bin")];
    let nvm = home.join(".nvm/versions/node");
    let mut versions: Vec<(Vec<u64>, PathBuf)> = fs::read_dir(&nvm)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((version_key(&e.file_name().to_string_lossy())?, e.path().join("bin"))))
        .collect();
    versions.sort_by(|a, b| b.0.cmp(&a.0));
    dirs.extend(versions.into_iter().map(|(_, p)| p));
    dirs
}

fn build_path(dirs: &[PathBuf], existing: &OsStr) -> OsString {
    let system = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(PathBuf::from);
    // A relative entry (or an empty one, which means the current folder) would run tools from wherever Pantau started.
    let parts = dirs.iter().cloned().chain(std::env::split_paths(existing).filter(|p| p.is_absolute())).chain(system);
    std::env::join_paths(parts).unwrap_or_else(|_| existing.to_os_string())
}

/// Where pnpm runs: `~/Library/pnpm` when it exists, else Pantau's own data folder (made if needed).
/// Both are the owner's, writable, and never a project, so no `.pnpmfile.cjs` is found there.
fn pnpm_cwd(home: &Path) -> PathBuf {
    let pnpm = home.join("Library/pnpm");
    if pnpm.is_dir() {
        return pnpm;
    }
    let own = home.join("Library/Application Support/local.toolslab.pantau");
    let _ = fs::create_dir_all(&own);
    own
}

fn real_pnpm(home: PathBuf) -> Box<dyn Fn() -> Result<(), String> + Send + Sync> {
    Box::new(move || {
        let dirs = tool_dirs(&home);
        let pnpm = find_in(&dirs, "pnpm").ok_or("pnpm tidak ditemukan di komputer ini")?;
        run_pnpm_at(&pnpm, &build_path(&dirs, &std::env::var_os("PATH").unwrap_or_default()), &pnpm_cwd(&home))
    })
}

fn disk_free() -> u64 {
    crate::system::read_system(&mut sysinfo::System::new()).disk_free
}

/// lsof is slow, so only the cleanups that read its answer run it.
fn needs_open_paths(cleaner: Cleaner) -> bool {
    matches!(cleaner, Cleaner::OldTemp | Cleaner::OldClaudeVersions)
}

fn real_context(item: &StorageItem) -> (Vec<RawProcess>, CleanContext) {
    let paths = Paths::current();
    let open_paths = if needs_open_paths(item.cleaner) { read_open_paths() } else { None };
    let processes = ProcessReader::new().read();
    let home = paths.home.clone();
    let ctx = CleanContext {
        paths,
        open_paths,
        now: SystemTime::now(),
        me_uid: unsafe { libc::getuid() },
        disk_free,
        run_pnpm: real_pnpm(home),
    };
    (processes, ctx)
}

// ---------------------------------------------------------------- reveal and icons

/// An item id, or `recent:<recentAtMs>:<index>` into the recent-files list (0 when never scanned).
/// The scan time pins the index to the list the owner was looking at. Never a path.
fn resolve_reveal(snap: &StorageData, id: &str) -> Result<PathBuf, String> {
    if let Some(rest) = id.strip_prefix("recent:") {
        let unknown = || "Lokasi tidak dikenal".to_string();
        let (at, index) = rest.split_once(':').ok_or_else(unknown)?;
        let at: u64 = at.parse().map_err(|_| unknown())?;
        if at != snap.recent_at_ms.unwrap_or(0) {
            return Err("Daftar sudah dipindai ulang, coba lagi".into());
        }
        let file = snap.recent.get(index.parse::<usize>().map_err(|_| unknown())?).ok_or_else(unknown)?;
        return Ok(file.path.clone());
    }
    snap.items.iter().find(|i| i.id == id).map(|i| i.path.clone()).ok_or_else(|| "Lokasi tidak dikenal".into())
}

/// Marks items whose cleanup `clean` would refuse right now, so the row shows the reason before
/// the click. Without a process list (panel closed) nothing is marked; `clean` still checks.
fn mark_refusals(snap: &mut StorageData, processes: Option<&[RawProcess]>) {
    let Some(processes) = processes else { return };
    for item in snap.items.iter_mut().filter(|i| matches!(i.state, ItemState::Ready | ItemState::Pending)) {
        if let Some(why) = refusal(item, processes) {
            item.state = ItemState::Refused(why);
        }
    }
}

fn valid_bundle(bundle: &str) -> Option<PathBuf> {
    let p = PathBuf::from(bundle);
    (p.is_absolute() && p.extension() == Some(OsStr::new("app")) && p.is_dir()).then_some(p)
}

enum IconError {
    /// The bundle has no `.icns` icon. This will not change, so it is cached.
    NoIcon,
    /// A tool failed or timed out; a later try may work.
    ToolFailed,
}

/// The icon file named by `CFBundleIconFile`, inside the bundle's own `Resources`.
fn icns_path(bundle: &Path) -> Result<PathBuf, IconError> {
    let plist = bundle.join("Contents/Info.plist");
    if !plist.is_file() {
        return Err(IconError::NoIcon);
    }
    let mut cmd = Command::new("/usr/bin/plutil");
    cmd.args(["-extract", "CFBundleIconFile", "raw", "-o", "-"]).arg(plist);
    let (code, out) = run_with_timeout(&mut cmd, TOOL_TIMEOUT).ok_or(IconError::ToolFailed)?;
    let name = out.trim();
    if code != Some(0) || name.is_empty() || name.contains('/') {
        return Err(IconError::NoIcon);
    }
    let file = if name.ends_with(".icns") { name.to_string() } else { format!("{name}.icns") };
    Some(bundle.join("Contents/Resources").join(file)).filter(|p| p.is_file()).ok_or(IconError::NoIcon)
}

static ICON_TMP: AtomicU32 = AtomicU32::new(0);

/// A new directory that only this user can enter, inside the per-user temp folder. Created
/// exclusively, so it can never be a pre-planted link. `None` when the per-user folder is
/// unknown (the fallback `/private/tmp` is shared, so it is not used).
fn private_scratch_dir() -> Option<PathBuf> {
    let base = Paths::current().tmpdir;
    if base == Path::new("/private/tmp") {
        return None;
    }
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let dir = base.join(format!("pantau-icon-{}-{}-{nanos}", std::process::id(), ICON_TMP.fetch_add(1, Ordering::Relaxed)));
    fs::DirBuilder::new().mode(0o700).create(&dir).ok()?;
    Some(dir)
}

/// Converts with `sips`, which ships with macOS, so no new dependency is needed. Apps whose icon
/// lives only in an asset catalog have no `.icns` and get no icon.
fn icon_data_url(bundle: &Path) -> Result<String, IconError> {
    let icns = icns_path(bundle)?;
    let dir = private_scratch_dir().ok_or(IconError::ToolFailed)?;
    let out = dir.join("icon.png");
    let mut cmd = Command::new("/usr/bin/sips");
    cmd.args(["-s", "format", "png", "-Z", ICON_SIZE]).arg(&icns).arg("--out").arg(&out);
    let ok = matches!(spawn_with_timeout(&mut cmd, TOOL_TIMEOUT), Ok(Some((Some(0), _))));
    let png = if ok { fs::read(&out).ok() } else { None };
    let _ = fs::remove_dir_all(&dir);
    png.filter(|b| !b.is_empty())
        .map(|b| format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(b)))
        .ok_or(IconError::ToolFailed)
}

/// Icons by bundle path. A bundle with no icon is remembered as `None`; a tool failure is not, so a retry can succeed.
#[derive(Default)]
pub struct IconCache(Mutex<HashMap<String, Option<String>>>);

impl IconCache {
    pub fn get(&self, bundle: &str) -> Option<String> {
        let path = valid_bundle(bundle)?;
        if let Some(hit) = lock(&self.0).get(bundle) {
            return hit.clone();
        }
        match icon_data_url(&path) {
            Ok(url) => {
                lock(&self.0).insert(bundle.to_string(), Some(url.clone()));
                Some(url)
            }
            Err(IconError::NoIcon) => {
                lock(&self.0).insert(bundle.to_string(), None);
                None
            }
            Err(IconError::ToolFailed) => None,
        }
    }
}

// ---------------------------------------------------------------- settings

/// What the settings window shows and sends back. `launch_at_login` is read from the autostart
/// plugin, never from the file, and `shortcut_error` is only ever sent to the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsView {
    pub launch_at_login: bool,
    pub shortcut: String,
    pub notifications: bool,
    pub scan_folders: Vec<PathBuf>,
    #[serde(default, skip_deserializing)]
    pub shortcut_error: Option<String>,
}

/// What the system shortcut currently is. Held for a whole save, so two saves never interleave.
#[derive(Default)]
pub struct ShortcutState(pub Mutex<shortcut::Status>);

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map(|d| d.join("settings.json")).map_err(|e| other_error("Folder data Pantau tidak ditemukan", e))
}

// ---------------------------------------------------------------- commands

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| other_error("Tugas berhenti sebelum selesai", e))
}

fn fresh_processes() -> Vec<RawProcess> {
    ProcessReader::new().read()
}

#[tauri::command]
pub fn get_snapshot(ops: State<'_, OpStore>) -> PanelData {
    panel_data(sampler::latest_snapshot(), sampler::history(), ops.operations())
}

#[tauri::command]
pub async fn plan_kill(targets: Vec<ProcId>) -> Result<KillPlan, String> {
    blocking(move || kill::plan_kill(&targets, &fresh_processes(), &project_name)).await
}

#[tauri::command]
pub async fn start_kill(app: AppHandle, targets: Vec<ProcId>, op_id: String, confirmed: bool) -> Result<(), String> {
    let store = app.state::<OpStore>().inner().clone();
    blocking(move || start_kill_inner(&store, &fresh_processes(), targets, op_id, confirmed)).await?
}

#[tauri::command]
pub async fn force_kill(app: AppHandle, targets: Vec<ProcId>, op_id: String) -> Result<(), String> {
    let store = app.state::<OpStore>().inner().clone();
    blocking(move || force_kill_inner(&store, &fresh_processes(), targets, op_id)).await?
}

/// Asking for storage data is what starts the (rate-limited) measurement.
#[tauri::command(async)]
pub fn get_storage(scheduler: State<'_, Scheduler>) -> StorageData {
    scheduler.request_measure(false);
    // Rate-limited to once a day by the scheduler.
    scheduler.request_recent_scan(false);
    let mut snap = scheduler.snapshot();
    mark_refusals(&mut snap, sampler::latest_raw().as_deref());
    snap
}

#[tauri::command(async)]
pub fn refresh_storage(scheduler: State<'_, Scheduler>) {
    scheduler.request_measure(true);
}

#[tauri::command(async)]
pub fn scan_recent_now(scheduler: State<'_, Scheduler>) {
    scheduler.request_recent_scan(true);
}

/// The item is looked up by id in the scheduler's own state; the webview never supplies a path.
#[tauri::command(async)]
pub fn clean_item(app: AppHandle, id: String, op_id: String) -> Result<(), String> {
    let item = app.state::<Scheduler>().snapshot().items.into_iter().find(|i| i.id == id).ok_or("Item tidak dikenal")?;
    let store = app.state::<OpStore>().inner().clone();
    let for_context = item.clone();
    let handle = app.clone();
    // The row must show the new size without any UI logic: ask for a measurement when it ends.
    clean_item_inner(&store, item, op_id, move || real_context(&for_context), move || {
        handle.state::<Scheduler>().request_measure_item(&id);
    })
}

fn run_open(args: Vec<std::ffi::OsString>) -> Result<(), String> {
    match Command::new("/usr/bin/open").args(args).status() {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Err("macOS tidak bisa membuka lokasi ini".into()),
        Err(e) => Err(os_error("Tidak bisa menjalankan open", &e)),
    }
}

#[tauri::command]
pub async fn reveal_in_finder(app: AppHandle, id: String) -> Result<(), String> {
    let snap = app.state::<Scheduler>().snapshot();
    let path = resolve_reveal(&snap, &id)?;
    blocking(move || run_open(vec!["-R".into(), path.into_os_string()])).await?
}

fn activity_monitor_args() -> Vec<OsString> {
    vec!["/System/Applications/Utilities/Activity Monitor.app".into()]
}

#[tauri::command]
pub async fn open_activity_monitor() -> Result<(), String> {
    blocking(|| run_open(activity_monitor_args())).await?
}

#[tauri::command]
pub async fn open_privacy_settings() -> Result<(), String> {
    blocking(|| run_open(vec![PRIVACY_PANE.into()])).await?
}

#[tauri::command]
pub fn open_settings(app: AppHandle) {
    panel::open_settings(&app);
}

#[tauri::command]
pub async fn app_icon(app: AppHandle, bundle: String) -> Result<Option<String>, String> {
    let icons = app.clone();
    blocking(move || icons.state::<IconCache>().get(&bundle)).await
}

fn view_of(app: &AppHandle, s: Settings) -> SettingsView {
    use tauri_plugin_autostart::ManagerExt;
    let shortcut_error = app.state::<ShortcutState>().0.lock().unwrap_or_else(|e| e.into_inner()).error.clone();
    SettingsView { launch_at_login: app.autolaunch().is_enabled().unwrap_or(false), shortcut: s.shortcut, notifications: s.notifications, scan_folders: s.scan_folders, shortcut_error }
}

#[tauri::command(async)]
pub fn get_settings(app: AppHandle) -> Result<SettingsView, String> {
    Ok(view_of(&app, settings::load(&settings_path(&app)?)))
}

/// All or nothing for the shortcut and the folders: a refused value changes nothing. Launch at
/// login is applied last; if it fails the rest is already saved, and the window re-reads.
/// Returns what was saved, with folders as their canonical paths.
#[tauri::command(async)]
pub fn save_settings(app: AppHandle, settings: SettingsView) -> Result<SettingsView, String> {
    use tauri_plugin_autostart::ManagerExt;
    let path = settings_path(&app)?;
    let home = settings::home();
    let folders = settings::validate_scan_folders(&settings.scan_folders, &home)?;

    let state = app.state::<ShortcutState>();
    let mut status = state.0.lock().unwrap_or_else(|e| e.into_inner());
    let previous_shortcut = status.registered.clone();
    let before = settings::load(&path);
    // A stored shortcut that cannot register must not lock the other settings.
    shortcut::apply_for_save(&shortcut::System(&app), &mut status, &settings.shortcut, &before.shortcut)?;

    let saved = Settings { shortcut: settings.shortcut.clone(), notifications: settings.notifications, scan_folders: folders };
    if let Err(e) = settings::save(&path, &saved) {
        if status.registered != previous_shortcut {
            match previous_shortcut {
                Some(old) => {
                    let _ = shortcut::apply(&shortcut::System(&app), &mut status, &old);
                }
                None => {
                    if let Some(new) = status.registered.take() {
                        shortcut::Registrar::unregister(&shortcut::System(&app), &new);
                    }
                }
            }
        }
        return Err(os_error("Pengaturan tidak bisa disimpan", &e));
    }
    settings::set_active(&saved);
    if saved.scan_folders != before.scan_folders {
        app.state::<Scheduler>().set_scan_folders(saved.scan_folders.clone());
    }
    drop(status);

    let autolaunch = app.autolaunch();
    if autolaunch.is_enabled().unwrap_or(false) != settings.launch_at_login {
        let changed = if settings.launch_at_login { autolaunch.enable() } else { autolaunch.disable() };
        changed.map_err(|e| other_error("Mulai saat login tidak bisa diubah", e))?;
    }
    Ok(view_of(&app, saved))
}

/// The folder picker is the only way to add a project folder; the window never sends typed text.
#[tauri::command]
pub async fn pick_scan_folder(window: tauri::WebviewWindow) -> Result<Option<PathBuf>, String> {
    use tauri_plugin_dialog::DialogExt;
    let home = settings::home();
    let picked = blocking(move || {
        window.dialog().file().set_title("Pilih folder project").set_directory(home).set_parent(&window).blocking_pick_folder()
    })
    .await?;
    picked.map(|p| p.into_path().map_err(|e| other_error("Folder ini tidak bisa dipakai", e))).transpose()
}

#[tauri::command]
pub fn close_settings(app: AppHandle) {
    if let Some(win) = app.get_webview_window(panel::SETTINGS) {
        let _ = win.destroy();
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant, SystemTime};

    use super::*;
    use crate::process::kill::read_start;
    use crate::process::test_support::proc;
    use crate::storage::recent::RecentFile;
    use crate::storage::{Category, Cleaner, ItemState};

    /// A spawned test child that is killed and reaped when dropped.
    struct Guard(Child);

    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn spawn(script: &str) -> Guard {
        let mut child = Command::new("sh")
            .args(["-c", script])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // Waits for "ready" so a TERM trap is installed before anything is signalled.
        let mut buf = [0u8; 5];
        child.stdout.as_mut().unwrap().read_exact(&mut buf).unwrap();
        Guard(child)
    }

    fn sleeper() -> Guard {
        spawn("echo ready; exec sleep 60")
    }

    fn id_of(g: &Guard) -> ProcId {
        ProcId { pid: g.0.id(), start_time: read_start(g.0.id()).expect("child is alive") }
    }

    fn alive(t: ProcId) -> bool {
        read_start(t.pid) == Some(t.start_time)
    }

    fn fresh() -> Vec<RawProcess> {
        ProcessReader::new().read()
    }

    fn state_of(store: &OpStore, op: &str) -> Option<OpState> {
        store.operations().into_iter().find(|o| o.op_id == op).map(|o| o.state)
    }

    fn cached_count(cache: &IconCache) -> usize {
        lock(&cache.0).len()
    }

    fn wait_done(store: &OpStore, op: &str) -> OpState {
        let end = Instant::now() + Duration::from_secs(15);
        loop {
            match state_of(store, op) {
                Some(OpState::Running) | None if Instant::now() < end => std::thread::sleep(Duration::from_millis(20)),
                Some(s) => return s,
                None => panic!("operation {op} never appeared"),
            }
        }
    }

    const SHORT: Duration = Duration::from_millis(400);

    #[test]
    fn an_operation_keeps_running_and_finishes_without_a_panel() {
        let child = sleeper();
        let t = id_of(&child);
        let store = OpStore::new();
        start_kill_inner(&store, &fresh(), vec![t], "op1".into(), false).unwrap();
        assert_eq!(wait_done(&store, "op1"), OpState::Done { freed_bytes: None, skipped: 0 });
        assert!(!alive(t));
    }

    #[test]
    fn old_finished_operations_are_dropped_after_60_seconds_but_running_ones_stay() {
        let store = OpStore::new();
        store.begin("done").unwrap();
        store.finish("done", OpState::Done { freed_bytes: None, skipped: 0 }, vec![]);
        store.begin("running").unwrap();
        store.prune_at(Instant::now() + Duration::from_secs(59));
        assert_eq!(store.operations().len(), 2);
        store.prune_at(Instant::now() + Duration::from_secs(61));
        let left = store.operations();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].op_id, "running");
    }

    #[test]
    fn an_op_id_that_is_already_running_is_refused() {
        let store = OpStore::new();
        store.begin("a").unwrap();
        assert!(store.begin("a").is_err());
        store.finish("a", OpState::StillAlive, vec![]);
        assert!(store.begin("a").is_ok(), "a finished op id can be reused for a retry");
    }

    #[test]
    fn several_targets_need_confirmation_and_nothing_is_sent_without_it() {
        let (a, b) = (sleeper(), sleeper());
        let targets = vec![id_of(&a), id_of(&b)];
        let store = OpStore::new();
        let err = start_kill_inner(&store, &fresh(), targets.clone(), "op".into(), false).unwrap_err();
        assert!(err.contains("konfirmasi"), "{err}");
        std::thread::sleep(Duration::from_millis(300));
        assert!(alive(targets[0]) && alive(targets[1]), "refused kill must not signal anything");
        assert!(store.operations().is_empty());

        start_kill_inner(&store, &fresh(), targets.clone(), "op".into(), true).unwrap();
        assert_eq!(wait_done(&store, "op"), OpState::Done { freed_bytes: None, skipped: 0 });
        assert!(!alive(targets[0]) && !alive(targets[1]));
    }

    #[test]
    fn a_target_the_fresh_list_says_is_root_is_never_signalled() {
        let child = sleeper();
        let t = ProcId { pid: child.0.id(), start_time: 0 };
        // The UI said killable; the fresh list says the process belongs to root.
        let raw = vec![proc(t.pid, 1).uid(0)];
        let store = OpStore::new();
        let err = start_kill_inner(&store, &raw, vec![t], "op".into(), true).unwrap_err();
        assert!(err.contains("root"), "{err}");
        let err = force_kill_inner(&store, &raw, vec![t], "op".into()).unwrap_err();
        assert!(err.contains("root"), "{err}");
        std::thread::sleep(Duration::from_millis(300));
        assert!(read_start(t.pid).is_some(), "child must still be alive");
        assert!(store.operations().is_empty());
    }

    #[test]
    fn a_blocked_target_among_allowed_ones_is_dropped_and_reported() {
        let (a, b) = (sleeper(), sleeper());
        let (ta, tb) = (id_of(&a), id_of(&b));
        let mut raw = fresh();
        raw.iter_mut().find(|p| p.pid == tb.pid).unwrap().uid = 0;
        let store = OpStore::new();
        start_kill_inner(&store, &raw, vec![ta, tb], "op".into(), true).unwrap();
        let OpState::Failed(msg) = wait_done(&store, "op") else { panic!("expected Failed") };
        assert!(msg.contains("1 dari 2") && msg.contains("root") && msg.contains("sudah berhenti"), "{msg}");
        assert!(!alive(ta));
        assert!(alive(tb), "the blocked process must be left alone");
    }

    #[test]
    fn a_reused_pid_is_treated_as_gone_and_the_new_process_is_left_alone() {
        let child = sleeper();
        let real = id_of(&child);
        let stale = ProcId { pid: real.pid, start_time: real.start_time + 1 };
        let store = OpStore::new();
        start_kill_inner(&store, &fresh(), vec![stale], "op".into(), false).unwrap();
        assert_eq!(wait_done(&store, "op"), OpState::Done { freed_bytes: None, skipped: 0 });
        assert!(alive(real));
    }

    #[test]
    fn a_process_that_ignores_term_is_reported_as_still_alive_then_force_stops_it() {
        let child = spawn(r#"trap "" TERM; echo ready; while :; do sleep 1; done"#);
        let t = id_of(&child);
        let store = OpStore::new();
        start_kill_with(&store, &fresh(), vec![t], "op".into(), false, SHORT).unwrap();
        assert_eq!(wait_done(&store, "op"), OpState::StillAlive);
        assert!(alive(t));
        force_kill_inner(&store, &fresh(), vec![t], "op".into()).unwrap();
        assert_eq!(wait_done(&store, "op"), OpState::Done { freed_bytes: None, skipped: 0 });
        assert!(!alive(t));
    }

    #[test]
    fn a_partial_kill_is_reported_honestly() {
        let failed = vec!["PID 9 gagal".to_string()];
        assert_eq!(kill_state(1, Some(KillOutcome::Exited), &[]), OpState::Done { freed_bytes: None, skipped: 0 });
        assert_eq!(kill_state(1, Some(KillOutcome::StillAlive), &[]), OpState::StillAlive);
        assert_eq!(kill_state(1, None, &[]), OpState::Done { freed_bytes: None, skipped: 0 });
        assert_eq!(
            kill_state(3, Some(KillOutcome::Exited), &failed),
            OpState::Failed("1 dari 3 gagal: PID 9 gagal. Sisanya sudah berhenti.".into())
        );
        assert_eq!(
            kill_state(3, Some(KillOutcome::StillAlive), &failed),
            OpState::Failed("1 dari 3 gagal: PID 9 gagal. Sisanya belum berhenti.".into())
        );
        assert_eq!(kill_state(1, None, &failed), OpState::Failed("Gagal dihentikan: PID 9 gagal".into()));
    }

    fn storage_item(path: &Path, cleaner: Cleaner) -> StorageItem {
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

    fn no_disk() -> u64 {
        0
    }

    fn clean_ctx(root: &Path) -> CleanContext {
        CleanContext {
            paths: Paths {
                home: root.join("home"),
                tmpdir: root.join("var-T"),
                private_tmp: root.join("private-tmp"),
                scan_folders: vec![root.join("work")],
            },
            open_paths: Some(Default::default()),
            now: SystemTime::now() + Duration::from_secs(10 * 24 * 3600),
            me_uid: unsafe { libc::getuid() },
            disk_free: no_disk,
            run_pnpm: Box::new(|| panic!("tests never run pnpm")),
        }
    }

    #[test]
    fn clean_builds_its_context_on_a_worker_thread_and_reports_done_with_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("var-T");
        std::fs::create_dir_all(tmp.join("old")).unwrap();
        std::fs::write(tmp.join("old/file"), b"data").unwrap();
        let root = dir.path().to_path_buf();
        let caller = std::thread::current().id();
        let store = OpStore::new();
        clean_item_inner(&store, storage_item(&tmp, Cleaner::OldTemp), "c1".into(), move || {
            assert_ne!(std::thread::current().id(), caller, "context must be built off the calling thread");
            (Vec::new(), clean_ctx(&root))
        }, || {})
        .unwrap();
        assert_eq!(wait_done(&store, "c1"), OpState::Done { freed_bytes: Some(0), skipped: 0 });
        assert!(!tmp.join("old").exists());
    }

    #[test]
    fn a_refused_clean_is_a_failed_operation_with_the_reason() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let store = OpStore::new();
        clean_item_inner(&store, storage_item(&root.join("work/thing"), Cleaner::None), "c2".into(), move || (Vec::new(), clean_ctx(&root)), || {}).unwrap();
        let OpState::Failed(msg) = wait_done(&store, "c2") else { panic!("expected Failed") };
        assert!(msg.contains("tidak bisa dibersihkan"), "{msg}");
    }

    #[test]
    fn temp_and_claude_version_cleanups_read_the_open_files() {
        assert!(needs_open_paths(Cleaner::OldTemp));
        assert!(needs_open_paths(Cleaner::OldClaudeVersions));
        assert!(!needs_open_paths(Cleaner::RemoveDir));
        assert!(!needs_open_paths(Cleaner::PnpmPrune));
    }

    #[test]
    fn clean_state_carries_the_skipped_count() {
        let r = CleanResult { freed_bytes: 10, skipped: 3 };
        assert_eq!(clean_state(Ok(r)), OpState::Done { freed_bytes: Some(10), skipped: 3 });
        assert_eq!(clean_state(Err("x".into())), OpState::Failed("x".into()));
    }

    #[test]
    fn op_state_serializes_with_snake_case_fields() {
        let op = Operation { op_id: "a".into(), state: OpState::Done { freed_bytes: Some(5), skipped: 1 }, survivors: vec![] };
        assert_eq!(serde_json::to_string(&op).unwrap(), r#"{"op_id":"a","state":{"Done":{"freed_bytes":5,"skipped":1}},"survivors":[]}"#);
    }

    fn snapshot_with(items: Vec<StorageItem>, recent: Vec<RecentFile>) -> StorageData {
        StorageData { items, recent, recent_at_ms: None, recent_skipped_dirs: 0, recent_denied: false, denied_roots: vec![], paused: false, measuring: false, scanning: false, measuring_auto: false, scanning_auto: false }
    }

    #[test]
    fn reveal_resolves_an_item_id_or_a_recent_index_and_nothing_else() {
        let item = storage_item(Path::new("/made/up/item"), Cleaner::None);
        let recent = |p: &str| RecentFile { path: p.into(), bytes: 1, changed_at_secs: 0, owner_guess: None };
        let mut snap = snapshot_with(vec![item], vec![recent("/made/up/a.bin"), recent("/made/up/b.bin")]);
        snap.recent_at_ms = Some(42);
        assert_eq!(resolve_reveal(&snap, "x"), Ok(PathBuf::from("/made/up/item")));
        assert_eq!(resolve_reveal(&snap, "recent:42:1"), Ok(PathBuf::from("/made/up/b.bin")));
        assert!(resolve_reveal(&snap, "recent:42:2").is_err());
        assert!(resolve_reveal(&snap, "/etc/passwd").is_err());
        assert!(resolve_reveal(&snap, "recent:42:x").is_err());
        assert!(resolve_reveal(&snap, "recent:1").is_err());
    }

    #[test]
    fn a_recent_id_from_an_older_scan_is_refused_instead_of_revealing_another_file() {
        let recent = |p: &str| RecentFile { path: p.into(), bytes: 1, changed_at_secs: 0, owner_guess: None };
        let mut snap = snapshot_with(vec![], vec![recent("/made/up/new.bin")]);
        snap.recent_at_ms = Some(200);
        let why = resolve_reveal(&snap, "recent:100:0").unwrap_err();
        assert!(why.contains("dipindai ulang"), "{why}");
        // A list that was never scanned is id 0.
        snap.recent_at_ms = None;
        assert_eq!(resolve_reveal(&snap, "recent:0:0"), Ok(PathBuf::from("/made/up/new.bin")));
    }

    #[test]
    fn the_privacy_button_opens_files_and_folders() {
        assert!(PRIVACY_PANE.ends_with("?Privacy_FilesAndFolders"), "{PRIVACY_PANE}");
    }

    #[test]
    fn panel_data_is_empty_but_has_history_before_the_first_sample() {
        let d = panel_data(None, Vec::new(), Vec::new());
        assert!(d.groups.is_empty() && d.leftovers.is_empty() && d.history.is_empty() && d.operations.is_empty());
    }

    #[test]
    fn tool_dirs_cover_the_known_places_and_pick_the_newest_node() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        for v in ["v22.9.0", "v22.21.1", "v24.15.0"] {
            std::fs::create_dir_all(home.join(".nvm/versions/node").join(v).join("bin")).unwrap();
        }
        let dirs = tool_dirs(home);
        let pos = |p: PathBuf| dirs.iter().position(|d| *d == p).unwrap_or_else(|| panic!("{p:?} missing"));
        assert!(pos(home.join("Library/pnpm")) < pos(PathBuf::from("/opt/homebrew/bin")));
        assert!(dirs.contains(&PathBuf::from("/usr/local/bin")));
        assert!(dirs.contains(&home.join(".local/bin")));
        assert!(dirs.contains(&home.join(".volta/bin")));
        assert!(pos(home.join(".nvm/versions/node/v24.15.0/bin")) < pos(home.join(".nvm/versions/node/v22.21.1/bin")));
        assert!(pos(home.join(".nvm/versions/node/v22.21.1/bin")) < pos(home.join(".nvm/versions/node/v22.9.0/bin")));
    }

    #[test]
    fn the_pnpm_runner_passes_a_path_that_includes_the_tool_folders() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("seen.txt");
        let fake = dir.path().join("pnpm");
        std::fs::write(&fake, format!("#!/bin/sh\necho \"$@\" > '{}'\necho \"$PATH\" >> '{}'\n", out.display(), out.display())).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = build_path(&[dir.path().to_path_buf(), PathBuf::from("/opt/made-up/bin")], "/usr/bin".as_ref());
        run_pnpm_at(&fake, &path, dir.path()).unwrap();
        let seen = std::fs::read_to_string(&out).unwrap();
        assert!(seen.starts_with("store prune\n"), "{seen}");
        assert!(seen.contains("/opt/made-up/bin") && seen.contains("/usr/bin"), "{seen}");
        assert_eq!(find_in(&[PathBuf::from("/nonexistent"), dir.path().to_path_buf()], "pnpm"), Some(fake));
        assert_eq!(find_in(&[dir.path().to_path_buf()], "no-such-tool"), None);
    }

    #[test]
    fn pnpm_runs_from_its_own_folder_when_there_is_one_and_from_ours_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let own = home.join("Library/Application Support/local.toolslab.pantau");
        assert_eq!(pnpm_cwd(home), own);
        assert!(own.is_dir(), "made when missing");
        std::fs::create_dir_all(home.join("Library/pnpm")).unwrap();
        assert_eq!(pnpm_cwd(home), home.join("Library/pnpm"));
    }

    #[test]
    fn a_relative_entry_in_path_is_not_passed_on() {
        let path = build_path(&[PathBuf::from("/opt/a")], "/usr/bin::bin:./node_modules/.bin:/opt/b".as_ref());
        let parts: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert!(parts.iter().all(|p| p.is_absolute()), "{parts:?}");
        assert!(parts.contains(&PathBuf::from("/opt/b")) && parts.contains(&PathBuf::from("/usr/bin")));
    }

    #[test]
    fn activity_monitor_is_opened_by_its_absolute_path() {
        assert_eq!(activity_monitor_args(), [std::ffi::OsString::from("/System/Applications/Utilities/Activity Monitor.app")]);
    }

    #[test]
    fn icons_are_only_read_from_real_app_directories() {
        let dir = tempfile::tempdir().unwrap();
        let file_app = dir.path().join("File.app");
        std::fs::write(&file_app, b"").unwrap();
        let empty_app = dir.path().join("Empty.app");
        std::fs::create_dir(&empty_app).unwrap();
        let cache = IconCache::default();
        assert_eq!(cache.get("/etc/passwd"), None);
        assert_eq!(cache.get("relative/X.app"), None);
        assert_eq!(cache.get("/nonexistent/X.app"), None);
        assert_eq!(cache.get(&file_app.to_string_lossy()), None);
        assert_eq!(cache.get(&empty_app.to_string_lossy()), None, "no Info.plist means no icon, not an error");
    }

    #[test]
    fn a_real_app_icon_becomes_a_png_data_url_and_is_cached() {
        let cache = IconCache::default();
        let url = cache.get("/System/Applications/Calculator.app").expect("Calculator has an icns icon");
        assert!(url.starts_with("data:image/png;base64,iVBOR"), "{}", &url[..40.min(url.len())]);
        assert_eq!(cache.get("/System/Applications/Calculator.app"), Some(url));
        assert_eq!(cached_count(&cache), 1);
    }

    #[test]
    fn force_kill_only_reaches_survivors_of_an_earlier_op() {
        let stubborn = spawn(r#"trap "" TERM; echo ready; while :; do sleep 1; done"#);
        let bystander = sleeper();
        let (t, other) = (id_of(&stubborn), id_of(&bystander));
        let store = OpStore::new();
        // Nobody tried TERM yet: no op, no force.
        assert!(force_kill_inner(&store, &fresh(), vec![t], "op".into()).is_err());
        assert!(alive(t));
        start_kill_with(&store, &fresh(), vec![t], "op".into(), false, SHORT).unwrap();
        assert_eq!(wait_done(&store, "op"), OpState::StillAlive);
        assert_eq!(store.operations()[0].survivors, vec![t]);
        // A target that was never terminated, or another op id, is refused.
        assert!(force_kill_inner(&store, &fresh(), vec![t, other], "op".into()).is_err());
        assert!(force_kill_inner(&store, &fresh(), vec![t], "other-op".into()).is_err());
        assert!(alive(t) && alive(other));
        force_kill_inner(&store, &fresh(), vec![t], "op".into()).unwrap();
        assert_eq!(wait_done(&store, "op"), OpState::Done { freed_bytes: None, skipped: 0 });
        assert!(!alive(t) && alive(other));
    }

    #[test]
    fn some_failed_and_some_alive_keeps_the_survivors_for_force() {
        let (stubborn, blocked) = (spawn(r#"trap "" TERM; echo ready; while :; do sleep 1; done"#), sleeper());
        let (ts, tb) = (id_of(&stubborn), id_of(&blocked));
        let mut raw = fresh();
        raw.iter_mut().find(|p| p.pid == tb.pid).unwrap().uid = 0;
        let store = OpStore::new();
        start_kill_with(&store, &raw, vec![ts, tb], "op".into(), true, SHORT).unwrap();
        let OpState::Failed(msg) = wait_done(&store, "op") else { panic!("expected Failed") };
        assert!(msg.contains("root") && msg.contains("belum berhenti"), "{msg}");
        assert_eq!(store.operations()[0].survivors, vec![ts]);
    }

    #[test]
    fn a_live_target_missing_from_the_list_is_refused_not_waved_through() {
        let child = sleeper();
        let t = id_of(&child);
        let store = OpStore::new();
        let err = start_kill_inner(&store, &[], vec![t], "op".into(), true).unwrap_err();
        assert!(err.contains("daftar"), "{err}");
        std::thread::sleep(Duration::from_millis(200));
        assert!(alive(t));
        // Gone or reused pids are not refused.
        let gone = ProcId { pid: 2_000_000, start_time: 5 };
        assert!(start_kill_inner(&store, &[], vec![gone], "gone".into(), true).is_ok());
    }

    #[test]
    fn an_item_cannot_be_cleaned_twice_at_once() {
        use std::sync::mpsc::channel;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let item = storage_item(&root.join("work/thing"), Cleaner::None);
        let (release, wait) = channel::<()>();
        let store = OpStore::new();
        let r = root.clone();
        clean_item_inner(&store, item.clone(), "c1".into(), move || {
            let _ = wait.recv();
            (Vec::new(), clean_ctx(&r))
        }, || {})
        .unwrap();
        let err = clean_item_inner(&store, item.clone(), "c2".into(), || unreachable!(), || {}).unwrap_err();
        assert!(err.contains("sedang dibersihkan"), "{err}");
        assert_eq!(state_of(&store, "c2"), None);
        release.send(()).unwrap();
        wait_done(&store, "c1");
        let end = Instant::now() + Duration::from_secs(5);
        while clean_item_inner(&store, item.clone(), "c3".into(), { let r = root.clone(); move || (Vec::new(), clean_ctx(&r)) }, || {}).is_err() {
            assert!(Instant::now() < end, "the guard was never released");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_finished_clean_asks_for_a_new_measurement() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        let store = OpStore::new();
        clean_item_inner(&store, storage_item(&root.join("work/thing"), Cleaner::None), "c".into(), move || (Vec::new(), clean_ctx(&root)), move || tx.send(()).unwrap()).unwrap();
        rx.recv_timeout(Duration::from_secs(5)).expect("remeasure hook was not called");
    }

    #[test]
    fn op_ids_must_be_short_and_not_empty() {
        let store = OpStore::new();
        assert!(store.begin("").is_err());
        assert!(store.begin(&"x".repeat(4097)).is_err());
        assert!(store.begin(&"x".repeat(4096)).is_ok());
    }

    #[test]
    fn running_projects_mark_next_items_refused_before_the_click() {
        let mut item = storage_item(Path::new("/made/up/proj/.next"), Cleaner::RemoveDir);
        item.state = ItemState::Ready;
        let mut other = storage_item(Path::new("/made/up/idle/.next"), Cleaner::RemoveDir);
        other.id = "idle".into();
        other.state = ItemState::Ready;
        let mut snap = snapshot_with(vec![item, other], vec![]);
        mark_refusals(&mut snap, None);
        assert_eq!(snap.items[0].state, ItemState::Ready, "no process list yet: leave alone");
        let raw = vec![proc(5, 1).cwd("/made/up/proj/src")];
        mark_refusals(&mut snap, Some(&raw));
        assert_eq!(snap.items[0].state, ItemState::Refused("Masih ada proses yang berjalan di project ini".into()));
        assert_eq!(snap.items[1].state, ItemState::Ready);
    }

    #[test]
    fn icon_failures_are_cached_but_invalid_bundles_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let empty_app = dir.path().join("Empty.app");
        std::fs::create_dir(&empty_app).unwrap();
        let cache = IconCache::default();
        assert_eq!(cache.get("/nonexistent/X.app"), None);
        assert_eq!(cached_count(&cache), 0);
        assert_eq!(cache.get(&empty_app.to_string_lossy()), None);
        assert_eq!(cached_count(&cache), 1);
    }

    #[test]
    fn icon_scratch_dirs_are_private_unique_and_under_the_user_temp_folder() {
        use std::os::unix::fs::PermissionsExt;
        let a = private_scratch_dir().expect("user temp dir");
        let b = private_scratch_dir().expect("user temp dir");
        assert_ne!(a, b);
        assert_ne!(a.parent(), Some(Path::new("/private/tmp")));
        assert_eq!(std::fs::metadata(&a).unwrap().permissions().mode() & 0o777, 0o700);
        let _ = std::fs::remove_dir(&a);
        let _ = std::fs::remove_dir(&b);
    }

    #[test]
    fn a_tool_failure_is_not_cached_but_a_bundle_without_icns_is() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Broken.app");
        std::fs::create_dir_all(app.join("Contents/Resources")).unwrap();
        std::fs::write(
            app.join("Contents/Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIconFile</key><string>Broken</string></dict></plist>"#,
        )
        .unwrap();
        std::fs::write(app.join("Contents/Resources/Broken.icns"), b"not an icns").unwrap();
        let cache = IconCache::default();
        assert_eq!(cache.get(&app.to_string_lossy()), None);
        assert_eq!(cached_count(&cache), 0, "sips failed: try again next time");
        let bare = dir.path().join("Bare.app");
        std::fs::create_dir(&bare).unwrap();
        assert_eq!(cache.get(&bare.to_string_lossy()), None);
        assert_eq!(cached_count(&cache), 1);
    }
}
