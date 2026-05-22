//! Platform-specific screen capture. Linux avoids the `screenshots` crate (libwayshot / ZwlrScreencopy)
//! and uses system tools instead.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use image::DynamicImage;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "linux")]
static LINUX_AUTOINSTALL_ATTEMPTED: AtomicBool = AtomicBool::new(false);

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

fn process_rgba_dynamic(img: DynamicImage) -> Result<(String, PathBuf), String> {
    let img = img.resize(960, 540, image::imageops::FilterType::Lanczos3);
    let mut png = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut png),
        image::ImageFormat::Png,
    )
    .map_err(|e| e.to_string())?;

    let desktop = dirs::desktop_dir().unwrap_or_else(|| PathBuf::from("."));
    let debug_dir = desktop.join("flowsight_screenshots_tmp");
    let _ = std::fs::create_dir_all(&debug_dir);
    let timestamp = chrono::Local::now().format("%H%M%S");
    let filename = format!("capture_{}.png", timestamp);
    let debug_path = debug_dir.join(filename);
    let _ = std::fs::write(&debug_path, &png);

    Ok((BASE64.encode(&png), debug_path))
}

fn process_png_bytes(png: &[u8]) -> Result<(String, PathBuf), String> {
    let img = image::load_from_memory(png).map_err(|e| e.to_string())?;
    process_rgba_dynamic(img)
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

    // Package sets: grim (Wayland), gnome-screenshot (GNOME Wayland/X), scrot (X11 fallback).
    let apt_pkgs = if wayland {
        "grim gnome-screenshot scrot"
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
        let pkgs = if wayland {
            "grim gnome-screenshot scrot"
        } else {
            "scrot maim ImageMagick grim"
        };
        let script = format!("dnf install -y {}", pkgs);
        Command::new("pkexec").args(["sh", "-c", &script]).status()
    } else if has_command("pacman") {
        let pkgs = if wayland {
            "grim gnome-screenshot scrot"
        } else {
            "scrot maim imagemagick grim"
        };
        let script = format!("pacman -S --needed --noconfirm {}", pkgs);
        Command::new("pkexec").args(["sh", "-c", &script]).status()
    } else if has_command("zypper") {
        let pkgs = if wayland {
            "grim gnome-screenshot scrot"
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

/// Called from Start: ensures grim/scrot/… are installed once per app session (Linux only).
#[tauri::command]
pub fn ensure_linux_capture_dependencies() -> Result<serde_json::Value, String> {
    #[cfg(target_os = "linux")]
    {
        if linux_any_capture_tool_in_path() {
            return Ok(serde_json::json!({
                "status": "ok",
                "message": "Screen capture tools already available"
            }));
        }

        if LINUX_AUTOINSTALL_ATTEMPTED.swap(true, Ordering::SeqCst) {
            return Ok(serde_json::json!({
                "status": "already_attempted",
                "message": "Automatic install was already tried this session. Install manually: sudo apt install grim gnome-screenshot scrot"
            }));
        }

        if !has_command("pkexec") {
            return Ok(serde_json::json!({
                "status": "install_failed",
                "message": "pkexec not found. Install manually: sudo apt install grim gnome-screenshot scrot"
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
fn linux_is_gnome_session() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_lowercase()
        .contains("gnome")
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

/// GNOME Shell full-screen capture via D-Bus (`flash=false`). Prefer this on GNOME before `grim`,
/// which can return a valid but black buffer on some multi-output / Mutter setups.
///
/// Mutter often writes to the path returned in the D-Bus reply (`filename_used`), not the path we pass in.
#[cfg(target_os = "linux")]
fn linux_try_gnome_shell_dbus() -> Option<(String, PathBuf)> {
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
        let bytes = std::fs::read(&path).ok()?;
        if bytes.len() < 500 {
            continue;
        }
        let done = process_png_bytes(&bytes).ok()?;
        if path == tmp {
            let _ = std::fs::remove_file(&tmp);
        }
        return Some(done);
    }

    let _ = std::fs::remove_file(&tmp);
    None
}

#[cfg(target_os = "linux")]
fn capture_linux() -> Result<(String, PathBuf), String> {
    let gnome = linux_is_gnome_session();

    if gnome {
        if let Some(ok) = linux_try_gnome_shell_dbus() {
            return Ok(ok);
        }
    }

    // 1a) grim — PNG on stdout (wlroots / many Wayland compositors)
    if let Ok(out) = Command::new("grim").arg("-").output() {
        if out.status.success() && out.stdout.len() > 500 {
            return process_png_bytes(&out.stdout);
        }
    }

    // 1b) grim — write to file (some setups fail `grim -` but succeed with a path)
    let tmp = tmp_png_name("flowsight_grim");
    let _ = std::fs::remove_file(&tmp);
    if let Some(tmp_s) = tmp.to_str() {
        if let Ok(out) = Command::new("grim").arg(tmp_s).output() {
            if out.status.success() {
                if let Some(bytes) = try_read_tmp_png(&tmp) {
                    let _ = std::fs::remove_file(&tmp);
                    return process_png_bytes(&bytes);
                }
            }
        }
    }
    let _ = std::fs::remove_file(&tmp);

    if !gnome {
        if let Some(ok) = linux_try_gnome_shell_dbus() {
            return Ok(ok);
        }
    }

    // 3) scrot — X11, typically no full-screen flash
    let tmp = tmp_png_name("flowsight_scrot");
    let tmp_s = tmp.to_str().ok_or("Invalid temp path")?;
    if Command::new("scrot")
        .arg(tmp_s)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        if let Some(bytes) = try_read_tmp_png(&tmp) {
            let _ = std::fs::remove_file(&tmp);
            return process_png_bytes(&bytes);
        }
    }
    let _ = std::fs::remove_file(&tmp);

    // 4) maim — X11 (common on i3/Awesome)
    let tmp = tmp_png_name("flowsight_maim");
    let tmp_s = tmp.to_str().ok_or("Invalid temp path")?;
    if Command::new("maim")
        .arg(tmp_s)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        if let Some(bytes) = try_read_tmp_png(&tmp) {
            let _ = std::fs::remove_file(&tmp);
            return process_png_bytes(&bytes);
        }
    }
    let _ = std::fs::remove_file(&tmp);

    // 5) ImageMagick 6 — `import`
    let tmp = tmp_png_name("flowsight_import");
    let tmp_s = tmp.to_str().ok_or("Invalid temp path")?;
    if Command::new("import")
        .args(["-window", "root", tmp_s])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        if let Some(bytes) = try_read_tmp_png(&tmp) {
            let _ = std::fs::remove_file(&tmp);
            return process_png_bytes(&bytes);
        }
    }
    let _ = std::fs::remove_file(&tmp);

    // 6) ImageMagick 7 — `magick import`
    let tmp = tmp_png_name("flowsight_magick");
    let tmp_s = tmp.to_str().ok_or("Invalid temp path")?;
    if Command::new("magick")
        .args(["import", "-window", "root", tmp_s])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        if let Some(bytes) = try_read_tmp_png(&tmp) {
            let _ = std::fs::remove_file(&tmp);
            return process_png_bytes(&bytes);
        }
    }
    let _ = std::fs::remove_file(&tmp);

    // 7) KDE Plasma — batch / non-interactive
    let tmp = tmp_png_name("flowsight_kde");
    let tmp_s = tmp.to_str().ok_or("Invalid temp path")?;
    if Command::new("spectacle")
        .args(["-b", "-o", tmp_s])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        if let Some(bytes) = try_read_tmp_png(&tmp) {
            let _ = std::fs::remove_file(&tmp);
            return process_png_bytes(&bytes);
        }
    }
    let _ = std::fs::remove_file(&tmp);

    // 8) Xfce
    let tmp = tmp_png_name("flowsight_xfce");
    let tmp_s = tmp.to_str().ok_or("Invalid temp path")?;
    if Command::new("xfce4-screenshooter")
        .args(["-f", tmp_s])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        if let Some(bytes) = try_read_tmp_png(&tmp) {
            let _ = std::fs::remove_file(&tmp);
            return process_png_bytes(&bytes);
        }
    }
    let _ = std::fs::remove_file(&tmp);

    // 9) gnome-screenshot CLI — often shows a white flash; kept as fallback only
    let tmp = tmp_png_name("flowsight_gnome_cli");
    let tmp_s = match tmp.to_str() {
        Some(s) => s,
        None => return Err("Invalid temp path".into()),
    };
    if Command::new("gnome-screenshot")
        .args(["-f", tmp_s])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        if let Some(bytes) = try_read_tmp_png(&tmp) {
            let _ = std::fs::remove_file(&tmp);
            return process_png_bytes(&bytes);
        }
    }
    let _ = std::fs::remove_file(&tmp);

    // 10) flameshot — full screen to path (no GUI when path is set)
    let tmp = tmp_png_name("flowsight_flameshot");
    let tmp_s = tmp.to_str().ok_or("Invalid temp path")?;
    if Command::new("flameshot")
        .args(["screen", "-p", tmp_s])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        if let Some(bytes) = try_read_tmp_png(&tmp) {
            let _ = std::fs::remove_file(&tmp);
            return process_png_bytes(&bytes);
        }
    }
    let _ = std::fs::remove_file(&tmp);

    let wayland = std::env::var("WAYLAND_DISPLAY").is_ok();
    let hint = if wayland {
        "On Wayland install `grim` (wlroots/sway/Hyprland) or `gnome-screenshot` (GNOME). Example: sudo apt install grim"
    } else {
        "On X11 install `scrot`, `maim`, or `imagemagick` (import/magick). Example: sudo apt install scrot"
    };

    Err(format!(
        "Could not capture the screen ({hint}). On GNOME, D-Bus is tried first, then grim; otherwise grim then D-Bus; then scrot, maim, import, magick, spectacle, xfce, gnome-screenshot, flameshot."
    ))
}
