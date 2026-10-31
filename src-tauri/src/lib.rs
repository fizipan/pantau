#[cfg(feature = "measure")]
mod measure;
pub mod commands;
pub mod errors;
pub mod panel;
pub mod process;
pub mod notify;
pub mod sampler;
pub mod settings;
pub mod shortcut;
pub mod storage;
pub mod system;
pub mod tray;
#[cfg(test)]
mod ts_fixtures;

use std::sync::{Arc, Mutex};

use tauri::{ActivationPolicy, Manager};

use crate::storage::Paths;
use crate::storage::scheduler::Scheduler;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            app.set_activation_policy(ActivationPolicy::Accessory);
            tray::setup(app)?;
            app.manage(commands::OpStore::new());
            app.manage(commands::IconCache::default());
            // Settings first: `Paths::current()` reads the saved folders.
            let saved = settings::load(&app.path().app_data_dir()?.join("settings.json"));
            settings::set_active(&saved);
            let mut status = shortcut::Status::default();
            if let Err(e) = shortcut::apply(&shortcut::System(app.handle()), &mut status, &saved.shortcut) {
                eprintln!("[pantau] {e}");
                status.error = Some(e);
            }
            app.manage(commands::ShortcutState(Mutex::new(status)));
            let cache_path = app.path().app_data_dir()?.join("storage-cache.json");
            let handle = app.handle().clone();
            app.manage(Scheduler::start(Paths::current(), cache_path, Arc::new(system::read_pressure), Arc::new(move || panel::is_open(&handle))));
            sampler::spawn_sampler(app.handle().clone());
            #[cfg(feature = "notify-test")]
            notify::start_test(app.handle().clone());
            #[cfg(feature = "measure")]
            measure::start(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::plan_kill,
            commands::start_kill,
            commands::force_kill,
            commands::get_storage,
            commands::refresh_storage,
            commands::scan_recent_now,
            commands::clean_item,
            commands::reveal_in_finder,
            commands::open_activity_monitor,
            commands::open_privacy_settings,
            commands::open_settings,
            commands::app_icon,
            commands::get_settings,
            commands::save_settings,
            commands::pick_scan_folder,
            commands::close_settings,
            panel::panel_ready,
            panel::close_panel,
        ])
        // No window is declared, so closing the last one must not quit the app.
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            if let tauri::RunEvent::ExitRequested { api, code: None, .. } = event {
                api.prevent_exit();
            }
        });
}

#[cfg(test)]
mod tests {
    #[test]
    fn no_window_is_declared_so_no_webview_exists_at_startup() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["identifier"], "local.toolslab.pantau");
        assert_eq!(conf["app"]["windows"].as_array().unwrap().len(), 0);
        assert_eq!(conf["build"]["devUrl"], "http://localhost:1430");
    }

    #[test]
    fn the_webviews_get_only_the_permissions_the_ui_uses_and_cannot_embed_or_post_elsewhere() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let csp = conf["app"]["security"]["csp"].as_str().unwrap();
        for directive in ["base-uri 'none'", "form-action 'none'", "object-src 'none'", "frame-src 'none'"] {
            assert!(csp.contains(directive), "{directive} missing from {csp}");
        }
        let caps: serde_json::Value = serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        // The UI calls `invoke` (app commands need no permission) and `listen`/`unlisten`, nothing else.
        assert_eq!(caps["permissions"], serde_json::json!(["core:event:allow-listen", "core:event:allow-unlisten"]));
        assert_eq!(conf["build"]["devUrl"], format!("http://localhost:{}", crate::panel::DEV_PORT));
    }
}
