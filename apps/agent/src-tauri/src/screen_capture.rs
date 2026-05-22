//! Platform-specific screen capture. Linux avoids the `screenshots` crate (libwayshot / ZwlrScreencopy)
//! and uses system tools instead.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use image::DynamicImage;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "linux")]
static LINUX_AUTOINSTALL_ATTEMPTED: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "linux")]
static LINUX_PERMISSIONS_GRANTED: AtomicBool = AtomicBool::new(false);

/// Tauri bundle identifier — must match `tauri.conf.json` and the portal permission store.
#[cfg(target_os = "linux")]
pub const LINUX_PORTAL_APP_ID: &str = "ai.flowsight.agent";

pub fn capture_screen() -> Result<(String, PathBuf), String> {
    #[cfg(windows)]
    {
        capture_windows()
    }
    #[cfg(target_os = "macos")]
    {
        capture_macos()
    }
    #[cfg(target_os = "linux")]
    {
        capture_linux()
    }
    #[cfg(not(any(
        windows,
        target_os = "macos",
        target_os = "linux"
    )))]
    {
        Err("Screen capture is not implemented for this operating system.".to_string())
    }
}

fn tmp_png_name(prefix: &str) -> PathBuf {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("{prefix}_{ms}.png"))
}

fn debug_capture_dir() -> PathBuf {
    crate::paths::screenshots_tmp_dir().unwrap_or_else(|_| {
        dirs::desktop_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("flowsight_screenshots_tmp")
    })
}

fn process_rgba_dynamic(img: DynamicImage) -> Result<(String, PathBuf), String> {
    let img = img.resize(960, 540, image::imageops::FilterType::Lanczos3);
    let mut png = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png),
        image::ImageFormat::Png,
    )
    .map_err(|e| e.to_string())?;

    let debug_dir = debug_capture_dir();
    let _ = std::fs::create_dir_all(&debug_dir);
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let filename = format!("capture_{}.png", timestamp);
    let debug_path = debug_dir.join(filename);
    let _ = std::fs::write(&debug_path, &png);

    Ok((BASE64.encode(&png), debug_path))
}

fn process_png_bytes(png: &[u8]) -> Result<(String, PathBuf), String> {
    let img = image::load_from_memory(png).map_err(|e| e.to_string())?;
    process_rgba_dynamic(img)
}

#[cfg(target_os = "linux")]
pub fn png_is_mostly_black_pub(png: &[u8]) -> bool {
    png_is_mostly_black(png)
}

#[cfg(target_os = "linux")]
fn png_is_mostly_black(png: &[u8]) -> bool {
    let Ok(img) = image::load_from_memory(png) else {
        return true;
    };
    let rgb = img.to_rgb8();
    let n = rgb.pixels().len();
    if n == 0 {
        return true;
    }
    let sum: u64 = rgb
        .pixels()
        .map(|p| (u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2])) / 3)
        .sum();
    let avg = sum as f64 / n as f64;
    avg < 2.0
}

#[cfg(target_os = "linux")]
fn linux_finish_png(bytes: Vec<u8>) -> Option<Result<(String, PathBuf), String>> {
    if bytes.len() < 500 || png_is_mostly_black(&bytes) {
        return None;
    }
    Some(process_png_bytes(&bytes))
}

#[cfg(windows)]
fn capture_windows() -> Result<(String, PathBuf), String> {
    use screenshots::Screen;

    let screens = Screen::all().map_err(|e| e.to_string())?;
    let screen = screens.first().ok_or("No display found")?;
    let captured = screen.capture().map_err(|e| e.to_string())?;
    let (width, height) = captured.dimensions();
    let img = DynamicImage::ImageRgba8(
        image::RgbaImage::from_raw(width, height, captured.into_raw())
            .ok_or("Failed to build image buffer")?,
    );
    process_rgba_dynamic(img)
}

#[cfg(target_os = "macos")]
fn capture_macos() -> Result<(String, PathBuf), String> {
    let tmp = tmp_png_name("flowsight_mac");
    let tmp_s = tmp.to_str().ok_or("Invalid temp path")?;

    let status = Command::new("screencapture")
        .args(["-x", "-t", "png", tmp_s])
        .status()
        .map_err(|e| {
            format!(
                "screencapture failed ({e}). Grant Screen Recording for FlowSight in System Settings → Privacy."
            )
        })?;

    if !status.success() {
        return Err("screencapture exited with a non-zero status".to_string());
    }

    let bytes = std::fs::read(&tmp).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&tmp);
    process_png_bytes(&bytes)
}

#[cfg(target_os = "linux")]
fn try_read_tmp_png(tmp: &Path) -> Option<Vec<u8>> {
    std::fs::read(tmp).ok().filter(|b| b.len() > 500)
}

#[cfg(target_os = "linux")]
const LINUX_CAPTURE_CANDIDATES: &[&str] = &[
    "grim",
    "gnome-screenshot",
    "spectacle",
    "scrot",
    "maim",
    "import",
    "magick",
    "xfce4-screenshooter",
    "flameshot",
];

#[cfg(target_os = "linux")]
fn tool_in_path(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// True if any known screenshot CLI is available (fast `which` checks).
#[cfg(target_os = "linux")]
pub fn linux_any_capture_tool_in_path() -> bool {
    LINUX_CAPTURE_CANDIDATES.iter().any(|c| tool_in_path(c))
}

#[cfg(target_os = "linux")]
fn has_command(name: &str) -> bool {
    tool_in_path(name)
}

/// One-shot per process: try `pkexec` to install distro packages when nothing is in PATH.
#[cfg(target_os = "linux")]
fn try_distro_install_capture_tools() -> Result<String, String> {
    let wayland = std::env::var("WAYLAND_DISPLAY").is_ok();

    // GNOME Wayland uses xdg-desktop-portal (no gnome-screenshot package — avoids shutter flash).
    let gnome = linux_is_gnome_session();
    let apt_pkgs = if wayland && gnome {
        "scrot"
    } else if wayland {
        "grim scrot"
    } else {
        "scrot maim imagemagick grim"
    };

    let status = if has_command("apt-get") {
        let script = format!(
            "export DEBIAN_FRONTEND=noninteractive; apt-get update -qq && apt-get install -y {}",
            apt_pkgs
        );
        Command::new("pkexec").args(["sh", "-c", &script]).status()
    } else if has_command("dnf") {
        let pkgs = if wayland && linux_is_gnome_session() {
            "scrot"
        } else if wayland {
            "grim scrot"
        } else {
            "scrot maim ImageMagick grim"
        };
        let script = format!("dnf install -y {}", pkgs);
        Command::new("pkexec").args(["sh", "-c", &script]).status()
    } else if has_command("pacman") {
        let pkgs = if wayland && linux_is_gnome_session() {
            "scrot"
        } else if wayland {
            "grim scrot"
        } else {
            "scrot maim imagemagick grim"
        };
        let script = format!("pacman -S --needed --noconfirm {}", pkgs);
        Command::new("pkexec").args(["sh", "-c", &script]).status()
    } else if has_command("zypper") {
        let pkgs = if wayland && linux_is_gnome_session() {
            "scrot"
        } else if wayland {
            "grim scrot"
        } else {
            "scrot maim ImageMagick grim"
        };
        let script = format!("zypper --non-interactive install -y {}", pkgs);
        Command::new("pkexec").args(["sh", "-c", &script]).status()
    } else {
        return Err(
            "No supported package manager (apt-get, dnf, pacman, zypper). Install grim or scrot manually."
                .to_string(),
        );
    };

    let st = status.map_err(|e| format!("Could not run pkexec: {e}"))?;
    if !st.success() {
        return Err(
            "Package install exited with an error (cancelled pkexec or no rights?). Try: sudo apt install grim scrot"
                .to_string(),
        );
    }

    if linux_any_capture_tool_in_path() {
        Ok("Packages installed; at least one capture tool is now on PATH.".to_string())
    } else {
        Err(
            "Install finished but no capture tool found in PATH. Log out and back in, or install grim/scrot manually."
                .to_string(),
        )
    }
}

/// Grant xdg-desktop-portal screenshot permission without a GNOME dialog (PermissionStore).
#[cfg(target_os = "linux")]
fn linux_grant_portal_screenshot_permission(app_id: &str) -> bool {
    Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.impl.portal.PermissionStore",
            "--object-path",
            "/org/freedesktop/impl/portal/PermissionStore",
            "--method",
            "org.freedesktop.impl.portal.PermissionStore.SetPermission",
            "screenshot",
            "true",
            "screenshot",
            app_id,
            "['yes']",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// App IDs the portal may use for this process (systemd scope name, dev binary `app`, etc.).
#[cfg(target_os = "linux")]
pub fn linux_collect_portal_app_ids() -> Vec<String> {
    use std::collections::BTreeSet;

    let mut ids = BTreeSet::new();
    ids.insert(LINUX_PORTAL_APP_ID.to_string());
    ids.insert("app".to_string());
    ids.insert("flowsight-agent".to_string());
    ids.insert("FlowSight Agent".to_string());

    if let Ok(exe) = std::env::current_exe() {
        if let Some(stem) = exe.file_stem().and_then(|s| s.to_str()) {
            if !stem.is_empty() {
                ids.insert(stem.to_string());
            }
        }
    }

    if let Ok(cgroup) = std::fs::read_to_string("/proc/self/cgroup") {
        for line in cgroup.lines() {
            let Some(scope) = line.rsplit('/').next() else {
                continue;
            };
            let Some(base) = scope.strip_suffix(".scope") else {
                continue;
            };
            let rest = base
                .strip_prefix("app-gnome-")
                .or_else(|| base.strip_prefix("app-"));
            let Some(rest) = rest else {
                continue;
            };
            let Some(dash) = rest.rfind('-') else {
                continue;
            };
            let (app_part, pid_part) = rest.split_at(dash);
            if !pid_part[1..].chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let decoded = app_part
                .replace("\\x2d", "-")
                .replace("\\x2e", ".");
            if !decoded.is_empty() {
                ids.insert(decoded);
            }
        }
    }

    ids.into_iter().collect()
}

/// Pre-grant portal screenshot access for every plausible app id (no Settings UI required).
#[cfg(target_os = "linux")]
pub fn linux_auto_grant_screenshot_permissions() -> Vec<String> {
    if LINUX_PERMISSIONS_GRANTED.swap(true, Ordering::SeqCst) {
        return Vec::new();
    }
    let mut granted = Vec::new();
    for id in linux_collect_portal_app_ids() {
        if linux_grant_portal_screenshot_permission(&id) {
            log::info!("[FlowSight] Granted portal screenshot permission for '{id}'");
            granted.push(id);
        }
    }
    granted
}

/// Called from Start: grant portal permission + ensure grim/scrot when needed (Linux only).
#[tauri::command]
pub fn ensure_linux_capture_dependencies() -> Result<serde_json::Value, String> {
    #[cfg(target_os = "linux")]
    {
        let granted = linux_auto_grant_screenshot_permissions();
        let wayland = std::env::var("WAYLAND_DISPLAY").is_ok();

        if linux_is_gnome_session() && wayland {
            let sc = crate::linux_silent_capture::linux_auto_grant_screencast_permissions();
            return Ok(serde_json::json!({
                "status": "ok",
                "message": "Silent capture via PipeWire ScreenCast (no shutter flash or sound).",
                "granted_app_ids": granted,
                "screencast_app_ids": sc,
            }));
        }

        if linux_any_capture_tool_in_path() {
            return Ok(serde_json::json!({
                "status": "ok",
                "message": "Screen capture tools already available"
            }));
        }
        if LINUX_AUTOINSTALL_ATTEMPTED.swap(true, Ordering::SeqCst) {
            return Ok(serde_json::json!({
                "status": "already_attempted",
                "message": "Automatic install was already tried this session. On GNOME: Settings → Privacy → Screen Capture. Else: sudo apt install grim scrot"
            }));
        }

        if !has_command("pkexec") {
            return Ok(serde_json::json!({
                "status": "install_failed",
                "message": "pkexec not found. On GNOME allow Screen Capture for FlowSight; else: sudo apt install grim scrot"
            }));
        }

        match try_distro_install_capture_tools() {
            Ok(detail) => {
                return Ok(serde_json::json!({
                    "status": "installed",
                    "message": detail
                }));
            }
            Err(e) => {
                return Ok(serde_json::json!({
                    "status": "install_failed",
                    "message": e
                }));
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(serde_json::json!({
            "status": "skipped",
            "message": "Not Linux"
        }))
    }
}

#[cfg(target_os = "linux")]
pub fn linux_is_gnome_session() -> bool {
    let check = |v: &str| {
        let l = v.to_lowercase();
        l.contains("gnome") || l.contains("ubuntu") || l.contains("unity")
    };
    if std::env::var("XDG_CURRENT_DESKTOP")
        .map(|v| check(&v))
        .unwrap_or(false)
    {
        return true;
    }
    if std::env::var("DESKTOP_SESSION")
        .map(|v| check(&v))
        .unwrap_or(false)
    {
        return true;
    }
    // Tauri/GTK apps often lack desktop env vars; detect GNOME by installed shell.
    std::path::Path::new("/usr/bin/gnome-shell").exists()
        || std::path::Path::new("/usr/libexec/gnome-session-binary").exists()
}

/// Parse absolute paths to image files from `gdbus call` stdout/stderr (GNOME returns `filename_used`).
#[cfg(target_os = "linux")]
fn paths_from_gdbus_screenshot_reply(s: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for sep in ['\'', '"'] {
        for frag in s.split(sep) {
            let t = frag.trim();
            if !t.starts_with('/') {
                continue;
            }
            let lower = t.to_ascii_lowercase();
            if lower.ends_with(".png")
                || lower.ends_with(".jpg")
                || lower.ends_with(".jpeg")
            {
                out.push(PathBuf::from(t));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// GNOME Shell full-screen capture via D-Bus (`flash=false`) — no white shutter animation.
#[cfg(target_os = "linux")]
pub fn try_linux_gnome_shell_capture() -> Option<Vec<u8>> {
    linux_auto_grant_screenshot_permissions();
    linux_raw_gnome_shell_dbus()
}

/// GNOME Shell full-screen capture via D-Bus (`flash=false`). Often denied on modern GNOME;
/// when it succeeds, Mutter may write to `filename_used` instead of the path we pass in.
#[cfg(target_os = "linux")]
fn linux_raw_gnome_shell_dbus() -> Option<Vec<u8>> {
    if !tool_in_path("gdbus") {
        return None;
    }
    let tmp = tmp_png_name("flowsight_gnome_dbus");
    let _ = std::fs::remove_file(&tmp);
    let tmp_s = tmp.to_str()?;
    let out = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.gnome.Shell",
            "--object-path",
            "/org/gnome/Shell/Screenshot",
            "--method",
            "org.gnome.Shell.Screenshot.Screenshot",
            "false",
            "false",
            tmp_s,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }

    let reply = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let mut try_paths = paths_from_gdbus_screenshot_reply(&reply);
    if !try_paths.iter().any(|p| p == &tmp) {
        try_paths.push(tmp.clone());
    }

    for path in try_paths {
        if !path.exists() {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&path) {
            if path == tmp {
                let _ = std::fs::remove_file(&tmp);
            }
            if bytes.len() >= 500 {
                return Some(bytes);
            }
        }
    }

    let _ = std::fs::remove_file(&tmp);
    None
}

/// GNOME: never use Screenshot portal (white flash + shutter sound on every capture).
#[cfg(target_os = "linux")]
pub fn linux_must_avoid_screenshot_portal() -> bool {
    linux_is_gnome_session()
}

/// Silent capture via xdg-desktop-portal Screenshot. Blocked on GNOME (flashes).
#[cfg(target_os = "linux")]
pub async fn try_linux_portal_capture() -> Option<Vec<u8>> {
    if linux_is_gnome_session() {
        log::warn!(
            "[FlowSight] Screenshot portal skipped on GNOME (use ScreenCast — no flash)"
        );
        return None;
    }
    use ashpd::desktop::screenshot::Screenshot;

    linux_auto_grant_screenshot_permissions();

    let portal_req = match Screenshot::request()
        .interactive(false)
        .modal(false)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            log::warn!("[FlowSight] portal screenshot request failed: {e}");
            return None;
        }
    };

    let resp = match portal_req.response() {
        Ok(r) => r,
        Err(e) => {
            log::warn!("[FlowSight] portal screenshot denied or failed: {e}");
            return None;
        }
    };

    let path = resp.uri().to_file_path().ok()?;
    let bytes = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    if bytes.len() < 500 || png_is_mostly_black(&bytes) {
        log::warn!("[FlowSight] portal screenshot returned empty or black frame");
        return None;
    }
    log::info!("[FlowSight] Linux screen capture succeeded via portal");
    Some(bytes)
}

/// Turn raw portal PNG bytes into base64 + debug path (after async portal capture).
#[cfg(target_os = "linux")]
pub fn finish_linux_png_bytes(bytes: Vec<u8>) -> Result<(String, PathBuf), String> {
    match linux_finish_png(bytes) {
        Some(Ok(r)) => Ok(r),
        Some(Err(e)) => Err(e),
        None => Err("Screenshot was empty or all-black.".to_string()),
    }
}

/// ScreenCast frames may be dark on first buffer; still run through vision.
#[cfg(target_os = "linux")]
pub fn finish_linux_png_bytes_screencast(bytes: Vec<u8>) -> Result<(String, PathBuf), String> {
    if bytes.len() < 500 {
        return Err("ScreenCast frame too small.".to_string());
    }
    process_png_bytes(&bytes)
}

#[cfg(target_os = "linux")]
fn linux_raw_grim_stdout() -> Option<Vec<u8>> {
    let out = Command::new("grim").arg("-").output().ok()?;
    if out.status.success() && out.stdout.len() >= 500 {
        Some(out.stdout)
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
fn linux_raw_grim_file() -> Option<Vec<u8>> {
    let tmp = tmp_png_name("flowsight_grim");
    let _ = std::fs::remove_file(&tmp);
    let tmp_s = tmp.to_str()?;
    let out = Command::new("grim").arg(tmp_s).output().ok()?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    let bytes = try_read_tmp_png(&tmp)?;
    let _ = std::fs::remove_file(&tmp);
    Some(bytes)
}

#[cfg(target_os = "linux")]
fn linux_raw_scrot() -> Option<Vec<u8>> {
    let tmp = tmp_png_name("flowsight_scrot");
    let tmp_s = tmp.to_str()?;
    if !Command::new("scrot")
        .arg(tmp_s)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    let bytes = try_read_tmp_png(&tmp)?;
    let _ = std::fs::remove_file(&tmp);
    Some(bytes)
}

#[cfg(target_os = "linux")]
fn linux_capture_to_tmp(cmd: &str, args: &[&str]) -> Option<Vec<u8>> {
    let tmp = tmp_png_name(&format!("flowsight_{cmd}"));
    let tmp_s = tmp.to_str()?;
    let mut c = Command::new(cmd);
    for a in args {
        c.arg(a);
    }
    c.arg(tmp_s);
    if !c.status().map(|s| s.success()).unwrap_or(false) {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    let bytes = try_read_tmp_png(&tmp)?;
    let _ = std::fs::remove_file(&tmp);
    Some(bytes)
}

#[cfg(target_os = "linux")]
fn capture_linux() -> Result<(String, PathBuf), String> {
    let gnome = linux_is_gnome_session();
    let wayland = std::env::var("WAYLAND_DISPLAY").is_ok();

    // GNOME 49+ blocks legacy gnome-screenshot API (flash + shutter). Prefer portal, then Shell D-Bus.
    let mut pipeline: Vec<(&'static str, fn() -> Option<Vec<u8>>)> = Vec::new();

    // GNOME: never Shell Screenshot or portal (flash + sound). ScreenCast runs in agent.rs.
    if gnome {
        return Err(
            "On GNOME use ScreenCast only. Press Start monitoring to open a silent PipeWire session."
                .into(),
        );
    }

    if !gnome {
        pipeline.push(("grim-stdout", linux_raw_grim_stdout));
        pipeline.push(("grim-file", linux_raw_grim_file));
        pipeline.push(("gnome-shell-dbus", linux_raw_gnome_shell_dbus));
    }

    if !wayland {
        pipeline.push(("scrot", linux_raw_scrot));
        pipeline.push((
            "maim",
            || linux_capture_to_tmp("maim", &[] as &[&str]),
        ));
        pipeline.push((
            "import",
            || linux_capture_to_tmp("import", &["-window", "root"]),
        ));
        pipeline.push((
            "magick",
            || linux_capture_to_tmp("magick", &["import", "-window", "root"]),
        ));
    }

    if !(gnome && wayland) {
        pipeline.push((
            "spectacle",
            || linux_capture_to_tmp("spectacle", &["-b", "-o"]),
        ));
        pipeline.push((
            "xfce4-screenshooter",
            || linux_capture_to_tmp("xfce4-screenshooter", &["-f"]),
        ));
        pipeline.push((
            "flameshot",
            || linux_capture_to_tmp("flameshot", &["screen", "-p"]),
        ));
    }

    let mut black_rejects = 0u32;

    for (name, capture) in pipeline {
        let Some(bytes) = capture() else {
            continue;
        };
        if png_is_mostly_black(&bytes) {
            black_rejects += 1;
            log::warn!(
                "[FlowSight] Linux capture '{name}' returned a black frame; trying next method"
            );
            continue;
        }
        if let Some(Ok(result)) = linux_finish_png(bytes) {
            log::info!("[FlowSight] Linux screen capture succeeded via {name}");
            return Ok(result);
        }
    }

    let hint = if gnome && wayland {
        "On GNOME Wayland: open Settings → Privacy → Screen Capture and allow FlowSight Agent. \
         The app uses the silent portal API (not gnome-screenshot flash). \
         Optional: install the GNOME extension \"Allow gnome-screenshot\" if you use legacy tools."
    } else if wayland {
        "On wlroots/Hyprland/Sway install `grim`. On GNOME allow Screen Capture for FlowSight in Settings → Privacy."
    } else {
        "On X11 install `scrot`, `maim`, or `imagemagick`."
    };

    let extra = if black_rejects > 0 {
        format!(
            " {black_rejects} method(s) returned black frames (common: scrot on Wayland).",
        )
    } else {
        String::new()
    };

    Err(format!("Could not capture the screen ({hint}).{extra}"))
}
