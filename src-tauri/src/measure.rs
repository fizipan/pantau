//! Only compiled with `--features measure`. Opens and closes the panel on a timer and prints phase
//! markers to stderr, so the memory of each phase can be sampled from outside without clicking.

use std::thread::sleep;
use std::time::Duration;

use tauri::AppHandle;

use crate::panel;

fn secs(n: u64) {
    sleep(Duration::from_secs(n));
}

fn on_main(app: &AppHandle, f: impl FnOnce(&AppHandle) + Send + 'static) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || f(&handle));
}

fn set_open(app: &AppHandle, open: bool) {
    on_main(app, move |a| {
        if panel::is_open(a) != open {
            panel::toggle(a);
        }
    });
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        eprintln!("[pantau-measure] phase=closed-never-opened");
        secs(60);
        for n in 1..=5 {
            eprintln!("[pantau-measure] phase=open n={n}");
            set_open(&app, true);
            secs(8);
            eprintln!("[pantau-measure] phase=close n={n}");
            set_open(&app, false);
            secs(12);
            eprintln!("[pantau-measure] phase=closed+12s n={n}");
            secs(4);
        }
        for n in 1..=20 {
            set_open(&app, true);
            secs(2);
            set_open(&app, false);
            secs(1);
            eprintln!("[pantau-measure] phase=cycle n={n}");
        }
        eprintln!("[pantau-measure] phase=after-20-closing");
        secs(15);
        eprintln!("[pantau-measure] phase=after-20-closed+15s");
        secs(60);
        eprintln!("[pantau-measure] phase=done");
    });
}
