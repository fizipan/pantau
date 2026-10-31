//! Samples the machine every 2 seconds on its own thread and keeps the menu bar title current.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, OnceLock, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sysinfo::System;
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::{OpStore, panel_data};
use crate::notify::{self, NotifyPolicy};
use crate::panel;
use crate::settings::notifications_enabled;
use crate::process::Snapshot;
use crate::process::group::{group_processes, project_name};
use crate::process::leftover::leftovers_if_ports_known;
use crate::process::ports::read_ports;
use crate::process::raw::{ProcessReader, RawProcess};
use crate::system::{History, SystemSample, read_system, tray_title};
use crate::tray;

const INTERVAL: Duration = Duration::from_secs(2);
/// `lsof` is the costly read, so ports refresh less often than processes.
const PORTS_INTERVAL: Duration = Duration::from_secs(5);
/// How often the ports thread checks whether a read is due or the panel closed.
const PORTS_TICK: Duration = Duration::from_secs(1);
/// CPU per process is a delta: the first list after opening has none, so a second read follows this
/// long after it and fills CPU in. The list itself is shown at once.
const SETTLE: Duration = Duration::from_millis(500);

type Ports = HashMap<u32, Vec<u16>>;

static HISTORY: Mutex<Option<History>> = Mutex::new(None);
/// Last good port reading since the panel opened; `None` before the first success.
static PORTS: Mutex<Option<Ports>> = Mutex::new(None);
static SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);
/// The process list behind `SNAPSHOT`, for checks that need working directories.
static RAW: Mutex<Option<Vec<RawProcess>>> = Mutex::new(None);
static WAKE: OnceLock<mpsc::Sender<()>> = OnceLock::new();

/// Samples for the panel graph, oldest first.
pub fn history() -> Vec<SystemSample> {
    HISTORY.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(History::samples).unwrap_or_default()
}

/// The newest snapshot, `None` until the panel has been open once.
pub fn latest_snapshot() -> Option<Snapshot> {
    SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The newest process list, `None` while the panel is closed.
pub fn latest_raw() -> Option<Vec<RawProcess>> {
    RAW.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn build_snapshot(reader: &mut ProcessReader, ports: Option<&Ports>) -> Snapshot {
    let cpu_pending = !reader.has_baseline();
    let raw = reader.read();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let me = unsafe { libc::getuid() };
    let snapshot = Snapshot {
        groups: group_processes(&raw, ports.unwrap_or(&Ports::new()), &project_name, me, std::process::id()),
        leftovers: leftovers_if_ports_known(&raw, ports, now, me, &project_name),
        cpu_pending,
    };
    *RAW.lock().unwrap_or_else(|e| e.into_inner()) = Some(raw);
    snapshot
}

/// Runs one tick's work. A panic ends that tick only, so it cannot freeze the tray title for good.
fn guarded(what: &str, f: impl FnOnce()) {
    if catch_unwind(AssertUnwindSafe(f)).is_err() {
        eprintln!("[pantau] {what}: satu putaran berhenti karena kesalahan tak terduga");
    }
}

/// Reads ports every 5 s while the panel is open. `lsof` can be slow, so it runs here and never
/// on the sampler thread. A failed read keeps the previous ports.
fn spawn_ports_reader(app: AppHandle) {
    thread::spawn(move || {
        let mut last_read: Option<Instant> = None;
        loop {
            thread::sleep(PORTS_TICK);
            guarded("port", || {
                if !panel::is_open(&app) {
                    last_read = None;
                    *PORTS.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    return;
                }
                if last_read.is_some_and(|t| t.elapsed() < PORTS_INTERVAL) {
                    return;
                }
                last_read = Some(Instant::now());
                if let Some(ports) = read_ports()
                    && panel::is_open(&app)
                {
                    *PORTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(ports);
                }
            });
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Event {
    Tick,
    PanelOpened,
    /// The read after a baseline read, due before the tick.
    Settled,
}

fn next_event(wake: &mpsc::Receiver<()>, next_tick: Instant, settle_at: Option<Instant>) -> Event {
    let (due, on_timeout) = match settle_at {
        Some(at) if at < next_tick => (at, Event::Settled),
        _ => (next_tick, Event::Tick),
    };
    match wake.recv_timeout(due.saturating_duration_since(Instant::now())) {
        Ok(()) => Event::PanelOpened,
        Err(_) => on_timeout,
    }
}

/// A tick between the window opening and `panel_ready` has already started the reading; a second
/// one milliseconds later would show CPU over a few milliseconds, which is noise.
fn reads_panel(event: Event, has_reader: bool) -> bool {
    !(event == Event::PanelOpened && has_reader)
}

/// The panel is on screen: read processes now instead of at the next tick.
pub fn wake() {
    if let Some(tx) = WAKE.get() {
        let _ = tx.send(());
    }
}

/// Only with `--features tick-log`. Prints the largest gap between the samples `NotifyPolicy` sees,
/// once a minute, so a real run shows whether the 10 s limit is ever reached.
#[cfg(feature = "tick-log")]
#[derive(Default)]
struct TickLog {
    last_ms: Option<u64>,
    max_gap_ms: u64,
    count: u32,
}

#[cfg(feature = "tick-log")]
impl TickLog {
    fn note(&mut self, at_ms: u64) {
        if let Some(last) = self.last_ms.replace(at_ms) {
            self.max_gap_ms = self.max_gap_ms.max(at_ms.saturating_sub(last));
        }
        self.count += 1;
        if self.count == 30 {
            eprintln!("[pantau-tick] at_ms={at_ms} ticks={} max_gap_ms={}", self.count, self.max_gap_ms);
            (self.count, self.max_gap_ms) = (0, 0);
        }
    }
}

pub fn spawn_sampler(app: AppHandle) {
    spawn_ports_reader(app.clone());
    let (wake_tx, wake_rx) = mpsc::channel();
    let _ = WAKE.set(wake_tx);
    thread::spawn(move || {
        let mut sys = System::new();
        // CPU usage needs a baseline refresh before the first real reading.
        sys.refresh_cpu_usage();
        // Process and port reads exist only while the panel is open.
        let mut reader: Option<ProcessReader> = None;
        // Alerts matter most when the panel is closed, so this runs on every tick.
        let mut notify_policy = NotifyPolicy::default();
        #[cfg(feature = "tick-log")]
        let mut tick_log = TickLog::default();
        let mut next_tick = Instant::now() + INTERVAL;
        let mut settle_at: Option<Instant> = None;
        loop {
            let event = next_event(&wake_rx, next_tick, settle_at);
            if !reads_panel(event, reader.is_some()) {
                continue;
            }
            guarded("sampler", || {
                if event == Event::Tick {
                    let sample = read_system(&mut sys);
                    #[cfg(feature = "tick-log")]
                    tick_log.note(sample.at_ms);
                    tray::set_title(&app, &tray_title(&sample));
                    for alert in notify_policy.tick(&sample, sample.at_ms, notifications_enabled()) {
                        notify::deliver(&app, alert);
                    }
                    HISTORY.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(History::default).push(sample);
                }
                if panel::is_open(&app) {
                    let ports = PORTS.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    #[cfg(feature = "measure")]
                    let first = reader.is_none().then(Instant::now);
                    // Cleared first: a read that panics must not leave a due time that makes the loop spin.
                    settle_at.take();
                    let snapshot = build_snapshot(reader.get_or_insert_with(ProcessReader::new), ports.as_ref());
                    settle_at = snapshot.cpu_pending.then(|| Instant::now() + SETTLE);
                    #[cfg(feature = "measure")]
                    if let Some(t) = first {
                        let busy = snapshot.groups.iter().filter(|g| g.cpu_pct > 0.0).count();
                        eprintln!("[pantau] daftar proses pertama dalam {} ms ({event:?}), {} kelompok, {busy} dengan CPU di atas 0", t.elapsed().as_millis(), snapshot.groups.len());
                    }
                    *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = Some(snapshot.clone());
                    let ops = app.try_state::<OpStore>().map(|s| s.operations()).unwrap_or_default();
                    let _ = app.emit("snapshot", &panel_data(Some(snapshot), history(), ops));
                } else if reader.is_some() {
                    reader = None;
                    settle_at = None;
                    *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    *RAW.lock().unwrap_or_else(|e| e.into_inner()) = None;
                }
            });
            if event == Event::Tick {
                next_tick = Instant::now() + INTERVAL;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_in_one_tick_does_not_end_the_loop() {
        let mut ticks = 0;
        for i in 0..3 {
            guarded("test", || {
                ticks += 1;
                assert_ne!(i, 1, "a tick that panics");
            });
        }
        assert_eq!(ticks, 3);
    }

    #[test]
    fn the_panel_opening_wakes_the_sampler_before_the_tick() {
        let (tx, rx) = mpsc::channel();
        tx.send(()).unwrap();
        let started = Instant::now();
        assert_eq!(next_event(&rx, started + INTERVAL, None), Event::PanelOpened);
        assert!(started.elapsed() < INTERVAL / 2);
    }

    #[test]
    fn without_a_wake_the_tick_comes_on_time() {
        let (_tx, rx) = mpsc::channel::<()>();
        let started = Instant::now();
        assert_eq!(next_event(&rx, started + Duration::from_millis(50), None), Event::Tick);
        assert!(started.elapsed() >= Duration::from_millis(50));
    }

    #[test]
    fn the_first_read_of_a_new_reader_is_marked_as_having_no_cpu_yet() {
        let mut reader = ProcessReader::new();
        let first = build_snapshot(&mut reader, None);
        assert!(first.cpu_pending, "CPU is a delta; the first read has nothing to compare with");
        assert!(!first.groups.is_empty(), "the list itself is there at once");
        let second = build_snapshot(&mut reader, None);
        assert!(!second.cpu_pending);
    }

    #[test]
    fn after_a_baseline_read_the_next_read_comes_after_the_settle_time_not_the_tick() {
        let (tx, rx) = mpsc::channel();
        let _keep = tx;
        let soon = Instant::now() + Duration::from_millis(20);
        let far = Instant::now() + Duration::from_secs(60);
        assert_eq!(next_event(&rx, far, Some(soon)), Event::Settled);
        assert_eq!(next_event(&rx, soon, Some(far)), Event::Tick);
        assert_eq!(next_event(&rx, soon, None), Event::Tick);
    }

    #[test]
    fn a_panel_opened_signal_is_ignored_when_a_tick_already_started_reading() {
        assert!(reads_panel(Event::PanelOpened, false));
        assert!(!reads_panel(Event::PanelOpened, true), "a second read milliseconds later would show a wrong CPU");
        assert!(reads_panel(Event::Tick, true));
        assert!(reads_panel(Event::Settled, true));
    }
}
