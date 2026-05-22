//! Single source of truth for filesystem paths used by the agent.
//!
//! Motivation: hasta ahora cada módulo (`auth`, `sync`, `jira`, `linear`,
//! `agent`, `main`) construía por su cuenta `dirs::data_local_dir().unwrap().join("FlowSight")`
//! sin garantizar que el directorio existiese. En instalación fresca
//! (pre-login, pre-`initialize_agent`) el directorio no existe y cualquier
//! `Connection::open` o escritura de log fallaba silenciosamente.
//!
//! Todos los paths del runtime del usuario deben pasar por acá. Los paths
//! de recursos read-only bundlados con el instalador de Tauri se resuelven
//! vía `resource_local_llm_dir` y requieren `AppHandle`.

use std::path::PathBuf;

use crate::vision_model::{VISION_GGUF_FILENAME, VISION_MMPROJ_FILENAME};
use serde_json::json;
use tauri::{AppHandle, Manager};

const APP_DIR_NAME: &str = "FlowSight";
const DB_FILE: &str = "dev-agent.db";
const SERVER_LOG_FILE: &str = "server.log";
const AGENT_ERROR_LOG_FILE: &str = "agent_error.log";
const CRASH_LOG_FILE: &str = "crash.log";
const SCREENSHOTS_TMP_DIR: &str = "screenshots_tmp";

/// Carpeta local de datos de FlowSight dentro del perfil del usuario (creada si no existe).
///
/// Se resuelve con `dirs::data_local_dir()` (Known Folders en Windows, equivalentes en otros
/// SO): **ruta real en disco**, sin depender del idioma de la interfaz ni de variables de entorno
/// escritas en la UI para el usuario.
///
/// Es el único lugar escribible que usamos. Tiene que funcionar igual en dev,
/// en release portable y en instalaciones a `Program Files` (donde el
/// directorio de instalación NO es escribible por el usuario estándar).
pub fn app_data_dir() -> Result<PathBuf, String> {
    let base = dirs::data_local_dir().ok_or_else(|| "No local data dir available".to_string())?;
    let dir = base.join(APP_DIR_NAME);
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create {:?}: {}", dir, e))?;
    }
    Ok(dir)
}

/// Path a `dev-agent.db`. Crea el directorio padre si hace falta.
pub fn db_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join(DB_FILE))
}

/// Variante infalible para sitios donde no podemos propagar Result (panic hooks,
/// static init). En ese caso cae a `.` que es subóptimo pero no panica.
pub fn db_path_or_fallback() -> PathBuf {
    db_path().unwrap_or_else(|_| PathBuf::from(DB_FILE))
}

pub fn server_log_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join(SERVER_LOG_FILE))
}

pub fn auth_log_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("auth.log"))
}

pub fn agent_error_log_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join(AGENT_ERROR_LOG_FILE))
}

pub fn crash_log_path_or_fallback() -> PathBuf {
    app_data_dir()
        .map(|d| d.join(CRASH_LOG_FILE))
        .unwrap_or_else(|_| PathBuf::from(CRASH_LOG_FILE))
}

/// PNG de captura persistentes solo para depuración; mismo árbol que la BD/logs
/// que [`app_data_dir`], subcarpeta `screenshots_tmp\`, no el Escritorio ni la carpeta de instalación.
pub fn screenshots_tmp_dir() -> Result<PathBuf, String> {
    let dir = app_data_dir()?.join(SCREENSHOTS_TMP_DIR);
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create {:?}: {}", dir, e))?;
    }
    Ok(dir)
}

/// Detecta bloqueos de escritura (p. ej. **Controlled Folder Access**, ACL, AV) antes de confiar en SQLite.
pub fn verify_app_dir_filesystem_writable() -> Result<(), String> {
    let dir = app_data_dir()?;
    let probe = dir.join(".flowsight_fs_write_probe");
    std::fs::write(&probe, b"ok").map_err(|e| {
        format!(
            "Cannot write application data under {} ({e}). On Windows 11 this may be Controlled Folder Access or Defender blocking an unsigned app—allow FlowSight or add an exclusion for this folder.",
            dir.display()
        )
    })?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// Elimina capturas de depuración `capture_*` más antiguas que `max_age` (retención / cumplimiento).
pub fn prune_screenshots_tmp_older_than(max_age: std::time::Duration) -> Result<usize, String> {
    use std::time::SystemTime;

    let dir = screenshots_tmp_dir()?;
    let now = SystemTime::now();
    let mut removed = 0usize;
    let entries =
        std::fs::read_dir(&dir).map_err(|e| format!("Failed to read screenshots_tmp: {e}"))?;
    for ent in entries.filter_map(Result::ok) {
        let name = ent.file_name();
        let s = name.to_string_lossy();
        if !s.starts_with("capture_") {
            continue;
        }
        let Ok(meta) = ent.metadata() else {
            continue;
        };
        let Ok(mtime) = meta.modified() else {
            continue;
        };
        let Ok(elapsed) = now.duration_since(mtime) else {
            continue;
        };
        if elapsed > max_age {
            let p = ent.path();
            if std::fs::remove_file(&p).is_ok() {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

/// Writable `local_llm/` under app data (Linux/macOS: downloaded `llama-server` lands in `bin/`).
pub fn local_llm_storage_dir() -> Result<PathBuf, String> {
    let dir = app_data_dir()?.join("local_llm");
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create {:?}: {}", dir, e))?;
    }
    Ok(dir)
}

fn local_llm_has_weights(dir: &std::path::Path) -> bool {
    dir.join(VISION_GGUF_FILENAME).is_file() && dir.join(VISION_MMPROJ_FILENAME).is_file()
}

fn dev_repo_local_llm_root() -> Option<PathBuf> {
    let check = |dir: &std::path::Path| local_llm_has_weights(&dir.join("local_llm"));
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent()?.to_path_buf();
        for _ in 0..8 {
            if check(&dir) {
                return Some(dir.join("local_llm"));
            }
            if !dir.pop() {
                break;
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let mut dir = cwd;
        for _ in 0..6 {
            if check(&dir) {
                return Some(dir.join("local_llm"));
            }
            if !dir.pop() {
                break;
            }
        }
    }
    None
}

/// Resuelve el directorio de recursos bundlados donde viven los GGUF de visión.
///
/// En un `.exe` instalado, Tauri descomprime los `bundle.resources` dentro
/// de `<install>\resources\`. En dev, caemos al layout del repo (`<repo-root>/local_llm`).
pub fn resource_local_llm_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|e| format!("resource_dir unavailable: {}", e))?;

    let bundled = resource_dir.join("local_llm");
    if local_llm_has_weights(&bundled) {
        return Ok(bundled);
    }

    if let Some(dev) = dev_repo_local_llm_root() {
        return Ok(dev);
    }

    Err(format!(
        "local_llm vision weights not found (looked in bundled resources at {:?} and dev tree). \
         Run scripts/fetch-models or place {} and {} under local_llm/.",
        bundled, VISION_GGUF_FILENAME, VISION_MMPROJ_FILENAME
    ))
}

#[tauri::command]
pub fn get_flowsight_user_paths() -> Result<serde_json::Value, String> {
    let dir = app_data_dir()?;
    Ok(json!({
        "appDataDir": dir.to_string_lossy(),
        "serverLog": server_log_path()?.to_string_lossy(),
        "authLog": auth_log_path()?.to_string_lossy(),
        "agentErrorLog": agent_error_log_path()?.to_string_lossy(),
    }))
}
