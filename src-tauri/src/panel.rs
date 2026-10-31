//! The panel window lives only while it is open. Closing it destroys the webview, so the WebKit
//! processes go away with it. Nothing is created until the first tray click.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::window::{Effect, EffectState, EffectsBuilder};
use tauri::{AppHandle, LogicalPosition, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

use crate::{sampler, tray};

pub const PANEL: &str = "panel";
pub const SETTINGS: &str = "settings";

/// A tray click this soon after the panel destroyed itself on blur is the click that closed it.
const BLUR_CLICK_GUARD: Duration = Duration::from_millis(200);

static LAST_BLUR: Mutex<Option<Instant>> = Mutex::new(None);
#[cfg(feature = "measure")]
static OPEN_REQUESTED: Mutex<Option<Instant>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn is_open(app: &AppHandle) -> bool {
    app.get_webview_window(PANEL).is_some()
}

pub fn toggle(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(PANEL) {
        let _ = win.destroy();
        return;
    }
    if lock(&LAST_BLUR).is_some_and(|t| t.elapsed() < BLUR_CLICK_GUARD) {
        return;
    }
    #[cfg(feature = "measure")]
    {
        *lock(&OPEN_REQUESTED) = Some(Instant::now());
    }
    if let Err(e) = open(app) {
        eprintln!("[pantau] cannot open panel: {e}");
    }
}

fn open(app: &AppHandle) -> tauri::Result<()> {
    let win = WebviewWindowBuilder::new(app, PANEL, WebviewUrl::App("index.html".into()))
        .title("Pantau")
        .inner_size(360.0, 640.0)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .resizable(false)
        .visible(false)
        .on_navigation(|url| navigation_allowed(url, cfg!(debug_assertions)))
        .build()?;
    win.set_effects(EffectsBuilder::new().effect(Effect::Popover).state(EffectState::Active).radius(16.0).build())?;
    let handle = app.clone();
    win.on_window_event(move |e| {
        if let WindowEvent::Focused(false) = e {
            *lock(&LAST_BLUR) = Some(Instant::now());
            if let Some(win) = handle.get_webview_window(PANEL) {
                let _ = win.destroy();
            }
        }
    });
    Ok(())
}

/// The windows load only the app's own pages. A link or a script that tries to take a window
/// elsewhere is refused, so a page from the internet never gets the app's commands. The dev server
/// is allowed in debug builds only.
fn navigation_allowed(url: &tauri::Url, dev: bool) -> bool {
    let own = url.scheme() == "tauri" && url.host_str() == Some("localhost");
    let dev_server = dev && url.scheme() == "http" && url.host_str() == Some("localhost") && url.port() == Some(DEV_PORT);
    own || dev_server
}

/// `build.devUrl` in `tauri.conf.json`; a test there keeps the two equal.
pub const DEV_PORT: u16 = 1430;

/// Called by the frontend once it has mounted. Only now is the window shown, so the user never sees an empty frame.
#[tauri::command]
pub fn panel_ready(app: AppHandle) {
    let Some(win) = app.get_webview_window(PANEL) else { return };
    if let Err(e) = place(&app, &win) {
        eprintln!("[pantau] panel tidak bisa ditempatkan: {e}");
    }
    let _ = win.show();
    let _ = win.set_focus();
    sampler::wake();
    #[cfg(feature = "measure")]
    if let Some(t) = lock(&OPEN_REQUESTED).take() {
        eprintln!("[pantau] panel siap dalam {} ms", t.elapsed().as_millis());
    }
}

#[derive(Debug, Clone, Copy)]
struct Area {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Area {
    fn contains_centre_of(&self, o: Area) -> bool {
        let (cx, cy) = (o.x + o.w / 2.0, o.y + o.h / 2.0);
        (self.x..self.x + self.w).contains(&cx) && (self.y..self.y + self.h).contains(&cy)
    }
}

/// A screen in points, the space `NSWindow` positions live in. Each screen reports pixels at its
/// own scale, so mixing the raw numbers of two screens (2x and 1x) compares unlike things.
#[derive(Debug, Clone, Copy)]
struct Display {
    area: Area,
    scale: f64,
}

impl Display {
    fn from_physical(pos: (f64, f64), size: (f64, f64), scale: f64) -> Display {
        Display { area: Area { x: pos.0 / scale, y: pos.1 / scale, w: size.0 / scale, h: size.1 / scale }, scale }
    }
}

/// The menu bar is 24 points tall, 37 on a notched screen. The tray's pixels are at its own
/// screen's scale and nothing in the rectangle says which, so the screen whose scale turns them
/// into a menu-bar-sized item is the right one when two screens would both contain it.
const MENU_BAR_POINTS: std::ops::RangeInclusive<f64> = 16.0..=40.0;

/// The screen the tray item is on, and the item in points. `tray_px` is in the pixels of the
/// tray's own screen.
fn tray_display(tray_px: Area, displays: &[Display]) -> Option<(usize, Area)> {
    let on_screen: Vec<(usize, Area)> = displays
        .iter()
        .enumerate()
        .map(|(i, d)| (i, Area { x: tray_px.x / d.scale, y: tray_px.y / d.scale, w: tray_px.w / d.scale, h: tray_px.h / d.scale }))
        .filter(|&(i, t)| displays[i].area.contains_centre_of(t))
        .collect();
    on_screen.iter().find(|(_, t)| MENU_BAR_POINTS.contains(&t.h)).or(on_screen.first()).copied()
}

/// Top-left corner for a window of size `win`: centred under the tray item, inside `monitor`. A
/// window larger than the monitor keeps its top left visible. All in points.
fn panel_origin(tray: Area, win: (f64, f64), monitor: Area) -> (f64, f64) {
    let inside = |want: f64, start: f64, room: f64, size: f64| want.min(start + room - size).max(start);
    (inside(tray.x + tray.w / 2.0 - win.0 / 2.0, monitor.x, monitor.w, win.0), inside(tray.y + tray.h, monitor.y, monitor.h, win.1))
}

/// Asks the tray where it is on every open, because the shortcut and a notification click open the
/// panel without any tray event having happened.
fn place(app: &AppHandle, win: &WebviewWindow) -> Result<(), String> {
    let rect = tray::rect(app).ok_or("posisi ikon menu bar tidak diketahui")?;
    let displays: Vec<Display> = win
        .available_monitors()
        .map_err(|e| e.to_string())?
        .iter()
        .map(|m| {
            let (p, s) = (m.position(), m.size());
            Display::from_physical((f64::from(p.x), f64::from(p.y)), (f64::from(s.width), f64::from(s.height)), m.scale_factor())
        })
        .collect();
    // The tray rectangle is physical pixels of its own screen; it is never rescaled by the panel's.
    let (pos, size) = (rect.position.to_physical::<f64>(1.0), rect.size.to_physical::<f64>(1.0));
    let tray_px = Area { x: pos.x, y: pos.y, w: size.width, h: size.height };
    let (which, tray) = tray_display(tray_px, &displays).ok_or("ikon menu bar tidak ada di monitor mana pun")?;
    // The window's own size in points, whichever screen it is on now.
    let outer = win.outer_size().map_err(|e| e.to_string())?.to_logical::<f64>(win.scale_factor().map_err(|e| e.to_string())?);
    let (x, y) = panel_origin(tray, (outer.width, outer.height), displays[which].area);
    #[cfg(feature = "measure")]
    eprintln!("[pantau] tray {tray:?} layar {which} panel di ({x}, {y}) pt");
    win.set_position(LogicalPosition::new(x, y)).map_err(|e| e.to_string())
}

/// Called by the frontend (Escape, or a button) to close the panel; destroying it frees the webview.
#[tauri::command]
pub fn close_panel(app: AppHandle) {
    if let Some(win) = app.get_webview_window(PANEL) {
        let _ = win.destroy();
    }
}

pub fn open_settings(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(SETTINGS) {
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }
    let built = WebviewWindowBuilder::new(app, SETTINGS, WebviewUrl::App("settings.html".into()))
        .title("Pengaturan Pantau")
        .inner_size(520.0, 600.0)
        .resizable(false)
        .center()
        .on_navigation(|url| navigation_allowed(url, cfg!(debug_assertions)))
        .build();
    // An accessory app is not active by default, so the new window must ask for focus itself.
    match built {
        Ok(win) => {
            let _ = win.set_focus();
        }
        Err(e) => eprintln!("[pantau] cannot open settings: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // All logical points. A 2x built-in screen and a 1x external one to its right.
    const SCREEN: Area = Area { x: 0.0, y: 0.0, w: 1440.0, h: 900.0 };
    const WIN: (f64, f64) = (360.0, 640.0);

    fn item_at(x: f64) -> Area {
        Area { x, y: 0.0, w: 80.0, h: 24.0 }
    }

    fn two_displays() -> [Display; 2] {
        [Display { area: SCREEN, scale: 2.0 }, Display { area: Area { x: 1440.0, y: 0.0, w: 1920.0, h: 1080.0 }, scale: 1.0 }]
    }

    #[test]
    fn the_panel_opens_centred_under_the_tray_item() {
        assert_eq!(panel_origin(item_at(1000.0), WIN, SCREEN), (1000.0 + 40.0 - 180.0, 24.0));
    }

    #[test]
    fn near_a_screen_edge_the_panel_stays_on_screen() {
        assert_eq!(panel_origin(item_at(1400.0), WIN, SCREEN).0, 1440.0 - 360.0);
        assert_eq!(panel_origin(item_at(10.0), WIN, SCREEN).0, 0.0);
    }

    #[test]
    fn a_second_monitor_uses_its_own_edges() {
        let right = Area { x: 1440.0, y: -100.0, w: 1920.0, h: 1080.0 };
        let item = Area { x: 3300.0, y: -100.0, w: 50.0, h: 24.0 };
        assert_eq!(panel_origin(item, WIN, right), (1440.0 + 1920.0 - 360.0, -76.0));
    }

    #[test]
    fn a_panel_taller_than_the_screen_keeps_its_top_visible() {
        let small = Area { x: 0.0, y: 0.0, w: 1280.0, h: 400.0 };
        assert_eq!(panel_origin(item_at(100.0), WIN, small).1, 0.0);
    }

    #[test]
    fn a_tray_item_on_the_external_1x_screen_is_not_taken_for_one_on_the_2x_screen() {
        // Physical pixels in the tray's own screen scale: 2500 here is also 1250 points on the 2x screen.
        let tray_px = Area { x: 2500.0, y: 0.0, w: 80.0, h: 24.0 };
        let (which, tray) = tray_display(tray_px, &two_displays()).unwrap();
        assert_eq!((which, tray.x, tray.h), (1, 2500.0, 24.0));
        let (x, y) = panel_origin(tray, WIN, two_displays()[which].area);
        assert_eq!((x, y), (2500.0 + 40.0 - 180.0, 24.0));
    }

    #[test]
    fn a_tray_item_on_the_2x_screen_is_found_in_points_not_pixels() {
        // 1000 points at 2x is 2000 pixels, which is also inside the external screen's range at 1x.
        let tray_px = Area { x: 2000.0, y: 0.0, w: 160.0, h: 48.0 };
        let (which, tray) = tray_display(tray_px, &two_displays()).unwrap();
        assert_eq!((which, tray.x, tray.w, tray.h), (0, 1000.0, 80.0, 24.0));
    }

    #[test]
    fn a_tray_item_outside_every_screen_is_not_placed() {
        assert!(tray_display(Area { x: 9000.0, y: 0.0, w: 80.0, h: 24.0 }, &two_displays()).is_none());
    }

    fn url(s: &str) -> tauri::Url {
        s.parse().unwrap()
    }

    #[test]
    fn a_window_may_only_load_the_apps_own_pages() {
        assert!(navigation_allowed(&url("tauri://localhost/index.html"), false));
        assert!(navigation_allowed(&url("tauri://localhost/settings.html"), false));
        for outside in ["https://example.com/", "http://localhost:1430/", "file:///etc/passwd", "tauri://evil/index.html", "about:blank", "javascript:alert(1)", "data:text/html,x"] {
            assert!(!navigation_allowed(&url(outside), false), "{outside}");
        }
    }

    #[test]
    fn a_debug_build_also_allows_the_dev_server_and_nothing_else_local() {
        assert!(navigation_allowed(&url("http://localhost:1430/index.html"), true));
        assert!(!navigation_allowed(&url("http://localhost:8080/"), true));
        assert!(!navigation_allowed(&url("http://localhost.evil.com:1430/"), true));
        assert!(!navigation_allowed(&url("https://example.com/"), true));
    }

    #[test]
    fn displays_are_taken_in_points_each_by_its_own_scale() {
        // Physical position and size as tao reports them: points times that screen's scale.
        let d = Display::from_physical((2880.0, -200.0), (3840.0, 2160.0), 2.0);
        assert_eq!((d.area.x, d.area.y, d.area.w, d.area.h, d.scale), (1440.0, -100.0, 1920.0, 1080.0, 2.0));
    }
}
