use crate::agent_pure::{parse_analysis, resolve_persisted_category};
use crate::focus_semantics::{canonical_ticket_value, LocalDateWindow};
use crate::vision_model::{
    CONFIG_VISION_MODEL_ID, LLAMA_CHAT_MODEL_ID, VISION_GGUF_FILENAME, VISION_MMPROJ_FILENAME,
    VISION_STATUS_LABEL,
};
use chrono::{Datelike, Local};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{Emitter, State};

pub type AgentState = Mutex<Option<FlowSightAgent>>;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ActivityReport {
    pub id: Option<i64>,
    pub timestamp: String,
    pub description: String,
    pub activity_type: String,
    pub synced: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct AgentConfig {
    #[serde(rename = "devName")]
    pub dev_name: Option<String>,
    #[serde(rename = "captureInterval")]
    pub capture_interval: Option<u64>,
    #[serde(rename = "visionModel")]
    pub vision_model: Option<String>,
    /// `None` or `-1` => automatic GPU layer ladder on local llama-server.
    /// `Some(n)` for `n >= 0` => fixed `--n-gpu-layers` (manual / power user).
    #[serde(rename = "gpuLayers")]
    pub gpu_layers: Option<i32>,
    #[serde(rename = "dailyGoalHours")]
    pub daily_goal_hours: Option<f64>,
}

pub struct FlowSightAgent {
    pub config: AgentConfig,
    pub is_running: bool,
    pub tracking_clock: Option<crate::tracking_clock::TrackingClock>,
    pub reports_sent: u32,
    pub db_path: PathBuf,
}

impl Default for FlowSightAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl FlowSightAgent {
    pub fn new() -> Self {
        let db_path = crate::paths::db_path().unwrap_or_else(|e| {
            log::error!(
                "[Agent] paths::db_path unavailable ({}); using cwd fallback.",
                e
            );
            dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("FlowSight")
                .join("dev-agent.db")
        });

        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let mut agent = Self {
            config: AgentConfig {
                dev_name: Some(whoami::realname()),
                capture_interval: Some(60000),
                vision_model: Some(CONFIG_VISION_MODEL_ID.to_string()),
                // -1 = automatic tier probing (maximum compatibility + strongest profile that survives).
                gpu_layers: Some(-1),
                daily_goal_hours: Some(6.0),
            },
            is_running: false,
            tracking_clock: None,
            reports_sent: 0,
            db_path,
        };

        agent.init_db();
        if let Ok(conn) = Connection::open(&agent.db_path) {
            agent.tracking_clock = load_tracking_clock(&conn).ok();
        }
        agent.load_config();
        crate::focus_alerts::set_enabled(crate::desktop_presence::focus_alerts_enabled());

        if let Err(error) = crate::privacy::enforce_local_retention(&agent.db_path) {
            log::warn!("[Privacy] Local retention enforcement failed: {error}");
        }
        crate::privacy::start_local_retention_thread(agent.db_path.clone());
        crate::anonymous_analytics::start_analytics_sync_thread(agent.db_path.clone());
        // Start Background Sync (10m interval)
        crate::sync::start_sync_thread(agent.db_path.clone());
        // Proactive Supabase JWT refresh (~every 2m when near expiry)
        crate::sync::start_token_refresh_thread(agent.db_path.clone());

        agent
    }

    fn init_db(&self) {
        match Connection::open(&self.db_path) {
            Ok(conn) => {
                if let Err(e) = conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS config (key TEXT PRIMARY KEY, value TEXT);
                     CREATE TABLE IF NOT EXISTS reports (
                        id INTEGER PRIMARY KEY,
                        description TEXT,
                        activity_type TEXT,
                        synced INTEGER DEFAULT 0,
                        created_at TEXT DEFAULT CURRENT_TIMESTAMP
                     );",
                ) {
                    log::error!(
                        "[Agent] SQLite schema/bootstrap failed {:?}: {}",
                        self.db_path,
                        e
                    );
                }
                let _ = conn.execute("ALTER TABLE reports ADD COLUMN jira_ticket_id TEXT", []);
                let _ = conn.execute("ALTER TABLE reports ADD COLUMN active_app TEXT", []);
                let _ = conn.execute("ALTER TABLE reports ADD COLUMN window_title TEXT", []);
                let _ = conn.execute("ALTER TABLE reports ADD COLUMN theme_hint TEXT", []);
                let _ = conn.execute("ALTER TABLE reports ADD COLUMN capture_source TEXT", []);
                if let Err(error) = crate::privacy::ensure_schema(&conn) {
                    log::error!("[Privacy] Schema initialization failed: {error}");
                }

                let _ = conn.execute(
                    "ALTER TABLE reports ADD COLUMN duration_seconds INTEGER DEFAULT 30",
                    [],
                );
                if let Err(error) = conn.execute_batch(crate::sync_pure::PENDING_REPORT_INDEX_SQL) {
                    log::warn!("[Agent] Could not index pending reports: {error}");
                }
            }
            Err(e) => log::error!(
                "[Agent] SQLite open failed {:?} (init_db): {}",
                self.db_path,
                e
            ),
        }
    }

    fn load_config(&mut self) {
        let Ok(conn) = Connection::open(&self.db_path) else {
            log::warn!(
                "[Agent] load_config: cannot open {:?}; using defaults",
                self.db_path
            );
            return;
        };

        for (key, field) in [
            ("dev_name", &mut self.config.dev_name),
            ("vision_model", &mut self.config.vision_model),
        ] {
            if let Ok(val) = conn.query_row::<String, _, _>(
                "SELECT value FROM config WHERE key = ?",
                [key],
                |r| r.get(0),
            ) {
                *field = Some(val);
            }
        }

        if let Ok(val) = conn.query_row::<String, _, _>(
            "SELECT value FROM config WHERE key = 'gpu_layers'",
            [],
            |r| r.get(0),
        ) {
            if let Ok(parsed) = val.parse::<i32>() {
                self.config.gpu_layers = Some(parsed);
            }
        }

        if let Ok(val) = conn.query_row::<String, _, _>(
            "SELECT value FROM config WHERE key = 'daily_goal_hours'",
            [],
            |r| r.get(0),
        ) {
            if let Ok(parsed) = val.parse::<f64>() {
                self.config.daily_goal_hours = Some(parsed.clamp(0.0, 24.0));
            }
        }
    }

    fn save_config(&self) {
        let Ok(conn) = Connection::open(&self.db_path) else {
            log::warn!("[Agent] save_config: cannot open {:?}", self.db_path);
            return;
        };

        for (key, val) in [
            ("dev_name", &self.config.dev_name),
            ("vision_model", &self.config.vision_model),
        ] {
            if let Some(v) = val {
                let _ = conn.execute(
                    "INSERT OR REPLACE INTO config (key, value) VALUES (?, ?)",
                    params![key, v],
                );
            }
        }

        if let Some(layers) = self.config.gpu_layers {
            let _ = conn.execute(
                "INSERT OR REPLACE INTO config (key, value) VALUES (?, ?)",
                params!["gpu_layers", layers.to_string()],
            );
        }

        if let Some(hours) = self.config.daily_goal_hours {
            let _ = conn.execute(
                "INSERT OR REPLACE INTO config (key, value) VALUES (?, ?)",
                params!["daily_goal_hours", hours.to_string()],
            );
        }
    }

    fn save_report(
        &self,
        desc: &str,
        activity_type: &str,
        ticket: Option<String>,
        duration: u64,
    ) -> Option<i64> {
        let Ok(conn) = Connection::open(&self.db_path) else {
            log::warn!("[Agent] save_report: cannot open {:?}", self.db_path);
            return None;
        };
        if conn
            .execute(
                "INSERT INTO reports (description, activity_type, jira_ticket_id, duration_seconds) VALUES (?, ?, ?, ?)",
                params![desc, activity_type, ticket, duration],
            )
            .is_err()
        {
            return None;
        }
        Some(conn.last_insert_rowid())
    }

    #[allow(dead_code)]
    fn mark_synced(&self, id: i64) {
        if let Ok(conn) = Connection::open(&self.db_path) {
            let _ = conn.execute("UPDATE reports SET synced = 1 WHERE id = ?", [id]);
        }
    }

    fn get_recent(&self, limit: u32) -> Vec<ActivityReport> {
        let mut reports = Vec::new();
        if let Ok(conn) = Connection::open(&self.db_path) {
            if let Ok(mut stmt) = conn.prepare(
                "SELECT id, description, activity_type, synced, created_at FROM reports ORDER BY id DESC LIMIT ?"
            ) {
                if let Ok(rows) = stmt.query_map([limit], |row| {
                    Ok(ActivityReport {
                        id: row.get(0).ok(),
                        description: row.get(1)?,
                        activity_type: row.get(2)?,
                        synced: row.get::<_, i32>(3).unwrap_or(0) == 1,
                        timestamp: row.get(4)?,
                    })
                }) {
                    for row_result in rows {
                        if let Ok(report) = row_result {
                            reports.push(report);
                        }
                    }
                }
            }
        }
        reports
    }
}

// Capture and analyze screen — platform logic in `screen_capture` (Linux: grim / GNOME D-Bus / …; Windows: `screenshots`).
fn capture_screen() -> Result<(String, std::path::PathBuf), String> {
    crate::screen_capture::capture_screen()
}

#[derive(Serialize, Clone)]
pub struct CaptureResult {
    path: String,
    base64: String,
}

#[tauri::command]
pub fn capture_screen_command() -> Result<CaptureResult, String> {
    let (base64, path) = capture_screen()?;
    Ok(CaptureResult {
        path: path.to_string_lossy().to_string(),
        base64,
    })
}
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ContextSnapshot {
    pub vector: Vec<f32>,
    pub dimension: usize,
    pub description: String,
    pub category: String, // NEW
    pub analysis_failed: bool,
    pub metadata: SnapshotMetadata,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SnapshotMetadata {
    pub task: Option<String>,
    pub file: Option<String>,
    pub app: Option<String>,
    pub branch: Option<String>,
    pub language: Option<String>,
}

#[tauri::command]
pub async fn capture_context_snapshot(
    app: tauri::AppHandle,
    state: State<'_, AgentState>,
    user_task: Option<String>,
    jira_ticket: Option<String>,
) -> Result<ContextSnapshot, String> {
    crate::telemetry::record_selected_task(jira_ticket.as_deref().or(user_task.as_deref()));
    let foreground = crate::context::get_system_context();
    if let Ok(path) = crate::paths::db_path() {
        if crate::privacy::application_is_excluded(&path, foreground.app_name.as_deref()) {
            crate::focus_alerts::excluded_app_entered();
            return Err("Capture skipped for an excluded application.".into());
        } else if let Some(name) = foreground.app_name.as_deref() {
            crate::focus_alerts::record_app_switch(&app, name);
        }
    }
    // Extract config (default to 16 if not set to ensure balanced load)
    let gpu_layers = {
        let guard = state.lock().unwrap();
        guard
            .as_ref()
            .and_then(|a| a.config.gpu_layers)
            .or(Some(16))
    };

    #[cfg(target_os = "linux")]
    let portal_png = if crate::screen_capture::linux_is_gnome_session() {
        log::info!("[FlowSight] Capture path: ScreenCast via PipeWire (silent, no portal flash)");
        match crate::linux_silent_capture::ensure_session().await {
            Ok(()) => {}
            Err(e) => log::warn!("[FlowSight] ScreenCast session: {e}"),
        }
        crate::linux_silent_capture::capture_monitoring_frame()
    } else {
        crate::screen_capture::try_linux_portal_capture().await
    };
    #[cfg(not(target_os = "linux"))]
    let portal_png: Option<Vec<u8>> = None;

    // Vision + sync fallbacks on a worker thread; portal must stay on the async runtime.
    tauri::async_runtime::spawn_blocking(move || {
        use crate::context::get_system_context;
        use std::path::PathBuf;

        #[cfg(target_os = "linux")]
        let is_gnome_session = crate::screen_capture::linux_is_gnome_session();
        #[cfg(not(target_os = "linux"))]
        let is_gnome_session = false;

        // 1. Capture Screen
        let (base64, path_str) = if let Some(bytes) = portal_png {
            #[cfg(target_os = "linux")]
            {
                let finished = if is_gnome_session {
                    crate::screen_capture::finish_linux_png_bytes_screencast(bytes)
                } else {
                    crate::screen_capture::finish_linux_png_bytes(bytes)
                };
                finished?
            }
            #[cfg(not(target_os = "linux"))]
            {
                return Err("Portal screen capture is not available on this platform.".into());
            }
        } else if is_gnome_session {
            log::warn!("[FlowSight] ScreenCast returned no frame — skipping vision this cycle");
            return Ok(ContextSnapshot {
                vector: vec![],
                dimension: 0,
                description: "Screen analysis failed. Category: General".to_string(),
                category: "General".to_string(),
                analysis_failed: true,
                metadata: SnapshotMetadata {
                    task: jira_ticket.clone().or(user_task.clone()),
                    file: None,
                    app: None,
                    branch: None,
                    language: None,
                },
            });
        } else {
            capture_screen()?
        };
        let path = PathBuf::from(&path_str);

        // 2. Local vision analysis (visual description + category)
        let task_context = jira_ticket
            .clone()
            .or(user_task.clone())
            .unwrap_or_else(|| "General".to_string());

        let raw_analysis = match analyze_image_with_vision(&base64, &task_context, gpu_layers) {
            Ok(res) => (res, false),
            Err(e) => {
                let err_msg = format!("[Agent] AI Analysis Failed: {}", e);
                println!("{}", err_msg);

                // Log a archivo en el app data dir (antes era "agent_error.log"
                // con path relativo: en release cwd puede ser Program Files y
                // el write fallaba silencioso por UAC).
                if let Ok(log_path) = crate::paths::agent_error_log_path() {
                    if let Ok(mut file) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&log_path)
                    {
                        let _ = writeln!(file, "{}", err_msg);
                    }
                }

                (
                    "Screen analysis failed. Category: General".to_string(),
                    true,
                )
            }
        };

        // Parse category from response
        let (description, category) = parse_analysis(&raw_analysis.0);
        let analysis_failed =
            raw_analysis.1 || description.eq_ignore_ascii_case("No analysis available");

        if analysis_failed {
            log::warn!("[FlowSight] Vision analysis failed or empty for this capture");
        } else {
            log::info!(
                "[FlowSight] Vision analysis OK — category={} preview={}",
                category,
                description.chars().take(80).collect::<String>()
            );
        }

        // 3. System Context (Window/App)
        let sys = get_system_context();

        // 4. Git Context (Project)
        // Antes: hardcodeaba ~/Desktop/FlowSight.AI (solo exist\u00eda en la m\u00e1quina
        // del dev) y ca\u00eda a CWD=="." en release, que en una instalaci\u00f3n a
        // Program Files es in\u00fatil y puede filtrar metadata ajena.
        // Hoy devolvemos `None` hasta tener una estrategia real para resolver
        // el repo del usuario desde la ventana activa (ver SystemContext).
        let git: Option<crate::context::GitContext> = None;

        // Cleanup temp file
        let _ = std::fs::remove_file(&path);

        Ok(ContextSnapshot {
            vector: vec![],
            dimension: 0,
            description,
            category,
            analysis_failed,
            metadata: SnapshotMetadata {
                task: jira_ticket.or(user_task),
                file: sys.file_name,
                app: sys.app_name,
                branch: git.and_then(|g| g.branch),
                language: None,
            },
        })
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

#[tauri::command]
pub fn save_activity(
    app: tauri::AppHandle,
    state: State<'_, AgentState>,
    description: String,
    activity_type: String,
    jira_ticket: Option<String>,
    duration_seconds: Option<u64>,
    active_app: Option<String>,
) -> Result<ActivityReport, String> {
    let mut agent = state.lock().unwrap();
    let Some(a) = agent.as_mut() else {
        return Err(
            "Agent not initialized — wait for startup to finish before capturing.".to_string(),
        );
    };
    let duration = duration_seconds.unwrap_or(30).clamp(1, 300);
    let active_app =
        active_app.filter(|name| !crate::privacy::application_is_excluded(&a.db_path, Some(name)));
    a.reports_sent += 1;
    let report_id = a
        .save_report(&description, &activity_type, jira_ticket, duration)
        .ok_or_else(|| "Failed to write activity to local database.".to_string())?;

    if let Ok(conn) = Connection::open(&a.db_path) {
        let _ = conn.execute(
            "UPDATE reports SET active_app=?1 WHERE id=?2",
            params![active_app, report_id],
        );
    }
    crate::focus_alerts::review_browsing_report(
        &app,
        &a.db_path,
        &activity_type,
        duration,
        &description,
        active_app.as_deref(),
    );
    Ok(ActivityReport {
        id: Some(report_id),
        timestamp: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        description,
        activity_type,
        synced: false,
    })
}

// ============== TAURI COMMANDS ==============

/// Comprueba que SQLite puede **escribir** en `dev-agent.db` (CFA / solo lectura / disco lleno).
fn probe_sqlite_database_rw() -> Result<(), String> {
    let db_path = crate::paths::db_path()?;
    let conn =
        Connection::open(&db_path).map_err(|e| format!("SQLite cannot open {:?}: {e}", db_path))?;
    conn.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE TEMP TABLE IF NOT EXISTS _flowsight_io_probe (x INTEGER);
         INSERT INTO _flowsight_io_probe VALUES (1);
         COMMIT;",
    )
    .map_err(|e| {
        format!(
            "SQLite cannot write to {:?}. On Windows 11, verify Controlled Folder Access / Defender is not blocking this app from modifying its data folder ({e})",
            db_path
        )
    })?;
    Ok(())
}

#[tauri::command]
pub fn initialize_agent(
    app_handle: tauri::AppHandle,
    state: State<'_, AgentState>,
) -> Result<bool, String> {
    let mut g = state.lock().unwrap();
    if g.is_some() {
        return Ok(true);
    }

    crate::paths::verify_app_dir_filesystem_writable()?;
    probe_sqlite_database_rw()?;

    let max_h: u64 = std::env::var("FLOWSIGHT_SCREENSHOT_TMP_MAX_HOURS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(72);
    match crate::paths::prune_screenshots_tmp_older_than(Duration::from_secs(max_h * 3600)) {
        Ok(n) if n > 0 => {
            log::info!(
                "[FlowSight] removed {n} screenshot(s) older than {max_h}h from screenshots_tmp"
            );
        }
        Err(e) => log::warn!("[FlowSight] screenshots_tmp retention prune: {e}"),
        _ => {}
    }

    *g = Some(FlowSightAgent::new());
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(15));
        use tauri::Manager;
        let _ = get_tracking_clock(app_handle.state::<AgentState>());
    });
    crate::language::initialize();
    Ok(true)
}

#[tauri::command]
pub fn get_config(state: State<'_, AgentState>) -> Result<AgentConfig, String> {
    Ok(state
        .lock()
        .unwrap()
        .as_ref()
        .map(|a| a.config.clone())
        .unwrap_or_default())
}

#[tauri::command]
pub fn update_config(state: State<'_, AgentState>, patch: AgentConfig) -> Result<bool, String> {
    if let Some(agent) = state.lock().unwrap().as_mut() {
        let c = &mut agent.config;
        if patch.dev_name.is_some() {
            c.dev_name = patch.dev_name;
        }
        if patch.capture_interval.is_some() {
            c.capture_interval = patch.capture_interval;
        }
        if patch.vision_model.is_some() {
            c.vision_model = patch.vision_model;
        }
        // callers (renderer) omit `gpuLayers`; full replace here used to wipe auto/manual choice
        if patch.gpu_layers.is_some() {
            c.gpu_layers = patch.gpu_layers;
        }
        if patch.daily_goal_hours.is_some() {
            c.daily_goal_hours = patch.daily_goal_hours.map(|h| h.clamp(0.0, 24.0));
        }
        agent.save_config();
    }
    Ok(true)
}

fn load_tracking_clock(conn: &Connection) -> Result<crate::tracking_clock::TrackingClock, String> {
    let today = Local::now().format("%Y-%m-%d").to_string();
    let observed = load_daily_totals(conn, &today, &today)?
        .get(&today)
        .copied()
        .unwrap_or(0)
        .max(0) as u64;
    crate::tracking_clock::TrackingClock::load(conn, observed)
}

#[tauri::command]
pub fn get_tracking_clock(
    state: State<'_, AgentState>,
) -> Result<crate::tracking_clock::TrackingClockSnapshot, String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    let agent = guard.as_mut().ok_or("Agent not initialized")?;
    let conn = Connection::open(&agent.db_path).map_err(|e| e.to_string())?;
    if agent.tracking_clock.is_none() {
        agent.tracking_clock = Some(load_tracking_clock(&conn)?);
    }
    agent
        .tracking_clock
        .as_mut()
        .unwrap()
        .snapshot(&conn, agent.is_running)
}

#[tauri::command]
pub fn get_status(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    let agent = state.lock().unwrap();
    Ok(if let Some(a) = agent.as_ref() {
        serde_json::json!({
            "isRunning": a.is_running,
            "reportsSent": a.reports_sent
        })
    } else {
        serde_json::json!({"isRunning": false, "reportsSent": 0})
    })
}

#[tauri::command]
pub fn start_monitoring(state: State<'_, AgentState>) -> Result<bool, String> {
    crate::privacy::require_monitoring_acknowledgement(&crate::paths::db_path()?)?;
    if let Some(a) = state.lock().unwrap().as_mut() {
        let conn = Connection::open(&a.db_path).map_err(|e| e.to_string())?;
        if a.tracking_clock.is_none() {
            a.tracking_clock = Some(load_tracking_clock(&conn)?);
        }
        a.tracking_clock.as_mut().unwrap().snapshot(&conn, true)?;
        a.is_running = true;
        crate::focus_alerts::start_monitoring(&a.db_path);
        crate::telemetry::set_enabled(true);
    }
    #[cfg(target_os = "linux")]
    if crate::linux_silent_capture::linux_use_silent_screencast() {
        tauri::async_runtime::spawn(async {
            if let Err(e) = crate::linux_silent_capture::ensure_session().await {
                log::warn!("[FlowSight] Silent ScreenCast: {e}");
            }
        });
    }
    Ok(true)
}

#[tauri::command]
pub fn stop_monitoring(state: State<'_, AgentState>) -> Result<bool, String> {
    if let Some(a) = state.lock().unwrap().as_mut() {
        if let Some(clock) = a.tracking_clock.as_mut() {
            let conn = Connection::open(&a.db_path).map_err(|e| e.to_string())?;
            clock.snapshot(&conn, false)?;
        }
        a.is_running = false;
        crate::focus_alerts::stop_monitoring();
        crate::telemetry::set_enabled(false);
    }
    #[cfg(target_os = "linux")]
    crate::linux_silent_capture::stop_session();
    Ok(true)
}

#[tauri::command]
pub fn get_activity_log(
    state: State<'_, AgentState>,
    limit: Option<u32>,
) -> Result<Vec<ActivityReport>, String> {
    Ok(state
        .lock()
        .unwrap()
        .as_ref()
        .map(|a| a.get_recent(limit.unwrap_or(20)))
        .unwrap_or_default())
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DayHistoryEntry {
    pub time: String,
    pub description: String,
    pub category: String,
    pub ticket: Option<String>,
    pub duration_seconds: i32,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    pub capture_source: Option<String>,
    pub theme_hint: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CategoryBreakdown {
    pub category: String,
    pub total_seconds: i32,
    pub count: i32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct TicketBreakdown {
    pub ticket: String,
    pub total_seconds: i32,
    pub count: i32,
}

#[derive(Serialize, Debug)]
pub struct TodayHistory {
    pub entries: Vec<DayHistoryEntry>,
    pub total_seconds: i32,
    pub tracking: Option<crate::tracking_clock::TrackingClockSnapshot>,
    pub category_breakdown: Vec<CategoryBreakdown>,
    pub ticket_breakdown: Vec<TicketBreakdown>,
    pub date: String,
    pub focus: crate::focus_semantics::FocusSummary,
}

fn load_day_history_entries(conn: &Connection, day: &str) -> Result<Vec<DayHistoryEntry>, String> {
    let window = LocalDateWindow::parse(day, day)?;
    let mut stmt = conn
        .prepare(
            "SELECT datetime(created_at, 'localtime'), description, activity_type, jira_ticket_id, duration_seconds,
                    active_app, window_title, capture_source, theme_hint
             FROM reports
             WHERE date(created_at, 'localtime') >= ?1
               AND date(created_at, 'localtime') <= date(?1, '+1 day')
             ORDER BY datetime(created_at, 'localtime') ASC",
        )
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map(params![day], |row| {
            Ok((
                row.get::<_, String>(0).unwrap_or_default(),
                row.get::<_, String>(1).unwrap_or_default(),
                row.get::<_, String>(2).unwrap_or_default(),
                row.get::<_, Option<String>>(3).unwrap_or(None),
                row.get::<_, i64>(4).unwrap_or(0).max(0),
                row.get::<_, Option<String>>(5).unwrap_or(None),
                row.get::<_, Option<String>>(6).unwrap_or(None),
                row.get::<_, Option<String>>(7).unwrap_or(None),
                row.get::<_, Option<String>>(8).unwrap_or(None),
            ))
        })
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok);

    let mut entries = Vec::new();
    for (time, description, raw_category, ticket, duration, app, title, source, theme) in rows {
        let category = resolve_persisted_category(&raw_category);
        let ticket = canonical_ticket_value(ticket.as_deref());
        let slices = window.slices_for_observation(&time, duration);
        if slices.is_empty() {
            // Zero-duration action reviews remain useful narrative evidence,
            // but never add tracked time or enter the focus detector.
            if duration == 0 && window.contains_local_timestamp(&time) {
                entries.push(DayHistoryEntry {
                    time,
                    description,
                    category,
                    ticket,
                    duration_seconds: 0,
                    app_name: app,
                    window_title: title,
                    capture_source: source,
                    theme_hint: theme,
                });
            }
            continue;
        }
        for slice in slices {
            let slice_end = slice.start + chrono::Duration::seconds(slice.duration_seconds);
            entries.push(DayHistoryEntry {
                time: slice_end.format("%Y-%m-%d %H:%M:%S").to_string(),
                description: description.clone(),
                category: category.clone(),
                ticket: ticket.clone(),
                duration_seconds: i32::try_from(slice.duration_seconds).unwrap_or(i32::MAX),
                app_name: app.clone(),
                window_title: title.clone(),
                capture_source: source.clone(),
                theme_hint: theme.clone(),
            });
        }
    }
    entries.sort_by(|left, right| right.time.cmp(&left.time));
    Ok(entries)
}

pub(crate) fn load_daily_totals(
    conn: &Connection,
    period_start: &str,
    period_end: &str,
) -> Result<std::collections::HashMap<String, i32>, String> {
    let window = LocalDateWindow::parse(period_start, period_end)?;
    let mut stmt = conn
        .prepare(
            "SELECT datetime(created_at, 'localtime'), duration_seconds
             FROM reports
             WHERE date(created_at, 'localtime') >= ?1
               AND date(created_at, 'localtime') <= date(?2, '+1 day')
             ORDER BY datetime(created_at, 'localtime') ASC",
        )
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map(params![period_start, period_end], |row| {
            Ok((
                row.get::<_, String>(0).unwrap_or_default(),
                row.get::<_, i64>(1).unwrap_or(0).max(0),
            ))
        })
        .map_err(|error| error.to_string())?;
    let mut totals = std::collections::HashMap::new();
    for (observed_end, duration) in rows.filter_map(Result::ok) {
        for slice in window.slices_for_observation(&observed_end, duration) {
            let date = slice.start.format("%Y-%m-%d").to_string();
            let seconds = i32::try_from(slice.duration_seconds).unwrap_or(i32::MAX);
            totals
                .entry(date)
                .and_modify(|total: &mut i32| *total = total.saturating_add(seconds))
                .or_insert(seconds);
        }
    }
    Ok(totals)
}

#[tauri::command]
pub fn get_today_history(state: State<'_, AgentState>) -> Result<TodayHistory, String> {
    let mut agent = state.lock().unwrap();
    let agent = agent.as_mut().ok_or("Agent not initialized")?;

    let conn = Connection::open(&agent.db_path).map_err(|e| e.to_string())?;
    let tracking = agent
        .tracking_clock
        .as_mut()
        .map(|clock| clock.snapshot(&conn, agent.is_running))
        .transpose()?;
    // Calendar 'today' in local TZ must use UTC→local conversion: `created_at`
    // defaults to CURRENT_TIMESTAMP (UTC). Comparing plain `date(created_at)`
    // to `date('now','localtime')` used mismatched halves and often returned zero rows.
    let today = Local::now().format("%Y-%m-%d").to_string();

    let entries = load_day_history_entries(&conn, &today)?;

    // Calculate total
    let total_seconds: i32 = entries.iter().map(|e| e.duration_seconds).sum();

    // Category breakdown
    let mut cat_map: std::collections::HashMap<String, (i32, i32)> =
        std::collections::HashMap::new();
    for e in &entries {
        let entry = cat_map.entry(e.category.clone()).or_insert((0, 0));
        entry.0 += e.duration_seconds;
        entry.1 += 1;
    }
    let mut category_breakdown: Vec<CategoryBreakdown> = cat_map
        .into_iter()
        .map(|(category, (total_seconds, count))| CategoryBreakdown {
            category,
            total_seconds,
            count,
        })
        .collect();
    category_breakdown.sort_by(|left, right| {
        right
            .total_seconds
            .cmp(&left.total_seconds)
            .then_with(|| left.category.cmp(&right.category))
    });

    // Ticket breakdown
    let mut ticket_map: std::collections::HashMap<String, (i32, i32)> =
        std::collections::HashMap::new();
    for e in &entries {
        if let Some(ref ticket) = e.ticket {
            let entry = ticket_map.entry(ticket.clone()).or_insert((0, 0));
            entry.0 += e.duration_seconds;
            entry.1 += 1;
        }
    }
    let mut ticket_breakdown: Vec<TicketBreakdown> = ticket_map
        .into_iter()
        .map(|(ticket, (total_seconds, count))| TicketBreakdown {
            ticket,
            total_seconds,
            count,
        })
        .collect();
    ticket_breakdown.sort_by(|left, right| {
        right
            .total_seconds
            .cmp(&left.total_seconds)
            .then_with(|| left.ticket.cmp(&right.ticket))
    });

    let focus = crate::focus_semantics::summarize_from_db(&conn, &today, &today)?;

    Ok(TodayHistory {
        entries,
        total_seconds,
        tracking,
        category_breakdown,
        ticket_breakdown,
        date: today,
        focus,
    })
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DayActivity {
    pub date: String,
    pub weekday: String,
    pub total_seconds: i32,
    pub has_activity: bool,
    pub is_today: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct WeekSummary {
    pub days: Vec<DayActivity>,
    pub yesterday_seconds: i32,
}

#[tauri::command]
pub fn get_week_summary(state: State<'_, AgentState>) -> Result<WeekSummary, String> {
    let agent = state.lock().unwrap();
    let agent = agent.as_ref().ok_or("Agent not initialized")?;

    let conn = Connection::open(&agent.db_path).map_err(|e| e.to_string())?;
    let today = Local::now().date_naive();
    let weekday = today.weekday().num_days_from_monday();
    let week_start = today - chrono::Duration::days(weekday as i64);
    let week_end = week_start + chrono::Duration::days(6);
    let yesterday = today - chrono::Duration::days(1);

    let start_str = week_start.format("%Y-%m-%d").to_string();
    let end_str = week_end.format("%Y-%m-%d").to_string();
    let day_totals = load_daily_totals(&conn, &start_str, &end_str)?;

    let weekday_labels = ["M", "T", "W", "T", "F", "S", "S"];
    let mut days = Vec::with_capacity(7);
    for offset in 0..7 {
        let day = week_start + chrono::Duration::days(offset);
        let date_str = day.format("%Y-%m-%d").to_string();
        let total = day_totals.get(&date_str).copied().unwrap_or(0);
        days.push(DayActivity {
            date: date_str,
            weekday: weekday_labels[offset as usize].to_string(),
            total_seconds: total,
            has_activity: total > 0,
            is_today: day == today,
        });
    }

    let yesterday_str = yesterday.format("%Y-%m-%d").to_string();
    let yesterday_seconds = day_totals.get(&yesterday_str).copied().unwrap_or(0);

    Ok(WeekSummary {
        days,
        yesterday_seconds,
    })
}

// Health check against nuestro llama-server local (NO es ollama; el nombre se
// mantuvo en el tauri command hist\u00f3ricamente pero el endpoint es de llama.cpp).
//
// Timeout generoso: en equipos lentos o con antivirus el primer /health puede tardar
// mientras el modelo termina de cargar; 1s provocaba falsos "offline" intermitentes.
const LOCAL_HEALTH_HTTP_TIMEOUT_SECS: u64 = 12;

/// Quick binary health check reused by diagnostics and automated tier probing.
fn local_server_health_ok() -> bool {
    let Some(health_url) = crate::llama_port::managed_health_url() else {
        return false;
    };
    let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(
            LOCAL_HEALTH_HTTP_TIMEOUT_SECS,
        ))
        .build()
    else {
        return false;
    };
    client
        .get(&health_url)
        .send()
        .ok()
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

// Async so Tauri dispatches it off the main/UI thread: the `reqwest::blocking`
// call below opens a real TCP socket even for localhost, which is exactly what
// makes Windows inject any registered Winsock LSP (VPN/AV network proxies —
// see crash_guard.rs) into this process. Keeping that off the main thread
// means a crash_guard-contained LSP crash here only takes down this
// background thread, never the window's message loop.
#[tauri::command]
pub fn check_local_server() -> Result<serde_json::Value, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(
            LOCAL_HEALTH_HTTP_TIMEOUT_SECS,
        ))
        .build()
        .map_err(|e| e.to_string())?;

    let Some(health_url) = crate::llama_port::managed_health_url() else {
        return Ok(serde_json::json!({
            "online": false,
            "installed": true,
            "error": "No managed llama-server port (start Local AI first)."
        }));
    };

    match client.get(&health_url).send() {
        Ok(r) if r.status().is_success() => Ok(serde_json::json!({
            "online": true,
            "installed": true,
            "models": [VISION_STATUS_LABEL],
            "hasVisionModel": true,
            "localServerPort": crate::llama_port::current_managed_listen_port(),
        })),
        Ok(r) => Ok(serde_json::json!({
            "online": false,
            "installed": true,
            "error": format!("Local server status: {}", r.status())
        })),
        Err(_) => Ok(serde_json::json!({
            "online": false,
            "installed": true
        })),
    }
}

// Legacy alias: el frontend todav\u00eda llama `check_ollama` en dos sitios. Lo
// mantenemos como thin wrapper para no cambiar el contrato en un solo PR.
// TODO: migrar los `invoke('check_ollama')` del renderer y borrar este alias.
#[tauri::command]
pub fn check_ollama() -> Result<serde_json::Value, String> {
    check_local_server()
}

// LLAMA SERVER COMMANDS

static SERVER_PROCESS: Mutex<Option<std::process::Child>> = Mutex::new(None);
static SERVER_STARTUP_LOCK: Mutex<()> = Mutex::new(());

/// Puertos nuevos ante `EADDRINUSE`/fallo rápido de escucha tras TOCTOU o TIME_WAIT.
const LLAMA_LISTEN_PORT_SPAWN_ATTEMPTS: u8 = 8;

fn clamp_llama_gpu_layers(n: i32) -> i32 {
    n.max(0).min(16_384)
}

/// Descending CUDA/Vulkan offload steps for vision GGUF (+ mmproj): try the highest that
/// survives startup + `/health`, then fall back toward CPU-only (`0`).
const AUTO_GPU_LAYER_TIERS: &[i32] = &[56, 40, 24, 12, 0];
/// Per-tier budget while the weights load (slow disks / AV can dominate here).
const AUTO_TIER_HEALTH_WAIT_SECS: u64 = 56;
/// Tras la ronda con descubrimiento Vulkan por defecto, probar cada índice físico por
/// separado (`GGML_VK_VISIBLE_DEVICES` en ggml-vulkan). Útil cuando el primer dispositivo
/// Vulkan de la lista es inválido para cómputo (GPU dual, drivers híbridos, `vkCreateFence`).
const AUTO_VULKAN_VISIBLE_DEVICE_TRIES: &[&str] = &["0", "1", "2", "3"];

fn auto_startup_attempts() -> Vec<(Option<&'static str>, i32)> {
    if cfg!(target_os = "linux") {
        // One conservative Vulkan attempt, then the separate CPU runtime.
        // Repeating every GPU tier on every visible device can leave Ubuntu
        // apparently frozen for many minutes on machines with broken Vulkan.
        return vec![(None, 24), (None, 0)];
    }
    std::iter::once(None)
        .chain(AUTO_VULKAN_VISIBLE_DEVICE_TRIES.iter().copied().map(Some))
        .flat_map(|device| {
            AUTO_GPU_LAYER_TIERS
                .iter()
                .copied()
                .map(move |layers| (device, layers))
        })
        .collect()
}

fn emit_startup_progress(
    app: &tauri::AppHandle,
    phase: &str,
    backend: &str,
    attempt: usize,
    total: usize,
) {
    let _ = app.emit(
        "local-ai-startup-progress",
        serde_json::json!({
            "phase": phase,
            "backend": backend,
            "attempt": attempt,
            "total": total,
        }),
    );
}

#[derive(Clone, Copy, Debug)]
enum GpuServeMode {
    Automatic,
    Manual(i32),
}

fn gpu_serve_mode(state: &State<AgentState>) -> GpuServeMode {
    let guard = state.lock().unwrap();
    let raw = match guard.as_ref() {
        None => return GpuServeMode::Automatic,
        Some(a) => a.config.gpu_layers,
    };
    match raw {
        None | Some(-1) => GpuServeMode::Automatic,
        Some(n) if n >= 0 => GpuServeMode::Manual(clamp_llama_gpu_layers(n)),
        Some(_) => GpuServeMode::Automatic,
    }
}

/// Returns true once `/health` succeeds. If the managed child exits, clears it and stops early.
fn wait_for_managed_health_secs(max_secs: u64) -> bool {
    for _ in 0..max_secs {
        if local_server_health_ok() {
            return true;
        }

        let still_running = {
            let mut g = SERVER_PROCESS.lock().unwrap();
            match g.as_mut() {
                Some(ch) => match ch.try_wait() {
                    Ok(Some(_)) => {
                        g.take();
                        false
                    }
                    Ok(None) => true,
                    Err(_) => {
                        let _ = g.take();
                        false
                    }
                },
                None => false,
            }
        };

        if !still_running {
            return false;
        }

        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    false
}

/// Starts the managed llama-server if needed and waits until `/health` responds.
pub fn ensure_local_llm_ready(
    app: tauri::AppHandle,
    state: State<'_, AgentState>,
) -> Result<(), String> {
    if local_server_health_ok() {
        return Ok(());
    }

    log::info!("[LocalReport] Local AI offline — starting server for insight generation…");
    let mode = gpu_serve_mode(&state);
    let result = start_server_with_mode(&app, mode)?;
    let status = result["status"].as_str().unwrap_or("");
    if status != "started" && status != "already_running" {
        return Err(format!("Could not start local AI: {}", result));
    }

    const REPORT_READY_WAIT_SECS: u64 = 180;
    if wait_for_managed_health_secs(REPORT_READY_WAIT_SECS) {
        log::info!("[LocalReport] Local AI ready for report generation");
        return Ok(());
    }

    Err(format!(
        "Local AI did not respond within {}s. Start monitoring from Today and retry.",
        REPORT_READY_WAIT_SECS
    ))
}

fn tail_chars(s: &str, max_chars: usize) -> &str {
    if max_chars == 0 {
        return "";
    }
    let start = s
        .char_indices()
        .rev()
        .nth(max_chars - 1)
        .map(|(index, _)| index)
        .unwrap_or(0);
    &s[start..]
}

fn read_server_log_tail_chars(max_chars: usize) -> String {
    let Ok(path) = crate::paths::server_log_path() else {
        return String::new();
    };
    let Ok(s) = std::fs::read_to_string(&path) else {
        return String::new();
    };
    tail_chars(&s, max_chars).to_string()
}

/// Estado del proceso hijo que FlowSight lanzó (no confundir con un llama-server huérfano).
#[tauri::command]
pub fn llama_managed_process_status() -> Result<serde_json::Value, String> {
    let mut guard = SERVER_PROCESS.lock().unwrap();
    match guard.as_mut() {
        None => Ok(serde_json::json!({
            "managed": false,
            "alive": null
        })),
        Some(child) => match child.try_wait() {
            Ok(Some(status)) => {
                let code = status.code();
                guard.take();
                Ok(serde_json::json!({
                    "managed": true,
                    "alive": false,
                    "exitCode": code
                }))
            }
            Ok(None) => Ok(serde_json::json!({
                "managed": true,
                "alive": true
            })),
            Err(e) => Err(e.to_string()),
        },
    }
}

#[tauri::command]
pub fn llama_server_log_tail(max_chars: Option<usize>) -> Result<String, String> {
    let n = max_chars.unwrap_or(1_200).clamp(200, 5_000);
    Ok(read_server_log_tail_chars(n))
}

fn configure_llama_command(
    bin_path: &Path,
    model_path: &Path,
    mmproj_path: &Path,
    log_path: &Path,
    listen_port: u16,
    gpu_layers: i32,
    vulkan_visible_device_index: Option<&str>,
    redirect_log_to_file: bool,
    #[cfg_attr(not(windows), allow(unused_variables))] creation_flags: Option<u32>,
) -> Result<std::process::Command, String> {
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    let n_gpu_layers = clamp_llama_gpu_layers(gpu_layers);

    let mut cmd = Command::new(bin_path);
    // Evita heredar stdin inválido tras FreeConsole en el proceso padre (release Windows).
    cmd.stdin(std::process::Stdio::null());
    cmd.arg("-m")
        .arg(model_path)
        .arg("--mmproj")
        .arg(mmproj_path)
        .arg("--alias")
        .arg(LLAMA_CHAT_MODEL_ID)
        .arg("--reasoning-budget")
        .arg("0")
        .arg("--chat-template-kwargs")
        .arg(r#"{"enable_thinking":false}"#)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(listen_port.to_string())
        .arg("--ctx-size")
        .arg(if cfg!(target_os = "linux") {
            "4096"
        } else {
            "8192"
        })
        .arg("--parallel")
        .arg(if cfg!(target_os = "linux") { "1" } else { "2" })
        .arg("--threads")
        .arg("2")
        .arg("--n-gpu-layers")
        .arg(n_gpu_layers.to_string());

    if let Some(idx) = vulkan_visible_device_index {
        cmd.env("GGML_VK_VISIBLE_DEVICES", idx);
    }

    // Con pesos sólo en CPU, los builds con Vulkan pueden igual inicializar la API y fallar
    // (p. ej. `vkCreateFence: Invalid device`) antes de que `/health` responda.
    // GGML + llama.cpp respetan estas variables sin pasar flags extra por CLI.
    if n_gpu_layers == 0 {
        cmd.env("GGML_DISABLE_VULKAN", "1");
        cmd.env("LLAMA_ARG_DEVICE", "none");
    }

    // CWD y PATH apuntan a la carpeta del binario. Algunos backends de
    // llama.cpp cargan DLLs dinámicamente por nombre, y en instalaciones
    // Windows no siempre basta con que estén junto al exe.
    if let Some(parent) = bin_path.parent() {
        cmd.current_dir(parent);
        if let Some(existing_path) = std::env::var_os("PATH") {
            let mut paths = vec![parent.to_path_buf()];
            paths.extend(std::env::split_paths(&existing_path));
            if let Ok(joined_path) = std::env::join_paths(paths) {
                cmd.env("PATH", joined_path);
            }
        } else {
            cmd.env("PATH", parent);
        }
    }

    #[cfg(windows)]
    if let Some(flags) = creation_flags {
        cmd.creation_flags(flags);
    }

    if redirect_log_to_file {
        if let Ok(file) = std::fs::File::create(log_path) {
            if let Ok(file_err) = file.try_clone() {
                cmd.stdout(std::process::Stdio::from(file));
                cmd.stderr(std::process::Stdio::from(file_err));
            }
        }
    }

    Ok(cmd)
}

fn log_suggests_listen_bind_failure(tail: &str) -> bool {
    let t = tail.to_ascii_lowercase();
    t.contains("eaddrinuse")
        || t.contains("address already in use")
        || t.contains("10048")
        || t.contains("failed to bind")
        || t.contains("bind failed")
        || t.contains("could not bind")
        || (t.contains("bind") && t.contains("in use"))
        || (t.contains("error") && t.contains("listen") && t.contains("socket"))
}

fn try_spawn_llama_process(
    bin_path: &Path,
    model_path: &Path,
    mmproj_path: &Path,
    log_path: &Path,
    listen_port: u16,
    gpu_layers: i32,
    vulkan_visible_device_index: Option<&str>,
) -> Result<std::process::Child, std::io::Error> {
    #[cfg(windows)]
    {
        use std::io::Error as IoError;

        const CREATE_NO_WINDOW: u32 = 0x08000000;
        const BELOW_NORMAL_PRIORITY: u32 = 0x00004000;

        let attempts: [(Option<u32>, bool); 6] = [
            (Some(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY), true),
            (Some(CREATE_NO_WINDOW), true),
            (None, true),
            (Some(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY), false),
            (Some(CREATE_NO_WINDOW), false),
            (None, false),
        ];

        let mut last_err = IoError::new(
            std::io::ErrorKind::Other,
            "llama-server spawn failed (no attempts)",
        );
        let mut spawned: Option<std::process::Child> = None;
        for &(flags, redirect_log) in &attempts {
            let mut cmd = configure_llama_command(
                bin_path,
                model_path,
                mmproj_path,
                log_path,
                listen_port,
                gpu_layers,
                vulkan_visible_device_index,
                redirect_log,
                flags,
            )
            .map_err(|msg| IoError::new(std::io::ErrorKind::Other, msg))?;

            match cmd.spawn() {
                Ok(child) => {
                    spawned = Some(child);
                    break;
                }
                Err(e) => {
                    let retry_os50 = e.raw_os_error() == Some(50);
                    last_err = e;
                    if !retry_os50 {
                        break;
                    }
                }
            }
        }
        spawned.ok_or(last_err)
    }

    #[cfg(not(windows))]
    {
        let mut cmd = configure_llama_command(
            bin_path,
            model_path,
            mmproj_path,
            log_path,
            listen_port,
            gpu_layers,
            vulkan_visible_device_index,
            true,
            None,
        )
        .map_err(|msg| std::io::Error::new(std::io::ErrorKind::Other, msg))?;
        cmd.spawn()
    }
}

fn spawn_llama_managed_child(
    app: &tauri::AppHandle,
    gpu_layers: i32,
    vulkan_visible_device_index: Option<&str>,
) -> Result<std::process::Child, String> {
    let (model_path, mmproj_path) = crate::model_assets::ensure_vision_weights(app)?;
    let weights_dir =
        crate::paths::resource_local_llm_dir(app).unwrap_or(crate::paths::local_llm_storage_dir()?);
    let cpu_only_runtime = cfg!(target_os = "linux") && gpu_layers == 0;
    let storage_name = if cpu_only_runtime {
        "bin-b10666-cpu"
    } else {
        "bin-b10666"
    };
    let bin_path = crate::llama_bin::ensure_llama_server(
        app,
        crate::paths::local_llm_storage_dir()?.join(storage_name),
        cpu_only_runtime,
    )?;

    if !model_path.exists() {
        return Err(format!(
            "Vision weights not found at {:?}. Reinstall FlowSight Agent.",
            model_path
        ));
    }
    if !mmproj_path.exists() {
        return Err(format!(
            "Vision projector not found at {:?}. Reinstall FlowSight Agent.",
            mmproj_path
        ));
    }

    let log_path = crate::paths::server_log_path()?;

    let mut last_err: Option<String> = None;

    for attempt in 0..LLAMA_LISTEN_PORT_SPAWN_ATTEMPTS {
        let listen_port = crate::llama_port::pick_localhost_listen_port()?;

        let spawn_result = try_spawn_llama_process(
            &bin_path,
            &model_path,
            &mmproj_path,
            &log_path,
            listen_port,
            gpu_layers,
            vulkan_visible_device_index,
        );

        match spawn_result {
            Ok(mut child) => {
                crate::llama_port::set_managed_llama_port(listen_port);
                std::thread::sleep(std::time::Duration::from_secs(2));
                if let Ok(Some(status)) = child.try_wait() {
                    crate::llama_port::clear_managed_llama_port();
                    let log_tail = read_server_log_tail_chars(1_200);
                    let can_retry_port = (attempt + 1) < LLAMA_LISTEN_PORT_SPAWN_ATTEMPTS
                        && log_suggests_listen_bind_failure(&log_tail);
                    if can_retry_port {
                        log::warn!(
                            "[FlowSight llama-server] quick exit (code {:?}); retrying another listen port (attempt {}/{})",
                            status.code(),
                            attempt + 2,
                            LLAMA_LISTEN_PORT_SPAWN_ATTEMPTS
                        );
                        std::thread::sleep(std::time::Duration::from_millis(
                            40_u64.saturating_mul(u64::from(attempt) + 1),
                        ));
                        continue;
                    }
                    return Err(format!(
                        "llama-server exited during startup (code: {:?}). {}",
                        status.code(),
                        if log_tail.is_empty() {
                            format!("See {:?}", log_path)
                        } else {
                            format!("Log tail: {}", log_tail)
                        }
                    ));
                }
                #[cfg(windows)]
                if let Err(e) =
                    crate::llama_windows_job::assign_llama_child_to_kill_on_close_job(&child)
                {
                    log::warn!(
                        "[FlowSight llama-server] Windows job-object attach skipped: {}",
                        e
                    );
                }
                return Ok(child);
            }
            Err(e) => {
                let hint = if e.raw_os_error() == Some(13) {
                    " (permission denied — on Linux use a native llama-server, not the Windows .exe in local_llm/bin)"
                } else {
                    ""
                };
                let msg = format!("Failed to start server: {}{}", e, hint);
                last_err = Some(msg.clone());
                if (attempt + 1) < LLAMA_LISTEN_PORT_SPAWN_ATTEMPTS
                    && crate::llama_port::tcp_bind_addr_in_use(&e)
                {
                    log::warn!(
                        "[FlowSight llama-server] spawn EADDRINUSE-style error; retrying another port ({}/{}) — {}",
                        attempt + 2,
                        LLAMA_LISTEN_PORT_SPAWN_ATTEMPTS,
                        e
                    );
                    std::thread::sleep(std::time::Duration::from_millis(
                        50_u64.saturating_mul(u64::from(attempt) + 1),
                    ));
                    continue;
                }
                return Err(msg);
            }
        }
    }

    Err(last_err
        .unwrap_or_else(|| "Failed to start server: exhausted listen-port retries.".to_string()))
}

/// Arranca llama-server: modo automático sube desde capas GPU altas hasta que `/health`
/// responda; modo manual fuerza `--n-gpu-layers` fijo.
#[tauri::command]
pub async fn start_server(
    app: tauri::AppHandle,
    state: State<'_, AgentState>,
) -> Result<serde_json::Value, String> {
    let mode = gpu_serve_mode(&state);
    tauri::async_runtime::spawn_blocking(move || start_server_with_mode(&app, mode))
        .await
        .map_err(|e| format!("Local AI startup task failed: {e}"))?
}

fn start_server_with_mode(
    app: &tauri::AppHandle,
    mode: GpuServeMode,
) -> Result<serde_json::Value, String> {
    let _startup_guard = SERVER_STARTUP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    {
        let mut guard = SERVER_PROCESS.lock().unwrap();
        let running = match guard.as_mut().map(|child| child.try_wait()) {
            Some(Ok(None)) => true,
            Some(Err(e)) => {
                log::warn!("[LocalAI] could not inspect managed server; replacing it: {e}");
                if let Some(child) = guard.as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                false
            }
            _ => false,
        };
        if running {
            return Ok(serde_json::json!({
                "status": "already_running",
                "message": "Server is already running",
                "gpuAuto": false,
                "localServerPort": crate::llama_port::current_managed_listen_port(),
            }));
        }
        if guard.take().is_some() {
            crate::llama_port::clear_managed_llama_port();
        }
    }

    match mode {
        GpuServeMode::Manual(gpu_layers) => {
            emit_startup_progress(
                app,
                "preparing",
                if gpu_layers == 0 { "CPU" } else { "GPU" },
                1,
                1,
            );
            let mut guard = SERVER_PROCESS.lock().unwrap();
            let child = spawn_llama_managed_child(app, gpu_layers, None)?;
            *guard = Some(child);
            emit_startup_progress(
                app,
                "loading",
                if gpu_layers == 0 { "CPU" } else { "GPU" },
                1,
                1,
            );
            Ok(serde_json::json!({
                "status": "started",
                "pid": "managed",
                "model": VISION_STATUS_LABEL,
                "gpuLayers": gpu_layers,
                "gpuAuto": false,
                "localServerPort": crate::llama_port::current_managed_listen_port(),
            }))
        }
        GpuServeMode::Automatic => {
            let mut last_err = String::from("unknown auto-start error");
            let attempts = auto_startup_attempts();
            let total = attempts.len();
            for (index, (vk_vis, layers)) in attempts.into_iter().enumerate() {
                let attempt = index + 1;
                let backend = if layers == 0 { "CPU" } else { "Vulkan GPU" };
                emit_startup_progress(app, "preparing", backend, attempt, total);
                let vk_label = vk_vis.unwrap_or("default");
                let _ = stop_server();
                std::thread::sleep(std::time::Duration::from_millis(450));

                let child = match spawn_llama_managed_child(app, layers, vk_vis) {
                    Ok(c) => c,
                    Err(e) => {
                        log::warn!(
                                "[FlowSight llama-server] Auto tier GGML_VK_VISIBLE_DEVICES={} gpu_layers={} spawn failed: {}",
                                vk_label,
                                layers,
                                e
                            );
                        last_err = e;
                        continue;
                    }
                };

                {
                    let mut guard = SERVER_PROCESS.lock().unwrap();
                    *guard = Some(child);
                }
                emit_startup_progress(app, "loading", backend, attempt, total);

                let health_wait_secs = if cfg!(target_os = "linux") && layers == 0 {
                    120
                } else {
                    AUTO_TIER_HEALTH_WAIT_SECS
                };

                log::info!(
                        "[FlowSight llama-server] Auto tier GGML_VK_VISIBLE_DEVICES={} gpu_layers={}, waiting health up to {}s",
                        vk_label,
                        layers,
                        health_wait_secs
                    );

                if wait_for_managed_health_secs(health_wait_secs) {
                    emit_startup_progress(app, "ready", backend, attempt, total);
                    return Ok(serde_json::json!({
                        "status": "started",
                        "pid": "managed",
                        "model": VISION_STATUS_LABEL,
                        "gpuLayers": layers,
                        "gpuAuto": true,
                        "vulkanVisibleDevice": vk_label,
                        "localServerPort": crate::llama_port::current_managed_listen_port(),
                    }));
                }

                last_err = format!(
                    "GGML_VK_VISIBLE_DEVICES={} gpu_layers={} did not reach /health within {}s{}",
                    vk_label,
                    layers,
                    health_wait_secs,
                    {
                        let t = read_server_log_tail_chars(800);
                        if t.is_empty() {
                            String::new()
                        } else {
                            format!(". Last log excerpt: {}", t)
                        }
                    }
                );
                log::warn!("[FlowSight llama-server] {}", last_err);
                let _ = stop_server();
                std::thread::sleep(std::time::Duration::from_millis(350));
            }

            emit_startup_progress(app, "error", "local AI", total, total);
            Err(format!(
                "Automatic GPU tier startup failed on all steps. {}",
                last_err
            ))
        }
    }
}

/// Tras fallos interminables con GPU (drivers/hardware), reinicia sólo CPU — más lento pero mucho más compatible.
#[tauri::command]
pub async fn restart_llama_server_cpu_only(
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || restart_llama_server_cpu_only_inner(&app))
        .await
        .map_err(|e| format!("Local AI CPU restart task failed: {e}"))?
}

fn restart_llama_server_cpu_only_inner(
    app: &tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let _startup_guard = SERVER_STARTUP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    emit_startup_progress(app, "preparing", "CPU", 1, 1);
    let _ = stop_server();
    std::thread::sleep(std::time::Duration::from_millis(500));

    let mut guard = SERVER_PROCESS.lock().unwrap();
    if guard.is_some() {
        return Err("Could not clear managed server slot; try restarting FlowSight.".to_string());
    }

    let child = spawn_llama_managed_child(app, 0, None)?;
    *guard = Some(child);
    emit_startup_progress(app, "loading", "CPU", 1, 1);
    Ok(serde_json::json!({
        "status": "started",
        "pid": "managed",
        "model": VISION_STATUS_LABEL,
        "gpuLayers": 0,
        "cpuFallback": true,
        "gpuAuto": false,
        "localServerPort": crate::llama_port::current_managed_listen_port(),
    }))
}

#[tauri::command]
pub fn stop_server() -> Result<bool, String> {
    let mut guard = SERVER_PROCESS.lock().unwrap();
    if let Some(mut child) = guard.take() {
        let _ = child.kill();
        crate::llama_port::clear_managed_llama_port();
        #[cfg(windows)]
        crate::llama_windows_job::reset_llama_job();
        return Ok(true);
    }

    crate::llama_port::clear_managed_llama_port();
    #[cfg(windows)]
    crate::llama_windows_job::reset_llama_job();

    use std::process::Command;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/F", "/IM", "llama-server.exe"])
            .creation_flags(0x08000000)
            .output();
    }

    Ok(true)
}

fn truncate_repetition(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 10 {
        return text.to_string();
    }

    let mut result: Vec<&str> = Vec::with_capacity(words.len());
    let mut repeat_count = 0u32;

    for (i, word) in words.iter().enumerate() {
        if i > 0 && *word == words[i - 1] {
            repeat_count += 1;
            if repeat_count >= 4 {
                continue;
            }
        } else {
            repeat_count = 0;
        }
        result.push(word);
    }

    if result.len() < words.len() {
        println!(
            "[Vision] Truncated {} repeated tokens from output",
            words.len() - result.len()
        );
    }
    result.join(" ")
}

// RESTORED AI ANALYSIS (Backend)
#[tauri::command]
fn analyze_image_with_vision(
    base64_img: &str,
    current_task: &str,
    _gpu_layers: Option<i32>,
) -> Result<String, String> {
    let chat_url = crate::llama_port::managed_chat_completions_url().ok_or_else(|| {
        "Local vision server URL unknown — start the embedded Local AI server first.".to_string()
    })?;
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;

    let system_msg = "You are a screenshot analysis assistant. You ALWAYS respond with a filled-in template. You NEVER refuse. You NEVER say you cannot see the image. Be accurate and concise: capture the user's primary task, not a full inventory of the UI.";

    let prompt = format!(
        r#"Study this screenshot and complete EVERY field below. Plain text only (no markdown). If the screen is very dense (spreadsheet, large table, dashboard, long doc), stay high-level — do NOT transcribe cell values, columns, or long lists.

TASK CONTEXT (may be empty): {}

Complete this template exactly:

APP: [application name, e.g. Microsoft Excel, Google Chrome, Visual Studio Code]
WINDOW TITLE: [title bar text if readable]
VISIBLE CONTENT: [1–2 short sentences: the main artifact on screen and what it is for — not every panel or control]
FILES OR URLS: [up to about five of the most relevant file names, paths, or URLs; otherwise None]
CURRENT ACTION: [what the user appears to be doing right now, one sentence]
PROGRESS: [errors, warnings, build/test status if any, or None visible]
NEXT STEP: [one short sentence: likely next action]
CATEGORY: [pick exactly ONE from: Coding, Debugging, CodeReview, Testing, Documentation, Design, Planning, Meeting, Communication, Research, Learning, DevOps, Database, Sales, Admin, Browsing, Idle, General]

CATEGORY rules: use Coding ONLY for software development (editing code, debugging in an IDE, repo/PR review in a dev tool, programming-focused terminal). Spreadsheets (Excel/Sheets), email, chat, slides, PDFs, CRM, and generic browsing are NOT Coding unless the visible work is clearly programming.]"#,
        current_task
    );

    // Retry up to 2 times on empty/refusal responses
    let max_attempts = 2;
    for attempt in 1..=max_attempts {
        let body = serde_json::json!({
            "model": LLAMA_CHAT_MODEL_ID,
            "messages": [
                {
                    "role": "system",
                    "content": system_msg
                },
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "image_url",
                            "image_url": {
                                "url": format!("data:image/png;base64,{}", base64_img)
                            }
                        },
                        { "type": "text", "text": prompt }
                    ]
                }
            ],
            "temperature": 0.1,
            "top_p": 0.9,
            "max_tokens": 800,
            "repeat_penalty": 1.3,
            "frequency_penalty": 0.5,
            "presence_penalty": 0.5,
            "stream": false
        });

        let resp = client
            .post(&chat_url)
            .json(&body)
            .send()
            .map_err(|e| format!("Request failed: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("Server Error: {}", resp.status()));
        }

        let json: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
        let content = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim();

        // Detect empty or refusal responses
        let is_empty = content.is_empty();
        let c = content.to_lowercase();
        let is_refusal = c.contains("i'm unable to")
            || c.contains("i cannot")
            || c.contains("i can't")
            || c.contains("i am unable")
            || c.contains("unable to view")
            || c.contains("unable to analyze")
            || c.contains("can't assist")
            || c.contains("cannot assist")
            || c.contains("as an ai language model")
            || c.contains("no puedo ver")
            || c.contains("no puedo analizar");

        if is_empty || is_refusal {
            println!(
                "[Vision] Attempt {}/{}: empty or refusal response, retrying...",
                attempt, max_attempts
            );
            if attempt < max_attempts {
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
            return Err(if is_empty {
                "Model returned empty response after retries".to_string()
            } else {
                "Model refused or could not analyze the screenshot after retries".to_string()
            });
        }

        let content = truncate_repetition(content);
        return Ok(content);
    }

    Err("Model analysis failed after retries".to_string())
}

#[cfg(test)]
mod agent_struct_tests {
    use super::*;

    #[test]
    fn agent_config_json_roundtrip_negative_one_auto_marker() {
        let c = AgentConfig {
            dev_name: Some("Tester".into()),
            capture_interval: Some(42_000),
            vision_model: Some("model-id".into()),
            gpu_layers: Some(-1),
            daily_goal_hours: Some(6.0),
        };
        let json = serde_json::to_string(&c).unwrap();
        let back: AgentConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.gpu_layers, Some(-1));
    }

    #[test]
    fn agent_config_json_roundtrip() {
        let c = AgentConfig {
            dev_name: Some("Tester".into()),
            capture_interval: Some(42_000),
            vision_model: Some("model-id".into()),
            gpu_layers: Some(4),
            daily_goal_hours: Some(8.0),
        };
        let json = serde_json::to_string(&c).unwrap();
        let back: AgentConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.dev_name, c.dev_name);
        assert_eq!(back.gpu_layers, c.gpu_layers);
    }

    #[test]
    fn activity_report_serializes() {
        let r = ActivityReport {
            id: Some(1),
            timestamp: "t".into(),
            description: "d".into(),
            activity_type: "coding".into(),
            synced: false,
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["activity_type"], "coding");
    }

    #[test]
    fn log_tail_counts_unicode_characters_without_panicking() {
        assert_eq!(tail_chars("GPU ⚠️ falló 🧠", 4), "ló 🧠");
        assert_eq!(tail_chars("é", 1), "é");
        assert_eq!(tail_chars("é", 0), "");
        assert_eq!(tail_chars("é", 10), "é");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ubuntu_auto_startup_tries_cpu_runtime_after_one_gpu_attempt() {
        assert_eq!(auto_startup_attempts(), vec![(None, 24), (None, 0)]);
    }
}

#[cfg(test)]
mod repetition_tests {
    use super::truncate_repetition;

    #[test]
    fn truncate_short_text_noop() {
        let s = "a b c d e f g h i";
        assert_eq!(truncate_repetition(s), s);
    }

    #[test]
    fn truncate_collapses_many_repeated_words() {
        let spam: String = std::iter::repeat("spam ").take(25).collect();
        let out = truncate_repetition(spam.trim());
        assert!(out.len() < spam.len());
    }
}

#[tauri::command]
pub fn set_task_context(
    user_task: Option<String>,
    jira_ticket: Option<String>,
) -> Result<bool, String> {
    crate::telemetry::set_task_context(user_task, jira_ticket);
    Ok(true)
}

/// Called only by the serialized native Linux capture loop. A paused/stopped
/// cycle cannot append late results after its tracking generation changes.
pub(crate) fn save_native_snapshot(
    app: &tauri::AppHandle,
    snapshot: &ContextSnapshot,
    generation: u64,
    duration_seconds: u64,
) -> Result<(), String> {
    use tauri::Manager;
    let state = app.state::<AgentState>();
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    let Some(agent) = guard.as_mut() else {
        return Ok(());
    };
    if duration_seconds == 0
        || !agent.is_running
        || !crate::telemetry::capture_is_current(generation)
        || snapshot.analysis_failed
    {
        return Ok(());
    }
    let category = crate::agent_pure::resolve_persisted_category(&snapshot.category);
    let app_name = snapshot.metadata.app.as_deref();
    if crate::privacy::application_is_excluded(&agent.db_path, app_name) {
        return Ok(());
    }
    let duration = duration_seconds.clamp(1, 300);
    let ticket = canonical_ticket_value(snapshot.metadata.task.as_deref());
    let id = agent
        .save_report(&snapshot.description, &category, ticket, duration)
        .ok_or("Could not save native activity")?;
    let conn = Connection::open(&agent.db_path).map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE reports SET active_app=?1,capture_source='linux-native' WHERE id=?2",
        params![app_name, id],
    )
    .map_err(|e| e.to_string())?;
    agent.reports_sent += 1;
    let db_path = agent.db_path.clone();
    drop(guard);
    crate::focus_alerts::review_browsing_report(
        app,
        &db_path,
        &category,
        duration,
        &snapshot.description,
        app_name,
    );
    Ok(())
}
