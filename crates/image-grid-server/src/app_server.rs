use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, broadcast, oneshot};
use tokio::time::timeout;

pub const APP_SERVER_PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppServerCandidateDiagnostic {
    pub source: String,
    pub command: Option<String>,
    pub status: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppServerDiagnosticError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppServerDiagnostics {
    pub status: String,
    pub ready: bool,
    pub selected_command: Option<String>,
    pub selected_source: Option<String>,
    pub candidates: Vec<AppServerCandidateDiagnostic>,
    pub error: Option<AppServerDiagnosticError>,
    pub platform_os: Option<String>,
    pub checked_at: Option<String>,
}

impl Default for AppServerDiagnostics {
    fn default() -> Self {
        Self {
            status: "not-started".to_owned(),
            ready: false,
            selected_command: None,
            selected_source: None,
            candidates: Vec::new(),
            error: None,
            platform_os: None,
            checked_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppServerPreflightResponse {
    pub ok: bool,
    pub app_server_image: bool,
    pub app_server_image_ready: bool,
    pub diagnostics: AppServerDiagnostics,
}

impl AppServerPreflightResponse {
    pub fn from_diagnostics(diagnostics: AppServerDiagnostics) -> Self {
        Self {
            ok: diagnostics.ready,
            app_server_image: diagnostics.ready,
            app_server_image_ready: diagnostics.ready,
            diagnostics,
        }
    }
}

#[derive(Debug, Clone)]
struct CandidateSpec {
    source: String,
    command: Option<PathBuf>,
    absent_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct AppServerLaunchConfig {
    candidates: Vec<CandidateSpec>,
}

impl AppServerLaunchConfig {
    pub(crate) fn from_environment() -> Self {
        let mut candidates = Vec::new();
        candidates.push(environment_candidate("IMAGE_GRID_CODEX_BIN"));
        candidates.push(environment_candidate("CODEX_CLI_PATH"));
        candidates.push(CandidateSpec {
            source: "chatgpt-bundled".to_owned(),
            command: Some(PathBuf::from(
                "/Applications/ChatGPT.app/Contents/Resources/codex",
            )),
            absent_reason: None,
        });

        let mut seen = HashSet::new();
        if let Some(path_value) = env::var_os("PATH") {
            for directory in env::split_paths(&path_value) {
                let command = directory.join("codex");
                if seen.insert(command.clone()) {
                    candidates.push(CandidateSpec {
                        source: "PATH".to_owned(),
                        command: Some(command),
                        absent_reason: None,
                    });
                }
            }
        } else {
            candidates.push(CandidateSpec {
                source: "PATH".to_owned(),
                command: None,
                absent_reason: Some("PATH is not set".to_owned()),
            });
        }

        Self { candidates }
    }

    #[cfg(test)]
    pub(crate) fn single(source: &str, command: PathBuf) -> Self {
        Self {
            candidates: vec![CandidateSpec {
                source: source.to_owned(),
                command: Some(command),
                absent_reason: None,
            }],
        }
    }

    fn select(&self) -> Result<SelectedCommand, SelectionFailure> {
        let mut diagnostics = Vec::with_capacity(self.candidates.len());
        let mut selected = None;

        for candidate in &self.candidates {
            let Some(command) = candidate.command.as_ref() else {
                diagnostics.push(AppServerCandidateDiagnostic {
                    source: candidate.source.clone(),
                    command: None,
                    status: "skipped".to_owned(),
                    reason: candidate.absent_reason.clone(),
                });
                continue;
            };

            let displayed_command = display_path(command);
            let rejection = validate_executable(command);
            if let Some(reason) = rejection {
                diagnostics.push(AppServerCandidateDiagnostic {
                    source: candidate.source.clone(),
                    command: Some(displayed_command),
                    status: if !command.is_absolute() || command.exists() {
                        "rejected".to_owned()
                    } else {
                        "unavailable".to_owned()
                    },
                    reason: Some(reason),
                });
                continue;
            }

            if selected.is_none() {
                diagnostics.push(AppServerCandidateDiagnostic {
                    source: candidate.source.clone(),
                    command: Some(displayed_command),
                    status: "selected".to_owned(),
                    reason: None,
                });
                selected = Some((candidate.source.clone(), command.clone()));
            } else {
                diagnostics.push(AppServerCandidateDiagnostic {
                    source: candidate.source.clone(),
                    command: Some(displayed_command),
                    status: "skipped".to_owned(),
                    reason: Some("a higher-priority executable was selected".to_owned()),
                });
            }
        }

        let Some((source, command)) = selected else {
            return Err(SelectionFailure { diagnostics });
        };
        Ok(SelectedCommand {
            source,
            command,
            diagnostics,
        })
    }
}

fn environment_candidate(name: &str) -> CandidateSpec {
    match env::var_os(name) {
        Some(value) => CandidateSpec {
            source: name.to_owned(),
            command: Some(PathBuf::from(value)),
            absent_reason: None,
        },
        None => CandidateSpec {
            source: name.to_owned(),
            command: None,
            absent_reason: Some(format!("{name} is not set")),
        },
    }
}

#[derive(Debug)]
struct SelectedCommand {
    source: String,
    command: PathBuf,
    diagnostics: Vec<AppServerCandidateDiagnostic>,
}

#[derive(Debug)]
struct SelectionFailure {
    diagnostics: Vec<AppServerCandidateDiagnostic>,
}

fn validate_executable(path: &Path) -> Option<String> {
    if !path.is_absolute() {
        return Some("command path is not absolute".to_owned());
    }
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => return Some(format!("command is unavailable: {error}")),
    };
    if !metadata.is_file() {
        return Some("command is not a regular file".to_owned());
    }
    if !is_executable(&metadata) {
        return Some("command is not executable".to_owned());
    }
    None
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    true
}

#[derive(Debug, Clone)]
pub(crate) struct AppServerClientError {
    pub(crate) code: String,
    pub(crate) message: String,
}

impl AppServerClientError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for AppServerClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

type PendingResponse = oneshot::Sender<Result<Value, AppServerClientError>>;
type PendingMap = Arc<StdMutex<HashMap<String, PendingResponse>>>;

pub(crate) struct AppServerClient {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    pending: PendingMap,
    next_id: AtomicU64,
    notifications: broadcast::Sender<Value>,
}

impl AppServerClient {
    async fn spawn(
        command_path: &Path,
        workspace_dir: &Path,
    ) -> Result<Arc<Self>, AppServerClientError> {
        let mut command = Command::new(command_path);
        command
            .arg("app-server")
            .current_dir(workspace_dir)
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn().map_err(|error| {
            AppServerClientError::new(
                "AppServerSpawnFailed",
                format!("could not start {}: {error}", command_path.display()),
            )
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            AppServerClientError::new(
                "AppServerTransportUnavailable",
                "child stdin was not available",
            )
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AppServerClientError::new(
                "AppServerTransportUnavailable",
                "child stdout was not available",
            )
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            AppServerClientError::new(
                "AppServerTransportUnavailable",
                "child stderr was not available",
            )
        })?;

        let pending = Arc::new(StdMutex::new(HashMap::new()));
        let (notifications, _) = broadcast::channel(256);
        spawn_stdout_reader(stdout, pending.clone(), notifications.clone());
        spawn_stderr_reader(stderr, notifications.clone());

        Ok(Arc::new(Self {
            stdin: Mutex::new(stdin),
            child: Mutex::new(child),
            pending,
            next_id: AtomicU64::new(1),
            notifications,
        }))
    }

    pub(crate) async fn request(
        &self,
        method: &str,
        params: Value,
        request_timeout: Duration,
    ) -> Result<Value, AppServerClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let id_key = id.to_string();
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .expect("App Server pending map poisoned")
            .insert(id_key.clone(), sender);

        let message = json!({
            "id": id,
            "method": method,
            "params": params
        });
        if let Err(error) = self.write_message(&message).await {
            self.pending
                .lock()
                .expect("App Server pending map poisoned")
                .remove(&id_key);
            return Err(error);
        }

        match timeout(request_timeout, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(AppServerClientError::new(
                "AppServerClosed",
                format!("{method} ended before a response arrived"),
            )),
            Err(_) => {
                self.pending
                    .lock()
                    .expect("App Server pending map poisoned")
                    .remove(&id_key);
                Err(AppServerClientError::new(
                    "AppServerRequestTimeout",
                    format!("{method} exceeded {} ms", request_timeout.as_millis()),
                ))
            }
        }
    }

    pub(crate) async fn notify(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), AppServerClientError> {
        let mut message = json!({ "method": method });
        if let Some(params) = params {
            message["params"] = params;
        }
        self.write_message(&message).await
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.notifications.subscribe()
    }

    async fn write_message(&self, message: &Value) -> Result<(), AppServerClientError> {
        let mut bytes = serde_json::to_vec(message).map_err(|error| {
            AppServerClientError::new(
                "AppServerSerializeFailed",
                format!("could not encode App Server request: {error}"),
            )
        })?;
        bytes.push(b'\n');

        let mut stdin = self.stdin.lock().await;
        stdin.write_all(&bytes).await.map_err(|error| {
            AppServerClientError::new(
                "AppServerWriteFailed",
                format!("could not write App Server request: {error}"),
            )
        })?;
        stdin.flush().await.map_err(|error| {
            AppServerClientError::new(
                "AppServerWriteFailed",
                format!("could not flush App Server request: {error}"),
            )
        })
    }

    async fn is_running(&self) -> bool {
        self.child
            .lock()
            .await
            .try_wait()
            .map(|status| status.is_none())
            .unwrap_or(false)
    }

    async fn shutdown(&self) {
        let mut child = self.child.lock().await;
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}

fn spawn_stdout_reader(
    stdout: tokio::process::ChildStdout,
    pending: PendingMap,
    notifications: broadcast::Sender<Value>,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let Ok(message) = serde_json::from_str::<Value>(&line) else {
                        let _ = notifications.send(json!({
                            "method": "server-log",
                            "params": {
                                "stream": "stdout",
                                "message": line
                            }
                        }));
                        continue;
                    };

                    if let Some(id_key) = message.get("id").map(rpc_id_key) {
                        let sender = pending
                            .lock()
                            .expect("App Server pending map poisoned")
                            .remove(&id_key);
                        if let Some(sender) = sender {
                            let result = if let Some(error) = message.get("error") {
                                Err(AppServerClientError::new(
                                    error
                                        .get("code")
                                        .map(Value::to_string)
                                        .unwrap_or_else(|| "AppServerRpcError".to_owned()),
                                    error
                                        .get("message")
                                        .and_then(Value::as_str)
                                        .unwrap_or("App Server request failed")
                                        .to_owned(),
                                ))
                            } else {
                                Ok(message.get("result").cloned().unwrap_or(Value::Null))
                            };
                            let _ = sender.send(result);
                            continue;
                        }
                    }

                    let _ = notifications.send(message);
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = notifications.send(json!({
                        "method": "server-log",
                        "params": {
                            "stream": "stdout",
                            "message": format!("App Server stdout failed: {error}")
                        }
                    }));
                    break;
                }
            }
        }

        let pending_responses = {
            let mut pending = pending.lock().expect("App Server pending map poisoned");
            pending
                .drain()
                .map(|(_, sender)| sender)
                .collect::<Vec<_>>()
        };
        for sender in pending_responses {
            let _ = sender.send(Err(AppServerClientError::new(
                "AppServerClosed",
                "App Server stdout closed",
            )));
        }
    });
}

fn spawn_stderr_reader(
    stderr: tokio::process::ChildStderr,
    notifications: broadcast::Sender<Value>,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = notifications.send(json!({
                "method": "server-log",
                "params": {
                    "stream": "stderr",
                    "message": line
                }
            }));
        }
    });
}

fn rpc_id_key(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

struct BridgeInner {
    diagnostics: AppServerDiagnostics,
    client: Option<Arc<AppServerClient>>,
}

#[derive(Clone)]
pub(crate) struct AppServerBridge {
    workspace_dir: PathBuf,
    launch: AppServerLaunchConfig,
    inner: Arc<Mutex<BridgeInner>>,
    preflight_gate: Arc<Mutex<()>>,
}

impl AppServerBridge {
    pub(crate) fn new(workspace_dir: PathBuf, launch: AppServerLaunchConfig) -> Self {
        Self {
            workspace_dir,
            launch,
            inner: Arc::new(Mutex::new(BridgeInner {
                diagnostics: AppServerDiagnostics::default(),
                client: None,
            })),
            preflight_gate: Arc::new(Mutex::new(())),
        }
    }

    pub(crate) async fn diagnostics(&self) -> AppServerDiagnostics {
        self.inner.lock().await.diagnostics.clone()
    }

    pub(crate) async fn ensure_ready(&self) -> AppServerDiagnostics {
        let _guard = self.preflight_gate.lock().await;
        let current_client = { self.inner.lock().await.client.clone() };
        if let Some(client) = current_client
            && client.is_running().await
        {
            return self.inner.lock().await.diagnostics.clone();
        }

        let selected = match self.launch.select() {
            Ok(selected) => selected,
            Err(failure) => {
                let diagnostics = AppServerDiagnostics {
                    status: "error".to_owned(),
                    ready: false,
                    selected_command: None,
                    selected_source: None,
                    candidates: failure.diagnostics,
                    error: Some(AppServerDiagnosticError {
                        code: "AppServerUnavailable".to_owned(),
                        message: "no executable Codex App Server candidate is available".to_owned(),
                    }),
                    platform_os: None,
                    checked_at: Some(now_string()),
                };
                self.set_failure(diagnostics.clone()).await;
                return diagnostics;
            }
        };

        let selected_command = display_path(&selected.command);
        let mut diagnostics = AppServerDiagnostics {
            status: "selected".to_owned(),
            ready: false,
            selected_command: Some(selected_command.clone()),
            selected_source: Some(selected.source.clone()),
            candidates: selected.diagnostics,
            error: None,
            platform_os: None,
            checked_at: Some(now_string()),
        };
        self.inner.lock().await.diagnostics = diagnostics.clone();

        let client = match AppServerClient::spawn(&selected.command, &self.workspace_dir).await {
            Ok(client) => client,
            Err(error) => {
                diagnostics.status = "error".to_owned();
                diagnostics.error = Some(AppServerDiagnosticError {
                    code: error.code,
                    message: error.message,
                });
                diagnostics.checked_at = Some(now_string());
                self.set_failure(diagnostics.clone()).await;
                return diagnostics;
            }
        };

        let initialize = client
            .request(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "codex_image_grid",
                        "title": "Codex Image Grid",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
                APP_SERVER_PREFLIGHT_TIMEOUT,
            )
            .await;
        let initialize = match initialize {
            Ok(result) => result,
            Err(error) => {
                client.shutdown().await;
                diagnostics.status = "error".to_owned();
                diagnostics.error = Some(AppServerDiagnosticError {
                    code: error.code,
                    message: error.message,
                });
                diagnostics.checked_at = Some(now_string());
                self.set_failure(diagnostics.clone()).await;
                return diagnostics;
            }
        };

        if let Err(error) = client.notify("initialized", Some(json!({}))).await {
            client.shutdown().await;
            diagnostics.status = "error".to_owned();
            diagnostics.error = Some(AppServerDiagnosticError {
                code: error.code,
                message: error.message,
            });
            diagnostics.checked_at = Some(now_string());
            self.set_failure(diagnostics.clone()).await;
            return diagnostics;
        }

        diagnostics.status = "ready".to_owned();
        diagnostics.ready = true;
        diagnostics.platform_os = initialize
            .get("platformOs")
            .and_then(Value::as_str)
            .map(str::to_owned);
        diagnostics.error = None;
        diagnostics.checked_at = Some(now_string());
        let mut inner = self.inner.lock().await;
        inner.diagnostics = diagnostics.clone();
        inner.client = Some(client);
        diagnostics
    }

    pub(crate) async fn ready_client(&self) -> Result<Arc<AppServerClient>, AppServerDiagnostics> {
        let diagnostics = self.ensure_ready().await;
        if !diagnostics.ready {
            return Err(diagnostics);
        }
        self.inner.lock().await.client.clone().ok_or(diagnostics)
    }

    pub(crate) async fn current_client(&self) -> Option<Arc<AppServerClient>> {
        self.inner.lock().await.client.clone()
    }

    async fn set_failure(&self, diagnostics: AppServerDiagnostics) {
        let mut inner = self.inner.lock().await;
        inner.diagnostics = diagnostics;
        inner.client = None;
    }
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn now_string() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| OffsetDateTime::now_utc().unix_timestamp().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn preflight_registers_the_request_before_a_synchronous_response() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let workspace = temporary.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let fake = temporary.path().join("fake-codex");
        let mut file = fs::File::create(&fake).expect("fake executable");
        file.write_all(
            br#"#!/bin/sh
test "$1" = "app-server" || exit 2
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":1,"result":{"userAgent":"fixture","codexHome":"/tmp/fixture","platformFamily":"unix","platformOs":"macos"}}'
      ;;
    *'"method":"initialized"'*)
      ;;
  esac
done
"#,
        )
        .expect("fake source");
        file.flush().expect("fake source flushed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = file.metadata().expect("fake metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&fake, permissions).expect("fake executable permissions");
        }

        let bridge = AppServerBridge::new(
            workspace,
            AppServerLaunchConfig::single("fixture", fake.clone()),
        );
        let diagnostics = bridge.ensure_ready().await;

        assert!(diagnostics.ready);
        assert_eq!(diagnostics.status, "ready");
        assert_eq!(
            diagnostics.selected_command.as_deref(),
            Some(display_path(&fake).as_str())
        );
        assert_eq!(diagnostics.selected_source.as_deref(), Some("fixture"));
        assert_eq!(diagnostics.platform_os.as_deref(), Some("macos"));
        assert_eq!(diagnostics.candidates.len(), 1);
        assert_eq!(diagnostics.candidates[0].status, "selected");
        assert!(diagnostics.error.is_none());
    }

    #[test]
    fn selection_records_rejected_and_selected_candidates_in_priority_order() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let relative = PathBuf::from("relative-codex");
        let executable = temporary.path().join("codex");
        fs::write(&executable, "#!/bin/sh\n").expect("fixture executable");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&executable)
                .expect("fixture metadata")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&executable, permissions).expect("fixture permissions");
        }

        let config = AppServerLaunchConfig {
            candidates: vec![
                CandidateSpec {
                    source: "bad".to_owned(),
                    command: Some(relative),
                    absent_reason: None,
                },
                CandidateSpec {
                    source: "good".to_owned(),
                    command: Some(executable.clone()),
                    absent_reason: None,
                },
            ],
        };
        let selected = config.select().expect("selected command");

        assert_eq!(selected.command, executable);
        assert_eq!(selected.source, "good");
        assert_eq!(selected.diagnostics[0].status, "rejected");
        assert_eq!(selected.diagnostics[1].status, "selected");
    }
}
