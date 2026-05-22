mod vision_model;
mod llama_bin;
mod llama_port;
mod llama_windows_job;
mod screenshot_disk;
pub mod screen_capture;
#[cfg(target_os = "linux")]
mod linux_silent_capture;
mod agent;
mod agent_pure;
mod jira;
mod sync_env;
mod sync_pure;
mod sync;
mod auth;
mod linear;
mod oauth_env;
mod entitlements;
mod insights_local;
mod user_preferences;
pub mod context;
pub mod paths;

use tauri::Manager;

use agent::{
    AgentState, initialize_agent, get_config, update_config,
    get_status, start_monitoring, stop_monitoring,
    capture_screen_command, save_activity,
    get_activity_log, get_today_history, get_week_summary,
    check_ollama, check_local_server,
    llama_managed_process_status, llama_server_log_tail, restart_llama_server_cpu_only,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
        .manage(AgentState::default())
        .invoke_handler(tauri::generate_handler![
            initialize_agent,
            get_config,
            update_config,
            get_status,
            start_monitoring,
            stop_monitoring,
    capture_screen_command,
    save_activity,
    get_activity_log,
    check_ollama,
    check_local_server,
            agent::capture_context_snapshot,
            jira::fetch_jira_tasks,
            jira::start_jira_oauth,
            jira::fetch_jira_profile,
            sync::force_sync_now,
            sync::save_user_session,
            sync::clear_user_session,
            sync::get_current_user,
            sync::upload_activity_report,
            sync::join_team,
            sync::get_user_teams,
            sync::set_active_team,
            entitlements::get_entitlements,
            entitlements::save_entitlements_command,
            entitlements::refresh_entitlements,
            entitlements::fetch_cloud_insights,
            entitlements::request_cloud_insights,
            insights_local::generate_local_status_report,
            user_preferences::get_user_preferences,
            user_preferences::save_user_preferences_command,
            agent::start_server,
            agent::stop_server,
            llama_managed_process_status,
            llama_server_log_tail,
            restart_llama_server_cpu_only,
            // Auth commands
            auth::start_auth,
            auth::get_auth_session,
            auth::logout,
            auth::is_logged_in,
            auth::login_with_code,
            // Linear commands
            linear::fetch_linear_tasks,
            linear::fetch_linear_profile,
            // History commands
            get_today_history,
            get_week_summary,
            paths::get_flowsight_user_paths,
            screen_capture::ensure_linux_capture_dependencies,
            #[cfg(target_os = "linux")]
            linux_silent_capture::ensure_screencast_ready,
        ])
    .setup(|app| {
      if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_theme(Some(tauri::Theme::Light));
        // GNOME/Wayland: frameless custom titlebars often swallow clicks; use SSD on Linux.
        #[cfg(target_os = "linux")]
        {
          if let Err(e) = window.set_decorations(true) {
            log::warn!("[FlowSight] set_decorations(true) on Linux: {e}");
          }
          let granted = screen_capture::linux_auto_grant_screenshot_permissions();
          if !granted.is_empty() {
            log::info!(
              "[FlowSight] Portal screenshot permissions pre-granted for: {}",
              granted.join(", ")
            );
          }
          log::info!(
            "[FlowSight] Linux session: gnome={} wayland={} XDG_CURRENT_DESKTOP={:?} WAYLAND_DISPLAY={:?} XDG_SESSION_TYPE={:?}",
            screen_capture::linux_is_gnome_session(),
            crate::linux_silent_capture::linux_is_wayland_session(),
            std::env::var("XDG_CURRENT_DESKTOP"),
            std::env::var("WAYLAND_DISPLAY"),
            std::env::var("XDG_SESSION_TYPE"),
          );
        }
      }

      // Log a archivo en TODOS los builds. En release el usuario no ve stderr,
      // así que sin esto no hay forma de diagnosticar crashes post-login.
      // Los archivos quedan en %LOCALAPPDATA%\ai.flowsight.agent\logs\ (Windows)
      // o equivalente del OS según tauri-plugin-log.
      app.handle().plugin(
        tauri_plugin_log::Builder::default()
          .level(log::LevelFilter::Info)
          .targets([
            tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
            tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir { file_name: None }),
          ])
          .build(),
      )?;
      Ok(())
    })
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
