//! Download and extract official llama.cpp `llama-server` when missing (Linux/macOS dev).

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use flate2::read::GzDecoder;
use tauri::AppHandle;
use tauri::Emitter;

// Qwen3-VL requires a recent llama.cpp runtime. Pin it, rather than allowing
// an older cached runtime or a future incompatible latest release.
const LLAMA_RELEASE_TAG: &str = "b10666";
static RUNTIME_INSTALL_LOCK: Mutex<()> = Mutex::new(());

pub fn exe_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// Walk `bin_root` and one subdirectory level for `llama-server[.exe]` (Linux/macOS tar uses `llama-bXXXX/`).
pub fn find_llama_executable(bin_root: &Path) -> Option<PathBuf> {
    let name = exe_name();
    let direct = bin_root.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    let rd = std::fs::read_dir(bin_root).ok()?;
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            let cand = p.join(name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

fn find_dev_repo_bin() -> Option<PathBuf> {
    let check = |dir: &Path| dir.join("local_llm").join("bin").exists();
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent()?.to_path_buf();
        for _ in 0..8 {
            if check(&dir) {
                let b = dir.join("local_llm").join("bin");
                if let Some(p) = find_llama_executable(&b) {
                    return Some(p);
                }
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
                let b = dir.join("local_llm").join("bin");
                if let Some(p) = find_llama_executable(&b) {
                    return Some(p);
                }
            }
            if !dir.pop() {
                break;
            }
        }
    }
    None
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .map_err(|e| e.to_string())?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).map_err(|e| e.to_string())
}

#[cfg(unix)]
fn is_elf_executable(path: &Path) -> bool {
    let Ok(mut f) = File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 4];
    if f.read_exact(&mut magic).is_err() {
        return false;
    }
    magic == [0x7f, b'E', b'L', b'F']
}

#[cfg(windows)]
fn validate_binary(path: &Path) -> Result<(), String> {
    if path.is_file() {
        return Ok(());
    }
    Err(format!("llama-server not found at {:?}", path))
}

#[cfg(unix)]
fn validate_binary(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("llama-server not found at {:?}", path));
    }
    if is_elf_executable(path) {
        return Ok(());
    }
    Err(format!(
        "{:?} is not a Linux executable (found Windows build or wrong file). \
         Delete it and restart FlowSight to download the correct llama-server.",
        path
    ))
}

fn release_urls(tag: &str, cpu_only: bool) -> Vec<(&'static str, String)> {
    let base = format!("https://github.com/ggml-org/llama.cpp/releases/download/{tag}");

    #[cfg(windows)]
    {
        let _ = cpu_only;
        vec![
            ("zip", format!("{base}/llama-{tag}-bin-win-vulkan-x64.zip")),
            ("zip", format!("{base}/llama-{tag}-bin-win-cpu-x64.zip")),
        ]
    }

    #[cfg(all(unix, target_os = "linux"))]
    {
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            _ => return vec![],
        };
        let cpu = (
            "tgz",
            format!("{base}/llama-{tag}-bin-ubuntu-{arch}.tar.gz"),
        );
        if cpu_only {
            vec![cpu]
        } else {
            vec![(
                "tgz",
                format!("{base}/llama-{tag}-bin-ubuntu-vulkan-{arch}.tar.gz"),
            )]
        }
    }

    #[cfg(target_os = "macos")]
    {
        let _ = cpu_only;
        if std::env::consts::ARCH == "aarch64" {
            vec![("tgz", format!("{base}/llama-{tag}-bin-macos-arm64.tar.gz"))]
        } else {
            vec![("tgz", format!("{base}/llama-{tag}-bin-macos-x64.tar.gz"))]
        }
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = (tag, cpu_only, base);
        vec![]
    }
}

fn download_bytes(url: &str, dest: &Path, app: &AppHandle, label: &str) -> Result<(), String> {
    let parent = dest.parent().ok_or("invalid download path")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;

    let client = reqwest::blocking::Client::builder()
        .user_agent("FlowSight-Agent/1.0")
        .timeout(Duration::from_secs(7200))
        .connect_timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;

    let mut resp = client
        .get(url)
        .send()
        .map_err(|e| format!("GET {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} for {}", resp.status(), url));
    }

    let total = resp.content_length().unwrap_or(0);
    let mut f = File::create(dest).map_err(|e| e.to_string())?;
    let mut buf = [0u8; 65536];
    let mut downloaded: u64 = 0;
    let mut last_emit = Instant::now() - Duration::from_millis(300);

    loop {
        let n = resp.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        downloaded += n as u64;
        let pct: u8 = if total > 0 {
            (((downloaded.min(total)) * 100) / total).min(100) as u8
        } else {
            0
        };
        if last_emit.elapsed() >= Duration::from_millis(300) || (total > 0 && downloaded >= total) {
            last_emit = Instant::now();
            let _ = app.emit(
                "local-ai-progress",
                serde_json::json!({
                    "phase": "llama-bin",
                    "message": format!("{} — {:.1} MB", label, downloaded as f64 / 1_048_576.0),
                    "percent": pct,
                    "downloaded": downloaded,
                    "total": total,
                }),
            );
        }
    }
    f.sync_all().map_err(|e| e.to_string())?;
    Ok(())
}

fn extract_tgz(archive_path: &Path, dest_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;
    let f = File::open(archive_path).map_err(|e| e.to_string())?;
    let dec = GzDecoder::new(f);
    let mut archive = tar::Archive::new(dec);
    archive
        .unpack(dest_dir)
        .map_err(|e| format!("tar unpack: {e}"))?;
    Ok(())
}

#[cfg(windows)]
fn extract_zip(archive_path: &Path, dest_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;
    let f = File::open(archive_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(f).map_err(|e| format!("zip: {e}"))?;
    archive
        .extract(dest_dir)
        .map_err(|e| format!("zip extract: {e}"))?;
    Ok(())
}

/// Ensures `llama-server` exists: dev checkout, app data `bin/`, or download from GitHub releases.
pub fn ensure_llama_server(
    app: &AppHandle,
    storage_bin: PathBuf,
    cpu_only: bool,
) -> Result<PathBuf, String> {
    if !cpu_only {
        if let Some(p) = find_dev_repo_bin() {
            #[cfg(unix)]
            let _ = make_executable(&p);
            validate_binary(&p)?;
            return Ok(p);
        }
    }

    let _install_guard = RUNTIME_INSTALL_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::fs::create_dir_all(&storage_bin).map_err(|e| e.to_string())?;
    if let Some(p) = find_llama_executable(&storage_bin) {
        #[cfg(unix)]
        let _ = make_executable(&p);
        match validate_binary(&p) {
            Ok(()) => return Ok(p),
            Err(e) => {
                log::warn!("[LocalAI] cached llama runtime invalid; reinstalling: {e}");
                std::fs::remove_dir_all(&storage_bin).map_err(|err| err.to_string())?;
                std::fs::create_dir_all(&storage_bin).map_err(|err| err.to_string())?;
            }
        }
    }

    let _ = app.emit(
        "local-ai-progress",
        serde_json::json!({
            "phase": "llama-bin",
            "message": "Downloading llama.cpp runtime (first run only)...",
            "percent": 2u8,
        }),
    );

    let candidates = release_urls(LLAMA_RELEASE_TAG, cpu_only);
    if candidates.is_empty() {
        return Err("Unsupported OS for automatic llama-server download.".to_string());
    }

    let tmp_dir = storage_bin
        .parent()
        .ok_or("invalid storage")?
        .join("downloads");
    std::fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;

    let mut last_err = String::new();

    for (kind, url) in candidates {
        let tmp = tmp_dir.join(format!(
            "llama-runtime.{}",
            if kind == "zip" { "zip" } else { "tar.gz" }
        ));
        let _ = std::fs::remove_file(&tmp);

        if let Err(e) = download_bytes(&url, &tmp, app, "Downloading llama.cpp runtime") {
            last_err = e;
            continue;
        }

        let _ = app.emit(
            "local-ai-progress",
            serde_json::json!({
                "phase": "extracting",
                "message": "Installing local AI engine…",
            }),
        );
        std::fs::remove_dir_all(&storage_bin).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&storage_bin).map_err(|e| e.to_string())?;
        let extract_ok = match kind {
            "tgz" => extract_tgz(&tmp, &storage_bin),
            #[cfg(windows)]
            "zip" => extract_zip(&tmp, &storage_bin),
            #[cfg(not(windows))]
            "zip" => Err("zip extract not supported on this OS".to_string()),
            _ => Err("unknown archive kind".to_string()),
        };

        let _ = std::fs::remove_file(&tmp);

        if let Err(e) = extract_ok {
            last_err = e;
            continue;
        }

        if let Some(p) = find_llama_executable(&storage_bin) {
            #[cfg(unix)]
            let _ = make_executable(&p);
            match validate_binary(&p) {
                Ok(()) => return Ok(p),
                Err(e) => {
                    last_err = e;
                    continue;
                }
            }
        }
        last_err = "extracted archive but llama-server not found".to_string();
    }

    Err(format!(
        "Could not install llama-server automatically. Last error: {last_err}"
    ))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::release_urls;

    #[test]
    fn ubuntu_cpu_runtime_is_separate_from_vulkan_runtime() {
        let gpu = release_urls("b10666", false);
        let cpu = release_urls("b10666", true);
        assert_eq!(gpu.len(), 1);
        assert_eq!(cpu.len(), 1);
        assert!(gpu[0].1.contains("ubuntu-vulkan-"));
        assert!(cpu[0].1.contains("ubuntu-"));
        assert!(!cpu[0].1.contains("vulkan"));
    }
}
