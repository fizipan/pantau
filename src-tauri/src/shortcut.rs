//! The global shortcut that toggles the panel. A new one is registered before the old one is
//! released, so a shortcut that cannot be registered leaves the old one working.

use crate::settings::validate_shortcut;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// What is registered with the system right now.
    pub registered: Option<String>,
    /// Why the saved shortcut could not be registered at startup. Cleared by the next success.
    pub error: Option<String>,
}

pub trait Registrar {
    fn register(&self, shortcut: &str) -> Result<(), String>;
    fn unregister(&self, shortcut: &str);
}

/// Makes `wanted` the registered shortcut. On any failure nothing changes.
pub fn apply(reg: &dyn Registrar, status: &mut Status, wanted: &str) -> Result<(), String> {
    validate_shortcut(wanted)?;
    if status.registered.as_deref() == Some(wanted) {
        status.error = None;
        return Ok(());
    }
    reg.register(wanted).map_err(|why| format!("Pintasan tidak bisa didaftarkan: {why}"))?;
    if let Some(old) = status.registered.replace(wanted.to_string()) {
        reg.unregister(&old);
    }
    status.error = None;
    Ok(())
}

/// For a save: a stored shortcut that failed at startup must not lock the other settings. When
/// `wanted` is what is already stored and is not registered, try once more and carry on either
/// way; `status.error` stays until a registration works.
pub fn apply_for_save(reg: &dyn Registrar, status: &mut Status, wanted: &str, stored: &str) -> Result<(), String> {
    if wanted == stored && status.registered.as_deref() != Some(wanted) {
        let _ = apply(reg, status, wanted);
        return Ok(());
    }
    apply(reg, status, wanted)
}

/// What the owner reads when the system refuses a key. The system only says "failed", in English,
/// and does not say who else holds the key, so this does not claim to know. Whatever the raw
/// error was, the text is the same.
pub fn registration_failure(_raw: &tauri_plugin_global_shortcut::Error) -> String {
    "macOS menolak kombinasi ini. Coba kombinasi lain.".into()
}

/// The real system registration. The handler only reacts to the press, not the release.
pub struct System<'a>(pub &'a tauri::AppHandle);

impl Registrar for System<'_> {
    fn register(&self, shortcut: &str) -> Result<(), String> {
        use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
        self.0
            .global_shortcut()
            .on_shortcut(shortcut, |app, _, event| {
                if event.state == ShortcutState::Pressed {
                    crate::panel::toggle(app);
                }
            })
            .map_err(|e| {
                // The owner's text hides the raw error, so it goes to stderr.
                eprintln!("[pantau] shortcut registration failed: {e}");
                registration_failure(&e)
            })
    }

    fn unregister(&self, shortcut: &str) {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        let _ = self.0.global_shortcut().unregister(shortcut);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[derive(Default)]
    struct Fake {
        taken_elsewhere: Vec<&'static str>,
        held: RefCell<Vec<String>>,
        calls: RefCell<Vec<String>>,
    }
    impl Registrar for Fake {
        fn register(&self, s: &str) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("register {s}"));
            if self.taken_elsewhere.contains(&s) {
                return Err("sudah dipakai".into());
            }
            self.held.borrow_mut().push(s.into());
            Ok(())
        }
        fn unregister(&self, s: &str) {
            self.calls.borrow_mut().push(format!("unregister {s}"));
            self.held.borrow_mut().retain(|h| h != s);
        }
    }

    fn status(s: &str) -> Status {
        Status { registered: Some(s.into()), error: None }
    }

    #[test]
    fn a_new_shortcut_is_registered_before_the_old_one_is_released() {
        let reg = Fake::default();
        reg.held.borrow_mut().push("Ctrl+Alt+M".into());
        let mut st = status("Ctrl+Alt+M");
        apply(&reg, &mut st, "Ctrl+Alt+P").unwrap();
        assert_eq!(*reg.calls.borrow(), ["register Ctrl+Alt+P", "unregister Ctrl+Alt+M"]);
        assert_eq!(*reg.held.borrow(), ["Ctrl+Alt+P"]);
        assert_eq!(st, status("Ctrl+Alt+P"));
    }

    #[test]
    fn a_shortcut_that_cannot_be_registered_keeps_the_old_one_and_says_why() {
        let reg = Fake { taken_elsewhere: vec!["Ctrl+Alt+P"], ..Fake::default() };
        reg.held.borrow_mut().push("Ctrl+Alt+M".into());
        let mut st = status("Ctrl+Alt+M");
        let err = apply(&reg, &mut st, "Ctrl+Alt+P").unwrap_err();
        assert!(err.starts_with("Pintasan") && err.contains("sudah dipakai"), "{err}");
        assert_eq!(*reg.held.borrow(), ["Ctrl+Alt+M"], "the old shortcut is untouched");
        assert_eq!(st, status("Ctrl+Alt+M"));
    }

    #[test]
    fn an_invalid_shortcut_is_refused_before_the_system_is_asked() {
        let reg = Fake::default();
        let mut st = status("Ctrl+Alt+M");
        assert!(apply(&reg, &mut st, "M").is_err());
        assert!(reg.calls.borrow().is_empty());
    }

    #[test]
    fn the_same_shortcut_is_not_registered_twice() {
        let reg = Fake::default();
        let mut st = status("Ctrl+Alt+M");
        apply(&reg, &mut st, "Ctrl+Alt+M").unwrap();
        assert!(reg.calls.borrow().is_empty());
    }

    #[test]
    fn a_startup_failure_is_kept_until_a_later_success_and_a_retry_of_the_same_key_is_made() {
        let reg = Fake { taken_elsewhere: vec!["Ctrl+Alt+M"], ..Fake::default() };
        let mut st = Status::default();
        let err = apply(&reg, &mut st, "Ctrl+Alt+M").unwrap_err();
        st.error = Some(err.clone());
        assert_eq!(st.registered, None);
        assert!(apply(&reg, &mut st, "Ctrl+Alt+M").is_err(), "retried, since nothing is registered");
        assert_eq!(st.error, Some(err), "a failed retry changes nothing");
        apply(&reg, &mut st, "Ctrl+Alt+P").unwrap();
        assert_eq!(st, status("Ctrl+Alt+P"), "success clears the error");
    }

    #[test]
    fn a_stored_shortcut_that_failed_at_startup_does_not_block_a_save_of_the_same_shortcut() {
        let reg = Fake { taken_elsewhere: vec!["Ctrl+Alt+M"], ..Fake::default() };
        let mut st = Status { registered: None, error: Some("Pintasan tidak bisa didaftarkan: x".into()) };
        assert_eq!(apply_for_save(&reg, &mut st, "Ctrl+Alt+M", "Ctrl+Alt+M"), Ok(()));
        assert_eq!(st.error.as_deref(), Some("Pintasan tidak bisa didaftarkan: x"), "the error stays");
        assert_eq!(st.registered, None);
    }

    #[test]
    fn a_save_retries_the_stored_shortcut_and_clears_the_error_when_it_works_now() {
        let reg = Fake::default();
        let mut st = Status { registered: None, error: Some("x".into()) };
        apply_for_save(&reg, &mut st, "Ctrl+Alt+M", "Ctrl+Alt+M").unwrap();
        assert_eq!(st, status("Ctrl+Alt+M"));
    }

    #[test]
    fn a_save_with_a_different_shortcut_still_refuses_when_it_cannot_register() {
        let reg = Fake { taken_elsewhere: vec!["Ctrl+Alt+P"], ..Fake::default() };
        let mut st = Status { registered: None, error: Some("x".into()) };
        assert!(apply_for_save(&reg, &mut st, "Ctrl+Alt+P", "Ctrl+Alt+M").is_err());
        assert_eq!(st.error.as_deref(), Some("x"));
    }

    #[test]
    fn the_real_system_errors_become_indonesian_without_claiming_a_conflict() {
        use tauri_plugin_global_shortcut::{Error, Shortcut};
        // One error from the plugin's own parser, the rest shaped like what the OS layer reports.
        let parse = Error::from("not a key".parse::<Shortcut>().unwrap_err());
        let raws = [
            parse,
            Error::GlobalHotkey("Unable to register hotkey: HotKey { mods: Modifiers(ALT), key: KeyM }".into()),
            Error::GlobalHotkey("HotKey already registered: Ctrl+Alt+M".into()),
            Error::GlobalHotkey("failed to register".into()),
        ];
        for raw in raws {
            let msg = registration_failure(&raw);
            assert_eq!(msg, "macOS menolak kombinasi ini. Coba kombinasi lain.", "from {raw}");
            assert!(!msg.contains("sudah dipakai"), "the system does not tell who holds a key: {msg}");
        }
    }
}
