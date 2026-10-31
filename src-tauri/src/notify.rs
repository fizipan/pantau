//! Closed-panel alerts: when to raise them (`NotifyPolicy`) and how (`deliver`).

use std::ffi::{CStr, c_char};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, OnceLock, mpsc};
use std::thread;
use std::time::Duration;

use objc2::msg_send;
use objc2::runtime::{AnyClass, AnyObject};
use tauri::AppHandle;

use crate::panel;
use crate::system::{Pressure, SystemSample};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alert {
    /// With the level at the moment it fired, so the title can say the same as the banner.
    MemoryPressure(Pressure),
    LowDisk,
}

const PRESSURE_FOR_MS: u64 = 30_000;
/// 20 GB as the panel shows it (the formatter counts in 1024s).
const LOW_DISK_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const REPEAT_MS: u64 = 15 * 60_000;
/// The sampler ticks every 2 s. A longer gap means sleep or a stall, so "30 seconds in a row" was not seen.
const MAX_TICK_GAP_MS: u64 = 10_000;

/// Decides when an alert is due. Cheap enough to run on every sampler tick: two comparisons.
#[derive(Debug, Default)]
pub struct NotifyPolicy {
    pressure_since: Option<u64>,
    last_tick: Option<u64>,
    last_memory: Option<u64>,
    last_disk: Option<u64>,
}

fn cooled(last: &mut Option<u64>, now_ms: u64) -> bool {
    if last.is_some_and(|l| now_ms.checked_sub(l).is_some_and(|d| d < REPEAT_MS)) {
        return false;
    }
    *last = Some(now_ms);
    true
}

impl NotifyPolicy {
    pub fn tick(&mut self, s: &SystemSample, now_ms: u64, enabled: bool) -> Vec<Alert> {
        let mut due = Vec::new();
        match self.last_tick.replace(now_ms) {
            // The clock moved back: forget the pressure run and count repeat waits from now.
            Some(prev) if now_ms < prev => {
                self.pressure_since = None;
                for last in [&mut self.last_memory, &mut self.last_disk] {
                    *last = last.map(|l| l.min(now_ms));
                }
            }
            Some(prev) if now_ms - prev > MAX_TICK_GAP_MS => self.pressure_since = None,
            _ => {}
        }
        if !enabled {
            // Turning alerts back on starts the 30 seconds then, not from an old reading.
            self.pressure_since = None;
            return due;
        }
        if s.pressure == Pressure::Normal {
            self.pressure_since = None;
        } else {
            let since = *self.pressure_since.get_or_insert(now_ms);
            if now_ms.saturating_sub(since) >= PRESSURE_FOR_MS && cooled(&mut self.last_memory, now_ms) {
                due.push(Alert::MemoryPressure(s.pressure));
            }
        }
        // A disk that could not be read reports zeros; that is not a full disk.
        if s.disk_total > 0 && s.disk_free < LOW_DISK_BYTES && cooled(&mut self.last_disk, now_ms) {
            due.push(Alert::LowDisk);
        }
        due
    }
}

/// At most one notification thread per kind, so a notification nobody answers cannot pile up threads.
#[derive(Default)]
struct Slots {
    memory: AtomicBool,
    disk: AtomicBool,
}

struct SlotGuard<'a>(&'a AtomicBool);

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Slots {
    fn acquire(&self, alert: Alert) -> Option<SlotGuard<'_>> {
        let slot = match alert {
            Alert::MemoryPressure(_) => &self.memory,
            Alert::LowDisk => &self.disk,
        };
        slot.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok().then_some(SlotGuard(slot))
    }
}

/// Runs `blocking`, and runs `on_timeout` if it is still running after `limit`. `on_timeout` must
/// make `blocking` return; the watchdog thread ends as soon as `blocking` does.
fn wait_bounded<T>(limit: Duration, blocking: impl FnOnce() -> T, on_timeout: impl FnOnce() + Send + 'static) -> T {
    let (done, finished) = mpsc::channel::<()>();
    thread::spawn(move || {
        if finished.recv_timeout(limit) == Err(mpsc::RecvTimeoutError::Timeout) {
            on_timeout();
        }
    });
    let result = blocking();
    drop(done);
    result
}

// ---------------------------------------------------------------- delivery

const BUNDLE_ID: &str = "local.toolslab.pantau";
/// The crate's wait for a click has no timeout of its own: a banner nobody touches sits in
/// Notification Center and the wait never returns. After this long the alert is taken back.
const WAIT_LIMIT: Duration = Duration::from_secs(5 * 60);

static SLOTS: LazyLock<Slots> = LazyLock::new(Slots::default);
static APPLICATION_SET: OnceLock<bool> = OnceLock::new();

/// (title, message). Short and plain: what happened, and where to look.
fn copy(alert: Alert) -> (&'static str, &'static str) {
    match alert {
        Alert::MemoryPressure(Pressure::Critical) => ("Tekanan memori tinggi", "Buka Pantau untuk melihat apa yang memakainya."),
        Alert::MemoryPressure(_) => ("Tekanan memori meningkat", "Buka Pantau untuk melihat apa yang memakainya."),
        Alert::LowDisk => ("Ruang disk hampir habis", "Sisa kurang dari 20 GB. Buka Pantau untuk melihat yang bisa dibersihkan."),
    }
}

/// Shows the alert from its own thread and opens the panel if the owner clicks it. Does nothing
/// when this kind is still waiting on an earlier alert.
pub fn deliver(app: &AppHandle, alert: Alert) {
    let Some(slot) = SLOTS.acquire(alert) else { return };
    let app = app.clone();
    let spawned = thread::Builder::new().name(format!("pantau-notify-{alert:?}")).spawn(move || {
        let _slot = slot;
        let ready = *APPLICATION_SET.get_or_init(|| match mac_notification_sys::set_application(BUNDLE_ID) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("[pantau] notifikasi tidak bisa dikirim: {e}");
                false
            }
        });
        if !ready {
            return;
        }
        let (title, message) = copy(alert);
        let sent = wait_bounded(
            WAIT_LIMIT,
            || mac_notification_sys::Notification::new().title(title).message(message).default_sound().wait_for_click(true).send(),
            move || remove_delivered(title),
        );
        #[cfg(feature = "notify-test")]
        eprintln!("[pantau-notify-test] notifikasi selesai: {sent:?}");
        match sent {
            Ok(mac_notification_sys::NotificationResponse::Click) => {
                let main = app.clone();
                let _ = app.run_on_main_thread(move || {
                    if !panel::is_open(&main) {
                        panel::toggle(&main);
                    }
                });
            }
            Ok(_) => {}
            Err(e) => eprintln!("[pantau] notifikasi gagal: {e}"),
        }
    });
    if let Err(e) = spawned {
        eprintln!("[pantau] thread notifikasi tidak bisa dibuat: {e}");
    }
}

/// Test build only: one real low-disk alert 5 s after launch, through `deliver`, so the banner and
/// the click handler can be checked on the real app.
#[cfg(feature = "notify-test")]
pub fn start_test(app: AppHandle) {
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(5));
        eprintln!("[pantau-notify-test] mengirim notifikasi uji");
        deliver(&app, Alert::LowDisk);
    });
}

/// Takes our delivered notification with this title out of Notification Center. The crate sees it
/// gone and its wait returns. Only ours: another kind's alert keeps its own banner.
fn remove_delivered(title: &str) {
    // SAFETY: plain Objective-C messages to Foundation objects that are alive for the call;
    // `deliveredNotifications` is a copy, so removing while looping is fine.
    unsafe {
        let Some(class) = AnyClass::get(c"NSUserNotificationCenter") else { return };
        let center: *mut AnyObject = msg_send![class, defaultUserNotificationCenter];
        if center.is_null() {
            return;
        }
        let delivered: *mut AnyObject = msg_send![center, deliveredNotifications];
        if delivered.is_null() {
            return;
        }
        let count: usize = msg_send![delivered, count];
        for i in 0..count {
            let item: *mut AnyObject = msg_send![delivered, objectAtIndex: i];
            let item_title: *mut AnyObject = msg_send![item, title];
            if item_title.is_null() {
                continue;
            }
            let text: *const c_char = msg_send![item_title, UTF8String];
            if !text.is_null() && CStr::from_ptr(text).to_str() == Ok(title) {
                let _: () = msg_send![center, removeDeliveredNotification: item];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;
    const MIN: u64 = 60_000;
    const WARNING: Alert = Alert::MemoryPressure(Pressure::Warning);

    fn sample(pressure: Pressure, free: u64) -> SystemSample {
        SystemSample {
            at_ms: 0,
            cpu_pct: 1.0,
            core_count: 8,
            ram_used: 1,
            ram_total: 2,
            swap_used: 0,
            swap_total: 0,
            pressure,
            disk_used: 500 * GIB - free,
            disk_total: 500 * GIB,
            disk_free: free,
        }
    }
    fn warn() -> SystemSample {
        sample(Pressure::Warning, 100 * GIB)
    }
    fn calm() -> SystemSample {
        sample(Pressure::Normal, 100 * GIB)
    }

    /// Ticks every 2 s like the sampler, and returns the alerts with the time they fired.
    fn run(p: &mut NotifyPolicy, s: &SystemSample, from: u64, to: u64) -> Vec<(u64, Alert)> {
        (from..=to).step_by(2_000).flat_map(|t| p.tick(s, t, true).into_iter().map(move |a| (t, a))).collect()
    }

    #[test]
    fn pressure_alert_fires_after_30_seconds_at_level_2_or_more() {
        let mut p = NotifyPolicy::default();
        assert_eq!(run(&mut p, &warn(), 0, 40_000), vec![(30_000, WARNING)]);
        let mut q = NotifyPolicy::default();
        assert_eq!(run(&mut q, &sample(Pressure::Critical, 100 * GIB), 0, 40_000), vec![(30_000, Alert::MemoryPressure(Pressure::Critical))]);
    }

    #[test]
    fn a_dip_to_normal_restarts_the_30_seconds() {
        let mut p = NotifyPolicy::default();
        assert!(run(&mut p, &warn(), 0, 20_000).is_empty());
        assert!(run(&mut p, &calm(), 22_000, 22_000).is_empty());
        assert_eq!(run(&mut p, &warn(), 24_000, 60_000), vec![(54_000, WARNING)]);
    }

    #[test]
    fn low_disk_fires_below_20_gb_only() {
        let mut p = NotifyPolicy::default();
        assert!(p.tick(&sample(Pressure::Normal, 20 * GIB), 0, true).is_empty());
        assert_eq!(p.tick(&sample(Pressure::Normal, 20 * GIB - 1), 2_000, true), vec![Alert::LowDisk]);
    }

    #[test]
    fn an_unreadable_disk_is_not_a_full_disk() {
        let mut p = NotifyPolicy::default();
        let mut s = sample(Pressure::Normal, 0);
        s.disk_total = 0;
        assert!(p.tick(&s, 0, true).is_empty());
    }

    #[test]
    fn each_alert_waits_15_minutes_before_repeating() {
        let mut p = NotifyPolicy::default();
        let low = sample(Pressure::Normal, GIB);
        assert_eq!(p.tick(&low, 0, true), vec![Alert::LowDisk]);
        assert!(run(&mut p, &low, 2_000, 15 * MIN - 2_000).is_empty());
        assert_eq!(run(&mut p, &low, 15 * MIN, 15 * MIN), vec![(15 * MIN, Alert::LowDisk)]);
    }

    #[test]
    fn the_two_kinds_have_separate_timers_and_can_fire_together() {
        let mut p = NotifyPolicy::default();
        let both = sample(Pressure::Warning, GIB);
        let fired = run(&mut p, &both, 0, 15 * MIN + 30_000);
        assert_eq!(fired, vec![(0, Alert::LowDisk), (30_000, WARNING), (15 * MIN, Alert::LowDisk), (15 * MIN + 30_000, WARNING)]);
    }

    #[test]
    fn nothing_fires_when_disabled_and_enabling_later_starts_the_clock_then() {
        let mut p = NotifyPolicy::default();
        let low = sample(Pressure::Warning, GIB);
        for t in (0..=60_000).step_by(2_000) {
            assert!(p.tick(&low, t, false).is_empty());
        }
        assert_eq!(p.tick(&low, 62_000, true), vec![Alert::LowDisk]);
        assert_eq!(run(&mut p, &low, 64_000, 100_000), vec![(92_000, WARNING)], "30 s counted from enabling");
    }

    #[test]
    fn pressure_before_sleep_and_after_wake_is_not_30_seconds_in_a_row() {
        let mut p = NotifyPolicy::default();
        assert!(run(&mut p, &warn(), 0, 20_000).is_empty());
        let wake = 3 * 60 * MIN;
        assert!(p.tick(&warn(), wake, true).is_empty(), "two samples hours apart are not 30 s");
        assert_eq!(run(&mut p, &warn(), wake + 2_000, wake + 40_000), vec![(wake + 30_000, WARNING)]);
    }

    #[test]
    fn a_stalled_sampler_also_restarts_the_30_seconds() {
        let mut p = NotifyPolicy::default();
        assert!(run(&mut p, &warn(), 0, 10_000).is_empty());
        assert!(p.tick(&warn(), 40_000, true).is_empty(), "30 s apart, but nothing was seen in between");
    }

    #[test]
    fn a_clock_that_moves_back_does_not_hold_alerts_until_it_catches_up() {
        let mut p = NotifyPolicy::default();
        let both = sample(Pressure::Warning, GIB);
        let start = 10 * 60 * MIN;
        let fired = run(&mut p, &both, start, start + 30_000);
        assert_eq!(fired, vec![(start, Alert::LowDisk), (start + 30_000, WARNING)]);
        // The clock is set back by an hour.
        let back = start - 60 * MIN;
        assert!(run(&mut p, &both, back, back + 28_000).is_empty(), "no repeat right after the jump");
        assert!(run(&mut p, &both, back + 30_000, back + 15 * MIN - 2_000).is_empty());
        let again = run(&mut p, &both, back + 15 * MIN, back + 15 * MIN);
        assert_eq!(again, vec![(back + 15 * MIN, WARNING), (back + 15 * MIN, Alert::LowDisk)], "repeat 15 min after the jump, not an hour later");
    }

    #[test]
    fn the_title_matches_the_level_the_banner_shows() {
        assert_eq!(copy(alert_for(Pressure::Warning)).0, "Tekanan memori meningkat");
        assert_eq!(copy(alert_for(Pressure::Critical)).0, "Tekanan memori tinggi");
    }

    fn alert_for(p: Pressure) -> Alert {
        let mut policy = NotifyPolicy::default();
        run(&mut policy, &sample(p, 100 * GIB), 0, 40_000)[0].1
    }

    #[test]
    fn a_kind_has_one_slot_until_the_guard_is_dropped() {
        let slots = Slots::default();
        let first = slots.acquire(WARNING).expect("free");
        assert!(slots.acquire(WARNING).is_none());
        assert!(slots.acquire(Alert::LowDisk).is_some(), "the other kind is independent");
        drop(first);
        assert!(slots.acquire(WARNING).is_some());
    }

    #[test]
    fn a_wait_that_never_ends_is_released_after_the_limit() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let started = std::time::Instant::now();
        let got = wait_bounded(Duration::from_millis(50), || rx.recv().map(|_| "woken"), move || tx.send(()).unwrap());
        assert_eq!(got, Ok("woken"));
        assert!(started.elapsed() >= Duration::from_millis(50) && started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_wait_that_ends_early_never_runs_the_timeout() {
        let fired = std::sync::Arc::new(AtomicBool::new(false));
        let flag = fired.clone();
        assert_eq!(wait_bounded(Duration::from_millis(50), || 7, move || flag.store(true, Ordering::SeqCst)), 7);
        std::thread::sleep(Duration::from_millis(150));
        assert!(!fired.load(Ordering::SeqCst));
    }
}
