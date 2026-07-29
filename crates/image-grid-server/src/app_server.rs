use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, broadcast, oneshot, watch};
use tokio::time::timeout;

pub const APP_SERVER_PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(15);
pub(crate) const RUNTIME_CLOSED_MESSAGE: &str = "Codex Image Grid runtime closed";

#[derive(Debug, Clone)]
pub(crate) struct AppServerEvent {
    pub(crate) name: String,
    pub(crate) data: Value,
}

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

struct AppServerLifecycle {
    public_events: broadcast::Sender<AppServerEvent>,
    state: StdMutex<AppServerLifecycleState>,
}

#[derive(Default)]
struct AppServerLifecycleState {
    active_operations: u64,
    pending_terminal: Option<(String, String)>,
    terminal_recorded: bool,
}

impl AppServerLifecycle {
    fn bind(self: &Arc<Self>) -> AppServerOperation {
        self.state
            .lock()
            .expect("App Server lifecycle state poisoned")
            .active_operations += 1;
        AppServerOperation {
            lifecycle: self.clone(),
        }
    }

    fn defer_terminal(&self, status: &str, message: &str) {
        let emit_now = {
            let mut state = self
                .state
                .lock()
                .expect("App Server lifecycle state poisoned");
            if state.terminal_recorded {
                return;
            }
            state.terminal_recorded = true;
            if state.active_operations == 0 {
                true
            } else {
                state.pending_terminal = Some((status.to_owned(), message.to_owned()));
                false
            }
        };
        if emit_now {
            self.emit_terminal(status, message);
        }
    }

    fn reset_terminal(&self) {
        let mut state = self
            .state
            .lock()
            .expect("App Server lifecycle state poisoned");
        state.terminal_recorded = false;
        state.pending_terminal = None;
    }

    fn operation_finished(&self) {
        let terminal = {
            let mut state = self
                .state
                .lock()
                .expect("App Server lifecycle state poisoned");
            debug_assert!(state.active_operations > 0);
            state.active_operations = state.active_operations.saturating_sub(1);
            if state.active_operations == 0 {
                state.pending_terminal.take()
            } else {
                None
            }
        };
        if let Some((status, message)) = terminal {
            self.emit_terminal(&status, &message);
        }
    }

    fn emit_terminal(&self, status: &str, message: &str) {
        let _ = self.public_events.send(AppServerEvent {
            name: "server-status".to_owned(),
            data: json!({
                "status": status,
                "message": message
            }),
        });
    }
}

pub(crate) struct AppServerOperation {
    lifecycle: Arc<AppServerLifecycle>,
}

impl Drop for AppServerOperation {
    fn drop(&mut self) {
        self.lifecycle.operation_finished();
    }
}

pub(crate) struct AppServerClient {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    pending: PendingMap,
    next_id: AtomicU64,
    notifications: broadcast::Sender<Value>,
    closed: Arc<AtomicBool>,
    lifecycle: Arc<AppServerLifecycle>,
    shutdown_gate: Mutex<()>,
}

impl AppServerClient {
    async fn spawn(
        command_path: &Path,
        workspace_dir: &Path,
        public_events: broadcast::Sender<AppServerEvent>,
        lifecycle: Arc<AppServerLifecycle>,
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
        let closed = Arc::new(AtomicBool::new(false));
        spawn_stdout_reader(
            stdout,
            pending.clone(),
            notifications.clone(),
            public_events.clone(),
            lifecycle.clone(),
            closed.clone(),
        );
        spawn_stderr_reader(stderr, notifications.clone(), public_events);

        Ok(Arc::new(Self {
            stdin: Mutex::new(stdin),
            child: Mutex::new(child),
            pending,
            next_id: AtomicU64::new(1),
            notifications,
            closed,
            lifecycle,
            shutdown_gate: Mutex::new(()),
        }))
    }

    pub(crate) async fn request(
        &self,
        method: &str,
        params: Value,
        request_timeout: Duration,
    ) -> Result<Value, AppServerClientError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(runtime_closed_error());
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let id_key = id.to_string();
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .expect("App Server pending map poisoned")
            .insert(id_key.clone(), sender);
        if self.closed.load(Ordering::Acquire) {
            self.pending
                .lock()
                .expect("App Server pending map poisoned")
                .remove(&id_key);
            return Err(runtime_closed_error());
        }

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
        if self.closed.load(Ordering::Acquire) {
            return Err(runtime_closed_error());
        }
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
        if self.closed.load(Ordering::Acquire) {
            return Err(runtime_closed_error());
        }
        let mut bytes = serde_json::to_vec(message).map_err(|error| {
            AppServerClientError::new(
                "AppServerSerializeFailed",
                format!("could not encode App Server request: {error}"),
            )
        })?;
        bytes.push(b'\n');

        let mut stdin = self.stdin.lock().await;
        if let Err(error) = stdin.write_all(&bytes).await {
            let error = AppServerClientError::new(
                "AppServerWriteFailed",
                format!("could not write App Server request: {error}"),
            );
            self.lifecycle.defer_terminal("error", &error.message);
            return Err(error);
        }
        if let Err(error) = stdin.flush().await {
            let error = AppServerClientError::new(
                "AppServerWriteFailed",
                format!("could not flush App Server request: {error}"),
            );
            self.lifecycle.defer_terminal("error", &error.message);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) async fn is_running(&self) -> bool {
        self.child
            .lock()
            .await
            .try_wait()
            .map(|status| status.is_none())
            .unwrap_or(false)
    }

    async fn shutdown(&self) {
        let _shutdown_guard = self.shutdown_gate.lock().await;
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let pending_responses = {
            let mut pending = self
                .pending
                .lock()
                .expect("App Server pending map poisoned");
            pending
                .drain()
                .map(|(_, sender)| sender)
                .collect::<Vec<_>>()
        };
        for sender in pending_responses {
            let _ = sender.send(Err(runtime_closed_error()));
        }
        let mut child = self.child.lock().await;
        terminate_owned_child(&mut child).await;
    }
}

#[cfg(unix)]
async fn terminate_owned_child(child: &mut Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    if let Some(pid) = child.id() {
        let _ = Command::new("/bin/kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    let _ = child.wait().await;
}

#[cfg(not(unix))]
async fn terminate_owned_child(child: &mut Child) {
    let _ = child.start_kill();
    let _ = child.wait().await;
}

fn runtime_closed_error() -> AppServerClientError {
    AppServerClientError::new("RuntimeClosed", RUNTIME_CLOSED_MESSAGE)
}

fn spawn_stdout_reader(
    stdout: tokio::process::ChildStdout,
    pending: PendingMap,
    notifications: broadcast::Sender<Value>,
    public_events: broadcast::Sender<AppServerEvent>,
    lifecycle: Arc<AppServerLifecycle>,
    closed: Arc<AtomicBool>,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let Ok(message) = serde_json::from_str::<Value>(&line) else {
                        emit_server_log(&public_events, "stdout", line.clone());
                        let _ = notifications.send(json!({
                            "method": "server-log",
                            "params": {
                                "stream": "stdout",
                                "message": line
                            }
                        }));
                        continue;
                    };
                    if message.get("method").and_then(Value::as_str) == Some("server-log") {
                        let params = message.get("params").unwrap_or(&Value::Null);
                        emit_server_log(
                            &public_events,
                            params
                                .get("stream")
                                .and_then(Value::as_str)
                                .unwrap_or("stdout"),
                            params
                                .get("text")
                                .or_else(|| params.get("message"))
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                        );
                    }

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
                    let text = format!("App Server stdout failed: {error}");
                    emit_server_log(&public_events, "stdout", text.clone());
                    let _ = notifications.send(json!({
                        "method": "server-log",
                        "params": {
                            "stream": "stdout",
                            "message": text
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
        if !closed.load(Ordering::Acquire) {
            let message = "Codex App Server stopped";
            let _ = notifications.send(json!({
                "method": "server-status",
                "params": {
                    "status": "stopped",
                    "message": message
                }
            }));
            lifecycle.defer_terminal("stopped", message);
        }
    });
}

fn spawn_stderr_reader(
    stderr: tokio::process::ChildStderr,
    notifications: broadcast::Sender<Value>,
    public_events: broadcast::Sender<AppServerEvent>,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            emit_server_log(&public_events, "stderr", line.clone());
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

fn emit_server_log(public_events: &broadcast::Sender<AppServerEvent>, stream: &str, text: String) {
    let stream = match stream {
        "stderr" => "stderr",
        "artifact" => "artifact",
        _ => "stdout",
    };
    let _ = public_events.send(AppServerEvent {
        name: "server-log".to_owned(),
        data: json!({
            "stream": stream,
            "text": text
        }),
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
    shutdown_gate: Arc<Mutex<()>>,
    closed: Arc<AtomicBool>,
    shutdown_signal: watch::Sender<bool>,
    public_events: broadcast::Sender<AppServerEvent>,
    lifecycle: Arc<AppServerLifecycle>,
}

impl AppServerBridge {
    pub(crate) fn new(workspace_dir: PathBuf, launch: AppServerLaunchConfig) -> Self {
        let (shutdown_signal, _) = watch::channel(false);
        let (public_events, _) = broadcast::channel(256);
        let lifecycle = Arc::new(AppServerLifecycle {
            public_events: public_events.clone(),
            state: StdMutex::new(AppServerLifecycleState::default()),
        });
        Self {
            workspace_dir,
            launch,
            inner: Arc::new(Mutex::new(BridgeInner {
                diagnostics: AppServerDiagnostics::default(),
                client: None,
            })),
            preflight_gate: Arc::new(Mutex::new(())),
            shutdown_gate: Arc::new(Mutex::new(())),
            closed: Arc::new(AtomicBool::new(false)),
            shutdown_signal,
            public_events,
            lifecycle,
        }
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<AppServerEvent> {
        self.public_events.subscribe()
    }

    pub(crate) fn shutdown_receiver(&self) -> watch::Receiver<bool> {
        self.shutdown_signal.subscribe()
    }

    pub(crate) fn bind_operation(&self) -> AppServerOperation {
        self.lifecycle.bind()
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub(crate) async fn diagnostics(&self) -> AppServerDiagnostics {
        self.inner.lock().await.diagnostics.clone()
    }

    pub(crate) async fn ensure_ready(&self) -> AppServerDiagnostics {
        if self.is_closed() {
            return stopped_diagnostics(self.diagnostics().await);
        }
        let _guard = self.preflight_gate.lock().await;
        if self.is_closed() {
            return stopped_diagnostics(self.diagnostics().await);
        }
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
                self.set_failure(diagnostics.clone(), false).await;
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

        self.lifecycle.reset_terminal();
        let client = match AppServerClient::spawn(
            &selected.command,
            &self.workspace_dir,
            self.public_events.clone(),
            self.lifecycle.clone(),
        )
        .await
        {
            Ok(client) => client,
            Err(error) => {
                diagnostics.status = "error".to_owned();
                diagnostics.error = Some(AppServerDiagnosticError {
                    code: error.code,
                    message: error.message,
                });
                diagnostics.checked_at = Some(now_string());
                self.set_failure(diagnostics.clone(), true).await;
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
                self.set_failure(diagnostics.clone(), true).await;
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
            self.set_failure(diagnostics.clone(), true).await;
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
        drop(inner);
        self.emit_ready(&diagnostics);
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

    async fn set_failure(&self, diagnostics: AppServerDiagnostics, emit_event: bool) {
        let message = diagnostics
            .error
            .as_ref()
            .map(|error| error.message.clone())
            .unwrap_or_else(|| "Codex App Server failed".to_owned());
        let mut inner = self.inner.lock().await;
        inner.diagnostics = diagnostics;
        inner.client = None;
        drop(inner);
        if emit_event {
            self.lifecycle.defer_terminal("error", &message);
        }
    }

    pub(crate) async fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        let _guard = self.shutdown_gate.lock().await;
        if *self.shutdown_signal.borrow() {
            return;
        }
        let _preflight = self.preflight_gate.lock().await;
        let client = {
            let mut inner = self.inner.lock().await;
            let client = inner.client.take();
            inner.diagnostics = stopped_diagnostics(inner.diagnostics.clone());
            client
        };
        if let Some(client) = client {
            client.shutdown().await;
        }
        let _ = self.shutdown_signal.send(true);
    }

    fn emit_ready(&self, diagnostics: &AppServerDiagnostics) {
        let _ = self.public_events.send(AppServerEvent {
            name: "server-status".to_owned(),
            data: json!({
                "status": "ready",
                "platform": diagnostics.platform_os.as_deref().unwrap_or_default(),
                "selectedCommand": diagnostics.selected_command.as_deref().unwrap_or_default(),
                "selectedSource": diagnostics.selected_source.as_deref().unwrap_or_default()
            }),
        });
    }
}

fn stopped_diagnostics(mut diagnostics: AppServerDiagnostics) -> AppServerDiagnostics {
    diagnostics.status = "stopped".to_owned();
    diagnostics.ready = false;
    diagnostics.error = Some(AppServerDiagnosticError {
        code: "RuntimeClosed".to_owned(),
        message: RUNTIME_CLOSED_MESSAGE.to_owned(),
    });
    diagnostics.checked_at = Some(now_string());
    diagnostics
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
    use std::sync::Barrier;

    fn fixture_lifecycle() -> (Arc<AppServerLifecycle>, broadcast::Receiver<AppServerEvent>) {
        let (public_events, receiver) = broadcast::channel(16);
        (
            Arc::new(AppServerLifecycle {
                public_events,
                state: StdMutex::new(AppServerLifecycleState::default()),
            }),
            receiver,
        )
    }

    #[test]
    fn shutdown_contract_terminal_defer_and_last_operation_emit_exactly_once() {
        let (lifecycle, mut events) = fixture_lifecycle();
        let operation = lifecycle.bind();
        let barrier = Arc::new(Barrier::new(3));
        let defer_lifecycle = lifecycle.clone();
        let defer_barrier = barrier.clone();
        let defer = std::thread::spawn(move || {
            defer_barrier.wait();
            defer_lifecycle.defer_terminal("stopped", "fixture stopped");
        });
        let finish_barrier = barrier.clone();
        let finish = std::thread::spawn(move || {
            finish_barrier.wait();
            drop(operation);
        });
        barrier.wait();
        defer.join().expect("terminal defer thread");
        finish.join().expect("operation finish thread");

        let event = events.try_recv().expect("one terminal event");
        assert_eq!(event.name, "server-status");
        assert_eq!(
            event.data,
            json!({"status": "stopped", "message": "fixture stopped"})
        );
        assert!(matches!(
            events.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_contract_owned_child_observes_sigterm_and_pending_rpc_closes() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let workspace = temporary.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let marker = temporary.path().join("term-observed");
        let fake = temporary.path().join("fake-codex");
        let mut file = fs::File::create(&fake).expect("fake executable");
        writeln!(
            file,
            "#!/bin/sh\n\
             test \"$1\" = \"app-server\" || exit 2\n\
             marker=\"{}\"\n\
             trap 'printf TERM > \"$marker\"; exit 0' TERM\n\
             printf READY > \"$marker\"\n\
             while :; do :; done",
            display_path(&marker)
        )
        .expect("fake source");
        file.flush().expect("fake source flushed");
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = file.metadata().expect("fake metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&fake, permissions).expect("fake executable permissions");
        }
        let (public_events, _) = broadcast::channel(16);
        let (lifecycle, _) = fixture_lifecycle();
        let client = AppServerClient::spawn(&fake, &workspace, public_events, lifecycle)
            .await
            .expect("owned child");
        timeout(Duration::from_secs(1), async {
            loop {
                if fs::read_to_string(&marker).ok().as_deref() == Some("READY") {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("TERM trap installed");
        let pending_client = client.clone();
        let pending = tokio::spawn(async move {
            pending_client
                .request("fixture/pending", json!({}), Duration::from_secs(10))
                .await
        });
        timeout(Duration::from_secs(1), async {
            loop {
                if !client.pending.lock().expect("pending map").is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("pending RPC registered");

        let left = client.clone();
        let right = client.clone();
        tokio::join!(left.shutdown(), right.shutdown());
        let error = pending
            .await
            .expect("pending task")
            .expect_err("pending RPC rejected");
        assert_eq!(error.code, "RuntimeClosed");
        assert_eq!(error.message, RUNTIME_CLOSED_MESSAGE);
        assert_eq!(
            fs::read_to_string(marker).expect("TERM marker"),
            "TERM",
            "owned child must observe SIGTERM rather than SIGKILL"
        );
        assert!(!client.is_running().await);
    }

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
