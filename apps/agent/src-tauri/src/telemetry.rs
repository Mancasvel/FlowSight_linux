//! Transient foreground samples for opt-in local focus reminders.
//! No keystrokes, titles, or task text are persisted by this module.

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex,
};
use std::time::Duration;

static ENABLED: AtomicBool = AtomicBool::new(false);
static SELECTED_TASK: Mutex<Option<String>> = Mutex::new(None);
static TRACKING_GENERATION: AtomicU64 = AtomicU64::new(0);

struct CaptureCadence {
    generation: Option<u64>,
    due: std::time::Instant,
    observed: std::time::Instant,
}
impl CaptureCadence {
    fn new(now: std::time::Instant) -> Self {
        Self {
            generation: None,
            due: now,
            observed: now,
        }
    }
    fn take(
        &mut self,
        enabled: bool,
        generation: u64,
        now: std::time::Instant,
        interval: Duration,
    ) -> bool {
        if !enabled {
            return false;
        }
        if self.generation != Some(generation) {
            self.generation = Some(generation);
            self.due = now;
            self.observed = now;
        }
        if now < self.due {
            return false;
        }
        self.due = now + interval;
        true
    }
    fn elapsed_seconds(&mut self, now: std::time::Instant) -> u64 {
        let elapsed = now
            .saturating_duration_since(self.observed)
            .as_secs()
            .min(300);
        self.observed = now;
        elapsed
    }
}

fn current_capture(enabled: bool, current_generation: u64, capture_generation: u64) -> bool {
    enabled && current_generation == capture_generation
}

pub fn set_enabled(enabled: bool) {
    TRACKING_GENERATION.fetch_add(1, Ordering::Relaxed);
    ENABLED.store(enabled, Ordering::Relaxed);
    if !enabled {
        record_selected_task(None);
    }
}

pub fn set_running(enabled: bool) {
    set_enabled(enabled);
}

pub fn set_task_context(user_task: Option<String>, jira_ticket: Option<String>) {
    record_selected_task(jira_ticket.as_deref().or(user_task.as_deref()));
}

pub fn refresh_privacy_filter() {
    let foreground = crate::context::get_system_context();
    if let Ok(path) = crate::paths::db_path() {
        if crate::privacy::application_is_excluded(&path, foreground.app_name.as_deref()) {
            crate::focus_alerts::excluded_app_entered();
        }
    }
}

pub(crate) fn capture_is_current(generation: u64) -> bool {
    current_capture(
        ENABLED.load(Ordering::Relaxed),
        TRACKING_GENERATION.load(Ordering::Relaxed),
        generation,
    )
}

pub fn record_selected_task(task: Option<&str>) {
    if let Ok(mut value) = SELECTED_TASK.lock() {
        *value = task
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.chars().take(140).collect());
    }
}

pub fn selected_task_for_reminder() -> Option<String> {
    SELECTED_TASK.lock().ok().and_then(|value| value.clone())
}

pub fn start(app: tauri::AppHandle) {
    start_capture_loop(app.clone());
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(2));
        if !ENABLED.load(Ordering::Relaxed) {
            continue;
        }
        let foreground = crate::context::get_system_context();
        let Ok(path) = crate::paths::db_path() else {
            continue;
        };
        if crate::privacy::application_is_excluded(&path, foreground.app_name.as_deref()) {
            crate::focus_alerts::excluded_app_entered();
            continue;
        }
        if let Some(name) = foreground.app_name {
            crate::focus_alerts::record_app_switch(&app, &name);
        }
    });
}

/// Keep the Linux PipeWire/portal capture on Tauri's async runtime. The frontend
/// only starts/stops tracking, and this one loop serializes actual captures.
fn start_capture_loop(app: tauri::AppHandle) {
    use tauri::Manager;
    std::thread::spawn(move || {
        let mut cadence = CaptureCadence::new(std::time::Instant::now());
        loop {
            std::thread::sleep(Duration::from_millis(250));
            if !ENABLED.load(Ordering::Relaxed) {
                continue;
            }
            let generation = TRACKING_GENERATION.load(Ordering::Relaxed);
            let state = app.state::<crate::agent::AgentState>();
            let interval = state
                .lock()
                .ok()
                .and_then(|guard| {
                    guard
                        .as_ref()
                        .and_then(|agent| agent.config.capture_interval)
                })
                .unwrap_or(60_000)
                .clamp(5_000, 300_000);
            if !cadence.take(
                ENABLED.load(Ordering::Relaxed),
                generation,
                std::time::Instant::now(),
                Duration::from_millis(interval),
            ) {
                continue;
            }
            let task = selected_task_for_reminder();
            let outcome = tauri::async_runtime::block_on(crate::agent::capture_context_snapshot(
                app.clone(),
                state,
                task,
                None,
            ));
            let elapsed = cadence.elapsed_seconds(std::time::Instant::now());
            match outcome {
                Ok(snapshot) => {
                    if let Err(error) =
                        crate::agent::save_native_snapshot(&app, &snapshot, generation, elapsed)
                    {
                        log::warn!("[Capture] Could not save native observation: {error}");
                    }
                }
                Err(error) => log::warn!("[Capture] Native observation skipped: {error}"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_capture_per_interval_and_resume_starts_a_new_cycle() {
        let now = std::time::Instant::now();
        let interval = Duration::from_secs(60);
        let mut cadence = CaptureCadence::new(now);
        assert!(!cadence.take(false, 0, now, interval));
        assert!(cadence.take(true, 1, now, interval));
        assert!(!cadence.take(true, 1, now + Duration::from_secs(20), interval));
        assert!(cadence.take(true, 1, now + interval, interval));
        assert!(!cadence.take(true, 1, now + interval, interval));
        assert!(!cadence.take(false, 2, now + interval, interval));
        assert!(cadence.take(true, 3, now + interval, interval));
    }
    #[test]
    fn pause_stop_and_restart_reject_in_flight_results() {
        assert!(current_capture(true, 1, 1));
        assert!(!current_capture(false, 1, 1));
        assert!(!current_capture(false, 2, 1));
        assert!(!current_capture(true, 3, 1));
        assert!(current_capture(true, 3, 3));
    }
    #[test]
    fn slow_capture_does_not_queue_a_backlog() {
        let now = std::time::Instant::now();
        let interval = Duration::from_secs(60);
        let mut cadence = CaptureCadence::new(now);
        assert!(cadence.take(true, 1, now, interval));
        let completed = now + Duration::from_secs(200);
        assert!(cadence.take(true, 1, completed, interval));
        assert!(!cadence.take(true, 1, completed, interval));
        assert!(!cadence.take(true, 1, completed + Duration::from_secs(59), interval));
    }
    #[test]
    fn duration_uses_elapsed_tracking_time_and_discards_paused_time() {
        let now = std::time::Instant::now();
        let mut cadence = CaptureCadence::new(now);
        assert!(cadence.take(true, 1, now, Duration::from_secs(60)));
        assert_eq!(cadence.elapsed_seconds(now + Duration::from_secs(2)), 2);
        assert_eq!(cadence.elapsed_seconds(now + Duration::from_secs(62)), 60);
        assert!(cadence.take(
            true,
            3,
            now + Duration::from_secs(600),
            Duration::from_secs(60)
        ));
        assert_eq!(cadence.elapsed_seconds(now + Duration::from_secs(603)), 3);
    }
}
