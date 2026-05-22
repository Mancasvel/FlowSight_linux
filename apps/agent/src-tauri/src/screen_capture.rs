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

/// `gnome-screenshot -f` — reliable on GNOME Wayland (D-Bus Shell API is often AccessDenied).
#[cfg(target_os = "linux")]
fn linux_raw_gnome_screenshot_cli() -> Option<Vec<u8>> {
    if !tool_in_path("gnome-screenshot") {
        return None;
    }
    let tmp = tmp_png_name("flowsight_gnome_cli");
    let tmp_s = tmp.to_str()?;
    let ok = Command::new("gnome-screenshot")
        .args(["-f", tmp_s])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    let bytes = try_read_tmp_png(&tmp)?;
    let _ = std::fs::remove_file(&tmp);
    Some(bytes)
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

    // Order matters: on GNOME+Wayland, `scrot` returns a valid but all-black PNG (~8 KB)
    // and must not run before `gnome-screenshot`.
    let mut pipeline: Vec<(&'static str, fn() -> Option<Vec<u8>>)> = Vec::new();

    if gnome {
        pipeline.push(("gnome-screenshot", linux_raw_gnome_screenshot_cli));
        pipeline.push(("gnome-shell-dbus", linux_raw_gnome_shell_dbus));
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
        "On GNOME Wayland use `gnome-screenshot` (sudo apt install gnome-screenshot). Grant Screen Recording if prompted."
    } else if wayland {
        "On wlroots/Hyprland/Sway install `grim`. On GNOME use `gnome-screenshot`."
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
