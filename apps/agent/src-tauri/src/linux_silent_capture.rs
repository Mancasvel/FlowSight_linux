//! GNOME Wayland: capture via ScreenCast + PipeWire (no shutter flash/sound).
//! Avoids org.gnome.Shell.Screenshot and xdg-desktop-portal Screenshot — both flash on GNOME 49+.

use std::os::fd::{AsRawFd, OwnedFd};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// At most one GStreamer grab per monitoring interval (~60s).
const MIN_FRAME_INTERVAL: Duration = Duration::from_secs(55);

static SESSION_READY: AtomicBool = AtomicBool::new(false);
static LAST_GRAB_MS: AtomicU64 = AtomicU64::new(0);

use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType};
use ashpd::desktop::PersistMode;

struct ScreencastHold {
    node_id: u32,
    fd: OwnedFd,
    _keepalive: JoinHandle<()>,
}

static HOLDER: OnceLock<Mutex<Option<ScreencastHold>>> = OnceLock::new();

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn may_grab_frame() -> bool {
    let last = LAST_GRAB_MS.load(Ordering::Relaxed);
    if last == 0 {
        return true;
    }
    now_ms().saturating_sub(last) >= MIN_FRAME_INTERVAL.as_millis() as u64
}

fn mark_grabbed() {
    LAST_GRAB_MS.store(now_ms(), Ordering::Relaxed);
}

fn shell_escape_single(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// Portal PipeWire fds often have FD_CLOEXEC; gst-launch must inherit them.
fn clear_fd_cloexec(fd: &OwnedFd) {
    unsafe {
        let raw = fd.as_raw_fd();
        let flags = libc::fcntl(raw, libc::F_GETFD);
        if flags >= 0 {
            libc::fcntl(raw, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
        }
    }
}

fn holder_slot() -> &'static Mutex<Option<ScreencastHold>> {
    HOLDER.get_or_init(|| Mutex::new(None))
}

pub fn linux_is_wayland_session() -> bool {
    std::env::var("WAYLAND_DISPLAY").is_ok()
        || std::env::var("XDG_SESSION_TYPE")
            .map(|s| s.eq_ignore_ascii_case("wayland"))
            .unwrap_or(false)
}

/// GNOME always uses ScreenCast (portal Screenshot flashes even when WAYLAND_DISPLAY is unset in the app).
pub fn linux_use_silent_screencast() -> bool {
    crate::screen_capture::linux_is_gnome_session()
}

pub fn linux_auto_grant_screencast_permissions() -> Vec<String> {
    let mut granted = Vec::new();
    for id in crate::screen_capture::linux_collect_portal_app_ids() {
        if grant_screencast_permission(&id) {
            log::info!("[FlowSight] Granted portal screencast permission for '{id}'");
            granted.push(id);
        }
    }
    granted
}

fn grant_screencast_permission(app_id: &str) -> bool {
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
            "screencast",
            "true",
            "screencast",
            app_id,
            "['yes']",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn open_screencast_session() -> Result<(u32, OwnedFd, JoinHandle<()>), String> {
    let proxy = Screencast::new()
        .await
        .map_err(|e| format!("ScreenCast: {e}"))?;
    let session = proxy
        .create_session()
        .await
        .map_err(|e| format!("ScreenCast session: {e}"))?;

    proxy
        .select_sources(
            &session,
            CursorMode::Hidden,
            SourceType::Monitor.into(),
            false,
            None,
            PersistMode::ExplicitlyRevoked,
        )
        .await
        .map_err(|e| format!("ScreenCast select_sources: {e}"))?
        .response()
        .map_err(|e| format!("ScreenCast select_sources response: {e}"))?;

    let streams = proxy
        .start(&session, None)
        .await
        .map_err(|e| format!("ScreenCast start: {e}"))?
        .response()
        .map_err(|e| format!("ScreenCast start response: {e}"))?;

    let stream = streams
        .streams()
        .first()
        .ok_or_else(|| "ScreenCast: no stream".to_string())?;

    let node_id = stream.pipe_wire_node_id();
    let fd = proxy
        .open_pipe_wire_remote(&session)
        .await
        .map_err(|e| format!("ScreenCast OpenPipeWireRemote: {e}"))?;

    let keepalive = std::thread::Builder::new()
        .name("flowsight-screencast".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(_) => return,
            };
            rt.block_on(async {
                while SESSION_READY.load(Ordering::SeqCst) {
                    tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                }
                let _ = (proxy, session);
            });
        })
        .map_err(|e| format!("screencast keepalive thread: {e}"))?;

    Ok((node_id, fd, keepalive))
}

fn grab_frame_with_gstreamer(fd: &OwnedFd, node_id: u32) -> Option<Vec<u8>> {
    if !Command::new("which")
        .arg("gst-launch-1.0")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        log::warn!("[FlowSight] gst-launch-1.0 not found — install gstreamer1.0-tools");
        return None;
    }

    let fd_num = fd.as_raw_fd();
    let tmp = std::env::temp_dir().join(format!(
        "flowsight_screencast_{}_{}.png",
        node_id,
        now_ms()
    ));
    let tmp_s = tmp.to_str()?;

    clear_fd_cloexec(fd);

    // Run exactly like the shell: word-split pipeline + inherited PipeWire fd (see xdg screencast examples).
    let script = format!(
        "exec gst-launch-1.0 -e pipewiresrc fd={fd_num} path={node_id} num-buffers=2 ! videoconvert ! pngenc ! filesink location={}",
        shell_escape_single(tmp_s)
    );

    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(&script)
        .output()
        .ok()?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        log::warn!(
            "[FlowSight] gst-launch screencast failed [v3]: {} | {}",
            err.trim(),
            script
        );
        let _ = std::fs::remove_file(&tmp);
        return None;
    }

    let bytes = std::fs::read(&tmp).ok()?;
    let _ = std::fs::remove_file(&tmp);
    if bytes.len() < 500 {
        log::warn!(
            "[FlowSight] screencast PNG too small ({} bytes)",
            bytes.len()
        );
        return None;
    }
    log::info!("[FlowSight] Linux screen capture succeeded via screencast (PipeWire, silent)");
    Some(bytes)
}

/// One frame at most per ~60s (matches monitoring interval).
pub fn capture_monitoring_frame() -> Option<Vec<u8>> {
    if !SESSION_READY.load(Ordering::SeqCst) {
        return None;
    }
    if !may_grab_frame() {
        return None;
    }
    let guard = holder_slot().lock().ok()?;
    let hold = guard.as_ref()?;
    let bytes = grab_frame_with_gstreamer(&hold.fd, hold.node_id)?;
    mark_grabbed();
    Some(bytes)
}

/// Start a persistent ScreenCast session (call when monitoring starts).
pub async fn ensure_session() -> Result<(), String> {
    if SESSION_READY.load(Ordering::SeqCst) {
        return Ok(());
    }

    crate::screen_capture::linux_auto_grant_screenshot_permissions();
    linux_auto_grant_screencast_permissions();

    let (node_id, fd, keepalive) = open_screencast_session().await?;

    *holder_slot().lock().map_err(|e| e.to_string())? = Some(ScreencastHold {
        node_id,
        fd,
        _keepalive: keepalive,
    });
    SESSION_READY.store(true, Ordering::SeqCst);
    log::info!("[FlowSight] Silent ScreenCast session ready (PipeWire, no flash)");
    Ok(())
}

pub fn stop_session() {
    SESSION_READY.store(false, Ordering::SeqCst);
    LAST_GRAB_MS.store(0, Ordering::Relaxed);
    if let Ok(mut g) = holder_slot().lock() {
        if let Some(h) = g.take() {
            let _ = h._keepalive.join();
        }
    }
}

/// Opens ScreenCast session only (no frame grab; frames happen on the 60s monitoring tick).
#[tauri::command]
pub async fn ensure_screencast_ready() -> Result<(), String> {
    ensure_session().await
}
