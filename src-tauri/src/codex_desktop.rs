//! CDP (Chrome DevTools Protocol) transport layer for Codex Desktop.
//!
//! Codex Desktop is an Electron application whose Windows shell is
//! `ChatGPT.exe` (or the legacy `Codex.exe`) inside the `OpenAI.Codex` MSIX
//! package.  The thinking strength / reasoning selector lives in that Electron
//! renderer, so CDP must attach to the *Desktop shell*, never to the native
//! `codex.exe` CLI / app-server under `AppData/Local/OpenAI/Codex/bin`.
//!
//! This module:
//! - detects an already-running Desktop shell that exposes a CDP endpoint;
//! - locates the installed Desktop executable (MSIX/Appx, WindowsApps,
//!   registry, remembered path);
//! - relaunches the Desktop shell with `--remote-debugging-port` when it is
//!   fully quit;
//! - lists Codex page targets and installs persistent scripts through a small
//!   request/response CDP session.

use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

pub const DEFAULT_CODEX_DEBUG_PORT: u16 = 9229;
pub const CDP_HTTP_TIMEOUT: Duration = Duration::from_secs(2);
const CDP_CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const CDP_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const REMEMBERED_CODEX_DESKTOP_EXECUTABLE_FILENAME: &str = "codex-desktop-executable.json";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A page target returned by Chromium's `/json` endpoint.
#[derive(Clone, Debug, Deserialize)]
pub struct CdpTarget {
    #[allow(dead_code)]
    pub id: String,
    #[serde(rename = "type")]
    pub target_type: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default, rename = "webSocketDebuggerUrl")]
    pub web_socket_debugger_url: Option<String>,
}

/// Serializable status for the frontend.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexCdpStatus {
    /// `cdp_ready` | `running_without_cdp` | `not_running` | `not_found`
    pub state: &'static str,
    pub message: String,
    pub port: Option<u16>,
    pub executable: Option<String>,
}

/// An established CDP session against the Codex Desktop renderer.
pub struct CodexDesktop {
    pub port: u16,
}

impl CodexDesktop {
    /// Ensure a Codex Desktop renderer is reachable over CDP.  If Codex is not
    /// running at all it is relaunched with the remote-debugging switches; if
    /// it is running without CDP the caller gets an actionable error instead of
    /// killing an active session.
    pub async fn new() -> Result<Self, String> {
        let port = ensure_codex_running_with_cdp().await?;
        Ok(Self { port })
    }

    /// Page targets that clearly belong to the Codex Desktop renderer.
    pub async fn codex_page_targets(&self) -> Result<Vec<CdpTarget>, String> {
        let targets = list_cdp_targets(self.port).await?;
        Ok(pick_codex_page_targets(&targets))
    }

    /// Register `source` for every new document and immediately evaluate it in
    /// the current context. Returns per-target evaluation responses.
    pub async fn install_script(&self, source: &str) -> Result<Vec<Value>, String> {
        let targets = self.codex_page_targets().await?;
        let mut responses = Vec::new();
        let mut errors = Vec::new();
        for target in targets {
            let Some(ws_url) = target.web_socket_debugger_url.as_deref() else {
                continue;
            };
            match install_script_on_target(ws_url, source).await {
                Ok(response) => responses.push(response),
                Err(error) => errors.push(error),
            }
        }
        if responses.is_empty() && !errors.is_empty() {
            return Err(errors.join("; "));
        }
        Ok(responses)
    }
}

// ---------------------------------------------------------------------------
// HTTP plumbing
// ---------------------------------------------------------------------------

async fn cdp_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(CDP_HTTP_TIMEOUT)
        .build()
        .map_err(|error| format!("failed to build CDP HTTP client: {error}"))
}

async fn http_get(url: &str) -> Option<String> {
    let client = cdp_http_client().await.ok()?;
    let response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.text().await.ok()
}

/// Chromium can bind the debug listener to either loopback family.
fn cdp_base_urls(port: u16) -> [String; 2] {
    [
        format!("http://127.0.0.1:{port}"),
        format!("http://[::1]:{port}"),
    ]
}

pub async fn cdp_version_ok(port: u16) -> bool {
    for base in cdp_base_urls(port) {
        if http_get(&format!("{base}/json/version")).await.is_some() {
            return true;
        }
    }
    false
}

pub async fn list_cdp_targets(port: u16) -> Result<Vec<CdpTarget>, String> {
    let mut errors = Vec::new();
    for base in cdp_base_urls(port) {
        let client = match cdp_http_client().await {
            Ok(client) => client,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        match client.get(format!("{base}/json")).send().await {
            Ok(response) => match response.error_for_status() {
                Ok(response) => match response.json::<Vec<CdpTarget>>().await {
                    Ok(targets) => return Ok(targets),
                    Err(error) => errors.push(format!("{base}/json: invalid JSON: {error}")),
                },
                Err(error) => errors.push(format!("{base}/json: {error}")),
            },
            Err(error) => errors.push(format!("{base}/json: {error}")),
        }
    }
    Err(errors.join("; "))
}

pub fn candidate_debug_ports(preferred: u16) -> Vec<u16> {
    let mut ports = vec![preferred, DEFAULT_CODEX_DEBUG_PORT, 9222, 9223, 9230, 9231];
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// Only accept a port whose `/json` contains a real Codex Desktop page.
pub async fn find_existing_codex_cdp_port() -> Option<u16> {
    for port in candidate_debug_ports(DEFAULT_CODEX_DEBUG_PORT) {
        if !cdp_version_ok(port).await {
            continue;
        }
        if let Ok(targets) = list_cdp_targets(port).await {
            if !pick_codex_page_targets(&targets).is_empty() {
                return Some(port);
            }
        }
    }
    None
}

fn target_matches_codex_desktop(target: &CdpTarget) -> bool {
    let haystack = format!("{} {}", target.title, target.url).to_ascii_lowercase();
    haystack.contains("codex") || haystack.contains("app://") || haystack.contains("chatgpt")
}

fn pick_codex_page_targets(targets: &[CdpTarget]) -> Vec<CdpTarget> {
    let mut picked = Vec::new();
    for target in targets {
        if target.target_type != "page"
            || target
                .web_socket_debugger_url
                .as_deref()
                .is_none_or(|url| url.is_empty())
        {
            continue;
        }
        if target_matches_codex_desktop(target) {
            picked.push(target.clone());
        }
    }
    picked
}

// ---------------------------------------------------------------------------
// CDP WebSocket session
// ---------------------------------------------------------------------------

/// Lightweight CDP WebSocket session handling request/response pairs.
type CdpWebSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct CdpSession {
    socket: CdpWebSocket,
    responses: std::collections::HashMap<u64, Value>,
}

impl CdpSession {
    async fn connect(url: &str) -> Result<Self, String> {
        let (socket, _) = tokio::time::timeout(CDP_CONNECT_TIMEOUT, connect_async(url))
            .await
            .map_err(|_| format!("timed out connecting CDP WebSocket at {url}"))?
            .map_err(|error| format!("failed to connect CDP WebSocket at {url}: {error}"))?;
        Ok(Self {
            socket,
            responses: std::collections::HashMap::new(),
        })
    }

    async fn send_command(
        &mut self,
        id: u64,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        self.socket
            .send(Message::Text(
                json!({ "id": id, "method": method, "params": params }).to_string(),
            ))
            .await
            .map_err(|error| format!("failed to send CDP command {method}: {error:?}"))?;
        tokio::time::timeout(CDP_COMMAND_TIMEOUT, self.wait_for_id(id, method))
            .await
            .map_err(|_| format!("timed out waiting for CDP command {method}"))?
    }

    async fn wait_for_id(&mut self, id: u64, method: &str) -> Result<Value, String> {
        loop {
            if let Some(response) = self.responses.remove(&id) {
                return cdp_command_result(response, method);
            }
            let Some(message) = self.socket.next().await else {
                return Err(format!("CDP WebSocket closed before {method} response"));
            };
            let message =
                message.map_err(|error| format!("failed to read CDP message: {error:?}"))?;
            let Message::Text(text) = message else {
                continue;
            };
            let value: Value = serde_json::from_str(&text)
                .map_err(|error| format!("failed to parse CDP message: {error}"))?;
            if let Some(response_id) = value.get("id").and_then(Value::as_u64) {
                if response_id == id {
                    return cdp_command_result(value, method);
                }
                self.responses.insert(response_id, value);
            }
        }
    }
}

fn cdp_command_result(response: Value, method: &str) -> Result<Value, String> {
    if let Some(error) = response.get("error") {
        Err(format!("CDP command {method} failed: {error}"))
    } else {
        Ok(response)
    }
}

async fn install_script_on_target(ws_url: &str, source: &str) -> Result<Value, String> {
    let mut session = CdpSession::connect(ws_url).await?;
    session.send_command(1, "Runtime.enable", json!({})).await?;
    session.send_command(2, "Page.enable", json!({})).await?;
    session
        .send_command(
            3,
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": source }),
        )
        .await?;
    session
        .send_command(
            4,
            "Runtime.evaluate",
            json!({
                "expression": source,
                "returnByValue": true,
                "awaitPromise": true,
                "timeout": 8000,
                "allowUnsafeEvalBlockedByCSP": true
            }),
        )
        .await
}

// ---------------------------------------------------------------------------
// Codex Desktop process discovery / launch
// ---------------------------------------------------------------------------

fn powershell_stdout(script: &str) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let output = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return if text.is_empty() { None } else { Some(text) };
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = script;
        None
    }
}

/// Path of the running Codex Desktop shell (main process only).
#[cfg(target_os = "windows")]
const DETECT_CODEX_MAIN_PROCESS_SCRIPT: &str = r#"
Get-CimInstance Win32_Process -Filter "Name = 'Codex.exe' OR Name = 'ChatGPT.exe'" |
  Where-Object {
    if (-not $_.ExecutablePath -or $_.CommandLine -match ' --type=') { return $false }
    $leaf = Split-Path -Leaf $_.ExecutablePath
    $legacyCodex = $leaf -ceq 'Codex.exe'
    $unifiedCodex = $leaf -ceq 'ChatGPT.exe' -and $_.ExecutablePath -match '\\WindowsApps\\OpenAI\.Codex(?:\.Preview)?_'
    return $legacyCodex -or $unifiedCodex
  } |
  Select-Object -First 1 -ExpandProperty ExecutablePath
"#;

pub fn detect_running_codex_main_process() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let text = powershell_stdout(DETECT_CODEX_MAIN_PROCESS_SCRIPT)?;
        return Some(PathBuf::from(text));
    }
    #[cfg(target_os = "macos")]
    {
        for candidate in macos_codex_common_bundle_candidates() {
            let executable = candidate.join("Contents").join("MacOS");
            let name = candidate.file_stem()?.to_str()?;
            let executable = executable.join(name);
            if executable.exists() {
                let probe = Command::new("pgrep")
                    .arg("-f")
                    .arg(executable.display().to_string())
                    .output()
                    .ok()?;
                if probe.status.success() {
                    return Some(executable);
                }
            }
        }
        return None;
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
fn macos_codex_common_bundle_candidates() -> Vec<PathBuf> {
    let mut candidates = vec![
        PathBuf::from("/Applications/Codex.app"),
        PathBuf::from("/Applications/ChatGPT.app"),
    ];
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join("Applications/Codex.app"));
        candidates.push(home.join("Applications/ChatGPT.app"));
    }
    candidates
}

#[cfg(target_os = "windows")]
const DISCOVER_CODEX_DESKTOP_SCRIPT: &str = r#"
$out = @()
$packages = @()
$packages += Get-AppxPackage -Name OpenAI.Codex -ErrorAction SilentlyContinue
$packages += Get-AppxPackage -Name OpenAI.Codex.Preview -ErrorAction SilentlyContinue
$packages += Get-AppxPackage -Name *Codex* -ErrorAction SilentlyContinue
$packages | Sort-Object PackageFullName -Unique | ForEach-Object {
  if (-not $_.InstallLocation) { return }
  foreach ($relative in @('app\ChatGPT.exe','app\Codex.exe','app\resources\Codex.exe','ChatGPT.exe','Codex.exe')) {
    $file = Join-Path $_.InstallLocation $relative
    if (Test-Path -LiteralPath $file -PathType Leaf) { $out += $file; break }
  }
}
$out | Sort-Object -Unique | ConvertTo-Json -Compress
"#;

#[cfg(target_os = "windows")]
fn discover_windows_codex_desktop() -> Vec<PathBuf> {
    let mut result = Vec::new();
    if let Some(json_text) = powershell_stdout(DISCOVER_CODEX_DESKTOP_SCRIPT) {
        if let Ok(value) = serde_json::from_str::<Value>(&json_text) {
            let values = if let Some(array) = value.as_array() {
                array.clone()
            } else {
                vec![value]
            };
            for value in values {
                if let Some(path) = value.as_str() {
                    let path = PathBuf::from(path);
                    if path.exists() {
                        result.push(path);
                    }
                }
            }
        }
    }
    if result.is_empty() {
        // Fallback: scan WindowsApps directly for the highest OpenAI.Codex version.
        let roots = [
            std::env::var_os("ProgramFiles").map(PathBuf::from),
            std::env::var_os("ProgramW6432").map(PathBuf::from),
            Some(PathBuf::from(r"C:\Program Files\WindowsApps")),
        ]
        .into_iter()
        .flatten()
        .collect::<std::collections::BTreeSet<_>>();
        let mut candidates: Vec<(Vec<u32>, PathBuf)> = Vec::new();
        for root in roots {
            let apps_root = if root.join("WindowsApps").exists() {
                root.join("WindowsApps")
            } else {
                root
            };
            let Ok(entries) = std::fs::read_dir(&apps_root) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if !name.starts_with("OpenAI.Codex") || !path.is_dir() {
                    continue;
                }
                for relative in [
                    "app\\ChatGPT.exe",
                    "app\\Codex.exe",
                    "app\\resources\\Codex.exe",
                ] {
                    let executable = path.join(relative);
                    if executable.exists() {
                        candidates.push((version_tuple_from_package_name(name), executable));
                        break;
                    }
                }
            }
        }
        candidates.sort_by(|left, right| left.0.cmp(&right.0));
        if let Some((_, executable)) = candidates.pop() {
            result.push(executable);
        }
    }
    result
}

#[cfg(target_os = "windows")]
fn version_tuple_from_package_name(package_name: &str) -> Vec<u32> {
    package_name
        .split('_')
        .nth(1)
        .unwrap_or_default()
        .split('.')
        .filter_map(|part| part.parse::<u32>().ok())
        .collect()
}

#[cfg(target_os = "windows")]
fn collect_registry_codex_executable_candidates() -> Vec<PathBuf> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;
    let mut result = Vec::new();
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let root = RegKey::predef(hive);
        for subkey in [
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\Codex.exe",
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths\Codex.exe",
        ] {
            let Ok(key) = root.open_subkey(subkey) else {
                continue;
            };
            let Ok(path) = key.get_value::<String, _>("") else {
                continue;
            };
            let path = PathBuf::from(path);
            if path.exists() {
                result.push(path);
            }
        }
    }
    result
}

fn remembered_codex_desktop_path() -> PathBuf {
    crate::config::get_app_config_dir().join(REMEMBERED_CODEX_DESKTOP_EXECUTABLE_FILENAME)
}

fn read_remembered_codex_desktop() -> Option<PathBuf> {
    let text = std::fs::read_to_string(remembered_codex_desktop_path()).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let path = PathBuf::from(value.get("path")?.as_str()?);
    path.exists().then_some(path)
}

fn remember_codex_desktop(path: &Path) {
    if let Ok(text) = serde_json::to_string(&json!({ "path": path.display().to_string() })) {
        if let Some(parent) = remembered_codex_desktop_path().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(remembered_codex_desktop_path(), text);
    }
}

#[cfg(target_os = "windows")]
fn is_windows_codex_desktop_executable(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if name == "Codex.exe" {
        return true;
    }
    name == "ChatGPT.exe"
        && path
            .to_string_lossy()
            .contains(r"\WindowsApps\OpenAI.Codex")
}

fn resolve_codex_desktop_executable() -> Option<PathBuf> {
    if let Some(running) = detect_running_codex_main_process() {
        remember_codex_desktop(&running);
        return Some(running);
    }
    if let Some(remembered) = read_remembered_codex_desktop() {
        return Some(remembered);
    }
    #[cfg(target_os = "windows")]
    {
        for executable in discover_windows_codex_desktop() {
            if is_windows_codex_desktop_executable(&executable) {
                return Some(executable);
            }
        }
        for executable in collect_registry_codex_executable_candidates() {
            if is_windows_codex_desktop_executable(&executable) {
                return Some(executable);
            }
        }
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        for bundle in macos_codex_common_bundle_candidates() {
            let name = bundle.file_stem()?.to_str()?;
            let executable = bundle.join("Contents/MacOS").join(name);
            if executable.exists() {
                return Some(executable);
            }
        }
        return None;
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        None
    }
}

fn pick_free_debug_port(start: u16) -> u16 {
    (start..start + 200)
        .find(|port| std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok())
        .unwrap_or(start)
}

#[cfg(target_os = "windows")]
fn launch_codex_with_debug_port(executable: &Path, port: u16) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    Command::new(executable)
        .creation_flags(CREATE_NO_WINDOW)
        .arg(format!("--remote-debugging-port={port}"))
        .arg(format!(
            "--remote-allow-origins=http://127.0.0.1:{port},http://localhost:{port}"
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| {
            format!(
                "failed to launch Codex Desktop {}: {error}",
                executable.display()
            )
        })
}

#[cfg(target_os = "macos")]
fn launch_codex_with_debug_port(executable: &Path, port: u16) -> Result<(), String> {
    let mut bundle = executable.to_path_buf();
    while bundle
        .file_name()
        .is_some_and(|name| !name.to_string_lossy().ends_with(".app"))
    {
        if !bundle.pop() {
            break;
        }
    }
    Command::new("open")
        .arg(&bundle)
        .arg("--args")
        .arg(format!("--remote-debugging-port={port}"))
        .arg(format!(
            "--remote-allow-origins=http://127.0.0.1:{port},http://localhost:{port}"
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| {
            format!(
                "failed to launch Codex Desktop {}: {error}",
                executable.display()
            )
        })
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn launch_codex_with_debug_port(executable: &Path, port: u16) -> Result<(), String> {
    Command::new(executable)
        .arg(format!("--remote-debugging-port={port}"))
        .arg(format!(
            "--remote-allow-origins=http://127.0.0.1:{port},http://localhost:{port}"
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| {
            format!(
                "failed to launch Codex Desktop {}: {error}",
                executable.display()
            )
        })
}

async fn wait_until_codex_page_ready(port: u16, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if let Ok(targets) = list_cdp_targets(port).await {
            if !pick_codex_page_targets(&targets).is_empty() {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

/// Open Codex Desktop with CDP when it is not running; never kills a live
/// Desktop session.
async fn ensure_codex_running_with_cdp() -> Result<u16, String> {
    if let Some(port) = find_existing_codex_cdp_port().await {
        return Ok(port);
    }

    if let Some(running) = detect_running_codex_main_process() {
        return Err(format!(
            "Codex Desktop is already running at {} but without CDP. Fully quit Codex Desktop, then click Unlock again so CC Switch can relaunch it with remote debugging.",
            running.display()
        ));
    }

    let executable = resolve_codex_desktop_executable()
        .ok_or_else(|| "Codex Desktop executable not found. Install or start Codex Desktop once so CC Switch can locate it.".to_string())?;
    let port = pick_free_debug_port(DEFAULT_CODEX_DEBUG_PORT);
    launch_codex_with_debug_port(&executable, port)?;
    remember_codex_desktop(&executable);

    if wait_until_codex_page_ready(port, Duration::from_secs(45)).await {
        Ok(port)
    } else {
        Err(format!(
            "Codex Desktop was launched with remote debugging on port {port}, but no renderer target appeared within 45 seconds. If Codex opened without CDP, fully quit it and try again."
        ))
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

pub async fn codex_cdp_status() -> CodexCdpStatus {
    if let Some(port) = find_existing_codex_cdp_port().await {
        return CodexCdpStatus {
            state: "cdp_ready",
            message: format!(
                "Codex Desktop is running with CDP on port {port}; thinking strength is injectable."
            ),
            port: Some(port),
            executable: detect_running_codex_main_process().map(|path| path.display().to_string()),
        };
    }
    if let Some(running) = detect_running_codex_main_process() {
        return CodexCdpStatus {
            state: "running_without_cdp",
            message: format!(
                "Codex Desktop is running at {} but without CDP. Fully quit Codex Desktop, then click Unlock so CC Switch can relaunch it with remote debugging.",
                running.display()
            ),
            port: None,
            executable: Some(running.display().to_string()),
        };
    }
    if let Some(executable) = resolve_codex_desktop_executable() {
        return CodexCdpStatus {
            state: "not_running",
            message: format!(
                "Codex Desktop is not running. Found {}; Unlock will relaunch it with remote debugging and inject the thinking strength patch.",
                executable.display()
            ),
            port: None,
            executable: Some(executable.display().to_string()),
        };
    }
    CodexCdpStatus {
        state: "not_found",
        message: "Codex Desktop executable not found. Install or start Codex Desktop once so CC Switch can locate it.".to_string(),
        port: None,
        executable: None,
    }
}
