//! Transient foreground samples for opt-in local focus reminders.
//! No keystrokes, titles, or task text are persisted by this module.

use std::sync::{atomic::{AtomicBool, Ordering}, Mutex};
use std::time::Duration;

static ENABLED: AtomicBool = AtomicBool::new(false);
static SELECTED_TASK: Mutex<Option<String>> = Mutex::new(None);

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
    if !enabled { record_selected_task(None); }
}

pub fn record_selected_task(task: Option<&str>) {
    if let Ok(mut value) = SELECTED_TASK.lock() {
        *value = task.filter(|value| !value.trim().is_empty()).map(|value| value.chars().take(140).collect());
    }
}

pub fn selected_task_for_reminder() -> Option<String> {
    SELECTED_TASK.lock().ok().and_then(|value| value.clone())
}

pub fn start(app: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(2));
        if !ENABLED.load(Ordering::Relaxed) { continue; }
        let foreground = crate::context::get_system_context();
        let Ok(path) = crate::paths::db_path() else { continue; };
        if crate::privacy::application_is_excluded(&path, foreground.app_name.as_deref()) {
            crate::focus_alerts::excluded_app_entered();
            continue;
        }
        if let Some(name) = foreground.app_name {
            crate::focus_alerts::record_app_switch(&app, &name);
        }
    });
}
