mod agent;
mod agent_pure;
mod anonymous_analytics;
mod auth;
mod calendar_companion;
mod coach_chat;
pub mod context;
mod desktop_presence;
mod entitlements;
mod focus_alerts;
mod focus_semantics;
mod insights_local;
mod jira;
mod language;
mod linear;
#[cfg(target_os = "linux")]
mod linux_silent_capture;
mod llama_bin;
mod llama_port;
mod llama_windows_job;
mod local_agent;
pub mod mcp;
mod model_assets;
mod oauth_env;
pub mod paths;
mod privacy;
mod report_schedule;
pub mod screen_capture;
mod screenshot_disk;
mod secure_config;
mod sync;
mod sync_env;
mod sync_pure;
mod telemetry;
mod user_preferences;
mod vision_model;

use tauri::Manager;

use agent::{
    capture_screen_command, check_local_server, check_ollama, get_activity_log, get_config,
    get_status, get_today_history, get_week_summary, initialize_agent,
    llama_managed_process_status, llama_server_log_tail, restart_llama_server_cpu_only,
    save_activity, start_monitoring, stop_monitoring, update_config, AgentState,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_autostart::Builder::new().arg("--flowsight-autostart").build())
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window)=app.get_webview_window("main") {
                let _=window.show(); let _=window.unminimize(); let _=window.set_focus();
            }
        }))
        .manage(AgentState::default())
        .invoke_handler(tauri::generate_handler![
            local_agent::get_local_agent_data,
            local_agent::session_plan::propose_session_plan,
            local_agent::session_plan::confirm_session_plan,
            local_agent::session_plan::cancel_session_plan,
            local_agent::session_plan::abandon_session_plan,
            local_agent::get_local_agent_tools,
            local_agent::propose_local_agent_tool,
            local_agent::ask_local_agent,
            local_agent::confirm_local_agent_action,
            local_agent::cancel_local_agent_action,
            local_agent::control_local_focus_block,
            local_agent::browser_bridge::get_browser_pairing,
            local_agent::browser_bridge::open_browser_extension_store,
            local_agent::browser_bridge::open_browser_extension_folder,
            local_agent::connectors::get_local_agent_connections,
            local_agent::connectors::save_local_agent_connection,
            local_agent::connectors::remove_local_agent_connection,
            local_agent::connectors::set_local_agent_providers,
            calendar_companion::get_calendar_companion_status,
            calendar_companion::start_calendar_oauth,
            calendar_companion::disconnect_calendar,
            calendar_companion::set_calendar_auto_publish,
            calendar_companion::open_current_calendar_event,
            coach_chat::get_coach_chat_messages,
            coach_chat::get_coach_chat_usage,
            coach_chat::send_coach_chat_message,
            language::get_language_preference,
            language::set_language_preference,
            model_assets::local_model_status,
            model_assets::download_local_model,
            privacy::get_privacy_settings,
            privacy::update_privacy_settings,
            privacy::export_personal_data,
            privacy::delete_local_data,
            privacy::delete_cloud_account,
            anonymous_analytics::get_analytics_consent,
            anonymous_analytics::set_analytics_consent,
            anonymous_analytics::sync_anonymous_analytics,
            anonymous_analytics::submit_product_feedback,
            desktop_presence::get_desktop_preferences,
            desktop_presence::set_launch_at_login,
            desktop_presence::set_start_monitoring_at_login,
            desktop_presence::dismiss_desktop_prompt,
            desktop_presence::set_focus_alerts_enabled,
            desktop_presence::set_contextual_focus_alerts_enabled,
            report_schedule::get_weekly_report_schedule,
            report_schedule::save_weekly_report_schedule,
            report_schedule::save_scheduled_report_pdf,
            paths::save_pdf_to_downloads,
            paths::open_path_in_file_manager,
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
            jira::fetch_jira_profile,
            sync::save_user_session,
            sync::clear_user_session,
            sync::get_current_user,
            sync::join_team,
            sync::get_user_teams,
            sync::set_active_team,
            entitlements::get_entitlements,
            entitlements::refresh_entitlements,
            insights_local::generate_local_status_report,
            mcp::get_mcp_connection_info,
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
            auth::login_with_code,
            // Linear commands
            linear::fetch_linear_tasks,
            agent::set_task_context,
            // History commands
            get_today_history,
            get_week_summary,
            paths::get_flowsight_user_paths,
            screen_capture::ensure_linux_capture_dependencies,
            #[cfg(target_os = "linux")]
            linux_silent_capture::ensure_screencast_ready,
        ])
    .setup(|app| {
      telemetry::start(app.handle().clone());
      language::initialize();
      desktop_presence::setup_tray(app)?;
      if let Err(error)=local_agent::browser_bridge::start() {
          log::warn!("Browser bridge unavailable: {error}");
      }
      local_agent::start_maintenance(app.handle().clone());
      calendar_companion::start_monitor();
      report_schedule::start_check_loop(app.handle().clone());
      if let Some(window) = app.get_webview_window("main") {
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
    .on_window_event(|window,event| {
        if window.label()=="main" && desktop_presence::should_hide_on_close() {
            if let tauri::WindowEvent::CloseRequested { api, .. }=event {
                api.prevent_close(); let _=window.hide();
            }
        }
    })
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
