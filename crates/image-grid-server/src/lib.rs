mod analysis;
mod app_server;
mod http_json;
mod runtime;

pub use app_server::{
    AppServerCandidateDiagnostic, AppServerDiagnosticError, AppServerDiagnostics,
    AppServerPreflightResponse,
};

use analysis::ReferenceAnalysisRuntime;
use app_server::{AppServerBridge, AppServerLaunchConfig};
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path as AxumPath, Query, Request as AxumRequest, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use image_grid_core::{APP_IDENTITY, MAX_RUN_JOBS};
use runtime::{
    GeneratedJobFileError, GenerationRuntime, content_type, render_artifact_page,
    render_image_page, valid_run_id,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::convert::Infallible;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub server_root: PathBuf,
    pub data_dir: PathBuf,
    pub generated_dir: PathBuf,
    pub run_dir: PathBuf,
    pub workspace_dir: PathBuf,
    pub launch_target: String,
    pub package_root_kind: String,
}

trait HostOpener: Send + Sync {
    fn open(&self, arguments: &[OsString]) -> io::Result<()>;
}

struct SystemHostOpener;

impl HostOpener for SystemHostOpener {
    fn open(&self, arguments: &[OsString]) -> io::Result<()> {
        Command::new("open")
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
    }
}

impl RuntimeConfig {
    pub fn new(
        server_root: PathBuf,
        data_dir: PathBuf,
        workspace_dir: Option<PathBuf>,
        launch_target: String,
    ) -> Self {
        let generated_dir = data_dir.join("generated");
        let run_dir = data_dir.join(".run");
        let workspace_dir = workspace_dir.unwrap_or_else(|| data_dir.clone());
        let package_root_kind = classify_package_root(&server_root).to_owned();

        Self {
            server_root,
            data_dir,
            generated_dir,
            run_dir,
            workspace_dir,
            launch_target,
            package_root_kind,
        }
    }

    pub fn prepare_directories(&self) -> io::Result<()> {
        fs::create_dir_all(&self.generated_dir)?;
        fs::create_dir_all(&self.run_dir)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerSnapshot {
    pub configured_max: usize,
    pub adaptive: bool,
    pub current_limit: usize,
    pub active: usize,
    pub queued: usize,
}

impl Default for SchedulerSnapshot {
    fn default() -> Self {
        Self {
            configured_max: MAX_RUN_JOBS,
            adaptive: false,
            current_limit: MAX_RUN_JOBS,
            active: 0,
            queued: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeIdentity {
    pub app: &'static str,
    pub server_root: String,
    pub data_dir: String,
    pub generated_dir: String,
    pub run_dir: String,
    pub workspace_dir: String,
    pub launch_target: String,
    pub package_name: &'static str,
    pub package_version: &'static str,
    pub package_root_kind: String,
    pub codex_app_server: AppServerDiagnostics,
    pub app_server_image_scheduler: SchedulerSnapshot,
}

impl RuntimeIdentity {
    pub fn from_config(
        config: &RuntimeConfig,
        codex_app_server: AppServerDiagnostics,
        app_server_image_scheduler: SchedulerSnapshot,
    ) -> Self {
        Self {
            app: APP_IDENTITY,
            server_root: display_path(&config.server_root),
            data_dir: display_path(&config.data_dir),
            generated_dir: display_path(&config.generated_dir),
            run_dir: display_path(&config.run_dir),
            workspace_dir: display_path(&config.workspace_dir),
            launch_target: config.launch_target.clone(),
            package_name: APP_IDENTITY,
            package_version: env!("CARGO_PKG_VERSION"),
            package_root_kind: config.package_root_kind.clone(),
            codex_app_server,
            app_server_image_scheduler,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub ok: bool,
    pub jobs: usize,
    pub app_server_image: bool,
    pub app_server_image_ready: bool,
    pub app_server_image_diagnostics: AppServerDiagnostics,
    #[serde(flatten)]
    pub identity_fields: RuntimeIdentity,
    pub identity: RuntimeIdentity,
}

impl HealthResponse {
    pub fn from_config(config: &RuntimeConfig) -> Self {
        Self::from_parts(
            config,
            AppServerDiagnostics::default(),
            SchedulerSnapshot::default(),
            0,
        )
    }

    pub fn from_parts(
        config: &RuntimeConfig,
        diagnostics: AppServerDiagnostics,
        scheduler: SchedulerSnapshot,
        jobs: usize,
    ) -> Self {
        let identity = RuntimeIdentity::from_config(config, diagnostics.clone(), scheduler);
        Self {
            ok: true,
            jobs,
            app_server_image: diagnostics.ready,
            app_server_image_ready: diagnostics.ready,
            app_server_image_diagnostics: diagnostics,
            identity_fields: identity.clone(),
            identity,
        }
    }
}

#[derive(Clone)]
struct RuntimeState {
    config: Arc<RuntimeConfig>,
    app_server: AppServerBridge,
    analysis: ReferenceAnalysisRuntime,
    generation: GenerationRuntime,
    host_opener: Arc<dyn HostOpener>,
}

impl RuntimeState {
    fn new(config: RuntimeConfig, launch: AppServerLaunchConfig) -> Self {
        Self::new_with_opener(config, launch, Arc::new(SystemHostOpener))
    }

    fn new_with_opener(
        config: RuntimeConfig,
        launch: AppServerLaunchConfig,
        host_opener: Arc<dyn HostOpener>,
    ) -> Self {
        let app_server = AppServerBridge::new(config.workspace_dir.clone(), launch);
        let config = Arc::new(config);
        let analysis = ReferenceAnalysisRuntime::new(config.clone(), app_server.clone());
        let generation = GenerationRuntime::new(config.clone(), app_server.clone());
        Self {
            config,
            app_server,
            analysis,
            generation,
            host_opener,
        }
    }
}

pub fn router(config: RuntimeConfig) -> Router {
    router_with_launch_config(config, AppServerLaunchConfig::from_environment())
}

fn router_with_launch_config(config: RuntimeConfig, launch: AppServerLaunchConfig) -> Router {
    let state = RuntimeState::new(config, launch);
    router_with_state(state)
}

fn router_with_state(state: RuntimeState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/events", get(events))
        .route("/api/run", axum::routing::post(run_single))
        .route("/api/run-batch", axum::routing::post(run_batch))
        .route(
            "/api/analyze-reference",
            axum::routing::post(analyze_reference),
        )
        .route(
            "/api/open-generated-dir",
            axum::routing::post(open_generated_dir),
        )
        .route(
            "/api/open-generated-file",
            axum::routing::post(open_generated_file),
        )
        .route("/api/runs", get(run_list))
        .route("/api/runs/{run_id}", get(run_status))
        .route("/api/generated", get(generated_list))
        .route("/generated/{run_id}/{filename}", get(generated_file))
        .route("/artifacts/{run_id}/image", get(generated_image_view))
        .route("/artifacts/{run_id}/{artifact}", get(artifact_view))
        .route(
            "/api/preflight/app-server-image",
            get(preflight).post(preflight),
        )
        .with_state(state)
}

async fn health(State(state): State<RuntimeState>) -> Json<HealthResponse> {
    let diagnostics = state.app_server.diagnostics().await;
    let scheduler = state.generation.scheduler_snapshot();
    let jobs = state.generation.job_count().await;
    Json(HealthResponse::from_parts(
        &state.config,
        diagnostics,
        scheduler,
        jobs,
    ))
}

async fn preflight(
    State(state): State<RuntimeState>,
) -> (StatusCode, Json<AppServerPreflightResponse>) {
    let diagnostics = state.app_server.ensure_ready().await;
    let status = if diagnostics.ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(AppServerPreflightResponse::from_diagnostics(diagnostics)),
    )
}

async fn analyze_reference(State(state): State<RuntimeState>, request: AxumRequest) -> Response {
    let body = match http_json::read_json_body(request).await {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    match state.analysis.analyze(body).await {
        Ok(premise) => (StatusCode::OK, Json(json!({ "premise": premise }))).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn open_generated_dir(State(state): State<RuntimeState>) -> Response {
    if let Err(error) = tokio::fs::create_dir_all(&state.config.generated_dir).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response();
    }
    let path = display_path(&state.config.generated_dir);
    if let Err(error) = state
        .host_opener
        .open(&[state.config.generated_dir.as_os_str().to_owned()])
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response();
    }
    (StatusCode::OK, Json(json!({ "ok": true, "path": path }))).into_response()
}

async fn open_generated_file(State(state): State<RuntimeState>, request: AxumRequest) -> Response {
    let body = match http_json::read_json_body(request).await {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    let job_id = compatible_request_string(body.get("jobId"), "")
        .trim()
        .to_owned();
    let action = compatible_request_string(body.get("action"), "reveal");
    if job_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "jobId is required" })),
        )
            .into_response();
    }
    if !matches!(action.as_str(), "reveal" | "open") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid action" })),
        )
            .into_response();
    }

    let path = match state.generation.generated_file_for_job(&job_id).await {
        Ok(path) => path,
        Err(GeneratedJobFileError::JobNotFound) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "job not found" })),
            )
                .into_response();
        }
        Err(GeneratedJobFileError::GeneratedFileNotFound) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "generated file not found" })),
            )
                .into_response();
        }
        Err(GeneratedJobFileError::Forbidden) => {
            return (StatusCode::FORBIDDEN, Json(json!({ "error": "forbidden" }))).into_response();
        }
        Err(GeneratedJobFileError::Io(error)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": error.to_string() })),
            )
                .into_response();
        }
    };
    let mut arguments = Vec::with_capacity(2);
    if action == "reveal" {
        arguments.push(OsString::from("-R"));
    }
    arguments.push(path.as_os_str().to_owned());
    if let Err(error) = state.host_opener.open(&arguments) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response();
    }
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "action": action,
            "path": display_path(&path)
        })),
    )
        .into_response()
}

fn compatible_request_string(value: Option<&Value>, fallback: &str) -> String {
    value
        .filter(|value| json_truthy(value))
        .map(json_string)
        .unwrap_or_else(|| fallback.to_owned())
}

fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn json_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(values) => values
            .iter()
            .map(|value| match value {
                Value::Null => String::new(),
                value => json_string(value),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

async fn events(
    State(state): State<RuntimeState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let snapshot = serde_json::to_string(&state.generation.snapshot().await)
        .unwrap_or_else(|_| "[]".to_owned());
    let initial = tokio_stream::once(Ok(Event::default().event("snapshot").data(snapshot)));
    let updates = BroadcastStream::new(state.generation.subscribe()).filter_map(|result| {
        let event = result.ok()?;
        let data = serde_json::to_string(&event.data).ok()?;
        Some(Ok(Event::default().event(event.name).data(data)))
    });
    Sse::new(initial.chain(updates))
}

async fn run_single(
    State(state): State<RuntimeState>,
    Query(query): Query<HashMap<String, String>>,
    request: AxumRequest,
) -> Response {
    let body = match http_json::read_json_body(request).await {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    create_run_response(state, body, false, query.get("waitMs").map(String::as_str)).await
}

async fn run_batch(
    State(state): State<RuntimeState>,
    Query(query): Query<HashMap<String, String>>,
    request: AxumRequest,
) -> Response {
    let body = match http_json::read_json_body(request).await {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    create_run_response(state, body, true, query.get("waitMs").map(String::as_str)).await
}

async fn create_run_response(
    state: RuntimeState,
    body: Value,
    require_prompts_array: bool,
    query_wait_ms: Option<&str>,
) -> Response {
    match state
        .generation
        .create_run(&body, require_prompts_array, query_wait_ms)
        .await
    {
        Ok((completed, response)) => (
            if completed {
                StatusCode::OK
            } else {
                StatusCode::ACCEPTED
            },
            Json(response),
        )
            .into_response(),
        Err(error) => (StatusCode::BAD_REQUEST, Json(error.body())).into_response(),
    }
}

async fn run_status(
    State(state): State<RuntimeState>,
    AxumPath(run_id): AxumPath<String>,
) -> Response {
    if !valid_run_id(&run_id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid run id" })),
        )
            .into_response();
    }
    match state.generation.run_response(&run_id, false).await {
        Some(response) => (StatusCode::OK, Json(response)).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "run not found" })),
        )
            .into_response(),
    }
}

async fn run_list(State(state): State<RuntimeState>) -> Json<Value> {
    Json(state.generation.run_list().await)
}

async fn generated_list(State(state): State<RuntimeState>) -> Response {
    match state.generation.generated_files().await {
        Ok(data) => (StatusCode::OK, Json(json!({ "data": data }))).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, Json(error.body())).into_response(),
    }
}

async fn generated_file(
    State(state): State<RuntimeState>,
    AxumPath((run_id, filename)): AxumPath<(String, String)>,
) -> Response {
    let path = match state.generation.generated_path(&run_id, &filename) {
        Ok(path) => path,
        Err(error) => return (StatusCode::BAD_REQUEST, Json(error.body())).into_response(),
    };
    file_response(path).await
}

async fn artifact_view(
    State(state): State<RuntimeState>,
    AxumPath((run_id, artifact)): AxumPath<(String, String)>,
) -> Response {
    let (path, label) = match state.generation.artifact_path(&run_id, &artifact) {
        Ok(value) => value,
        Err(error) => {
            let status = if error.error == "invalid run id" {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::NOT_FOUND
            };
            return (status, Json(error.body())).into_response();
        }
    };
    let content = match tokio::fs::read_to_string(path).await {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "artifact not found" })),
            )
                .into_response();
        }
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": error.to_string() })),
            )
                .into_response();
        }
    };
    html_response(render_artifact_page(&run_id, label, &content))
}

async fn generated_image_view(
    State(state): State<RuntimeState>,
    AxumPath(run_id): AxumPath<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(filename) = query.get("file") else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid image file" })),
        )
            .into_response();
    };
    let path = match state.generation.generated_path(&run_id, filename) {
        Ok(path) => path,
        Err(error) => return (StatusCode::BAD_REQUEST, Json(error.body())).into_response(),
    };
    match tokio::fs::metadata(&path).await {
        Ok(metadata) if metadata.is_file() => {}
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "image not found" })),
            )
                .into_response();
        }
    }
    if !content_type(&path).starts_with("image/") {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Json(json!({ "error": "not an image" })),
        )
            .into_response();
    }
    html_response(render_image_page(
        &run_id,
        filename,
        &format!("/generated/{run_id}/{filename}"),
    ))
}

async fn file_response(path: PathBuf) -> Response {
    match tokio::fs::read(&path).await {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type(&path))
            .body(Body::from(bytes))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            StatusCode::NOT_FOUND.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn html_response(content: String) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(content))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn classify_package_root(path: &Path) -> &'static str {
    let normalized = path.to_string_lossy().replace('\\', "/");
    if normalized.contains("/.codex/plugins/cache/") {
        "cache"
    } else if normalized.contains(".app/Contents/Resources/") {
        "packaged"
    } else if normalized.ends_with("/plugins/codex-image-grid-native") {
        "source"
    } else {
        "unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use http_body_util::BodyExt;
    use std::io::Write;
    use std::sync::Mutex as StdMutex;
    use tower::ServiceExt;

    #[test]
    fn fresh_health_preserves_the_baseline_shape_with_native_identity() {
        let config = RuntimeConfig::new(
            PathBuf::from("/Users/example/plugins/codex-image-grid-native"),
            PathBuf::from("/tmp/codex-image-grid-native"),
            None,
            "server".to_owned(),
        );

        let health = HealthResponse::from_config(&config);

        assert!(health.ok);
        assert_eq!(health.jobs, 0);
        assert!(!health.app_server_image);
        assert_eq!(health.app_server_image_diagnostics.status, "not-started");
        assert_eq!(health.identity.app, APP_IDENTITY);
        assert_eq!(health.identity.package_name, APP_IDENTITY);
        assert_eq!(health.identity.package_root_kind, "source");
        assert_eq!(
            health.identity.app_server_image_scheduler,
            SchedulerSnapshot::default()
        );
        assert_eq!(health.identity_fields, health.identity);
    }

    #[tokio::test]
    async fn provider_free_preflight_updates_health_without_hiding_the_server() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let server_root = temporary.path().join("server");
        let data_dir = temporary.path().join("data");
        let workspace = temporary.path().join("workspace");
        fs::create_dir_all(&server_root).expect("server root");
        fs::create_dir_all(&data_dir).expect("data root");
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

        let config =
            RuntimeConfig::new(server_root, data_dir, Some(workspace), "server".to_owned());
        let app = router_with_launch_config(config, AppServerLaunchConfig::single("fixture", fake));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/preflight/app-server-image")
                    .body(Body::empty())
                    .expect("preflight request"),
            )
            .await
            .expect("preflight response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("preflight body")
            .to_bytes();
        let preflight: serde_json::Value = serde_json::from_slice(&body).expect("preflight JSON");
        assert_eq!(preflight["ok"], true);
        assert_eq!(preflight["appServerImageReady"], true);
        assert_eq!(preflight["diagnostics"]["status"], "ready");
        assert_eq!(preflight["diagnostics"]["selectedSource"], "fixture");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .expect("health request"),
            )
            .await
            .expect("health response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("health body")
            .to_bytes();
        let health: serde_json::Value = serde_json::from_slice(&body).expect("health JSON");
        assert_eq!(health["ok"], true);
        assert_eq!(health["appServerImage"], true);
        assert_eq!(health["appServerImageReady"], true);
        assert_eq!(health["codexAppServer"]["status"], "ready");
        assert_eq!(health["identity"]["codexAppServer"]["status"], "ready");
    }

    #[tokio::test]
    async fn provider_free_run_writes_image_artifacts_and_compatible_routes() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let server_root = temporary.path().join("server");
        let data_dir = temporary.path().join("data");
        let workspace = temporary.path().join("workspace");
        fs::create_dir_all(&server_root).expect("server root");
        fs::create_dir_all(&data_dir).expect("data root");
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
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":2,"result":{"thread":{"id":"fixture-thread"}}}'
      ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":3,"result":{"turn":{"id":"fixture-turn"}}}'
      printf '%s\n' '{"method":"item/completed","params":{"threadId":"fixture-thread","turnId":"fixture-turn","item":{"type":"imageGeneration","id":"fixture-image","status":"completed","revisedPrompt":null,"result":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="}}}'
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"fixture-thread","turn":{"id":"fixture-turn","items":[],"itemsView":"full","status":"completed","error":null,"startedAt":null,"completedAt":null,"durationMs":1}}}'
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

        let config =
            RuntimeConfig::new(server_root, data_dir, Some(workspace), "server".to_owned());
        let app = router_with_launch_config(config, AppServerLaunchConfig::single("fixture", fake));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/events")
                    .body(Body::empty())
                    .expect("SSE request"),
            )
            .await
            .expect("SSE response");
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body().into_data_stream();
        let first_event = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            tokio_stream::StreamExt::next(&mut stream),
        )
        .await
        .expect("initial SSE deadline")
        .expect("initial SSE chunk")
        .expect("initial SSE data");
        let first_event = String::from_utf8(first_event.to_vec()).expect("SSE UTF-8");
        assert!(first_event.contains("event: snapshot"));
        assert!(first_event.contains("data: []"));
        drop(stream);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/run-batch")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "prompts": ["fixture prompt"],
                            "count": 1,
                            "mood": "warm-mascot",
                            "engine": "app-server-image",
                            "aspectRatio": "16:9",
                            "waitMs": 5000
                        })
                        .to_string(),
                    ))
                    .expect("run request"),
            )
            .await
            .expect("run response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("run body")
            .to_bytes();
        let run: Value = serde_json::from_slice(&body).expect("run JSON");
        let run_id = run["runId"].as_str().expect("run id");
        assert_eq!(run_id.len(), 8);
        assert_eq!(run["status"], "done");
        assert_eq!(run["completed"], true);
        assert_eq!(
            run["counts"],
            json!({ "total": 1, "done": 1, "running": 0, "failed": 0 })
        );
        assert_eq!(run["jobs"][0]["status"], "queued");
        assert_eq!(run["outputs"][0]["status"], "done");
        assert_eq!(run["outputs"][0]["filename"], "variant-01.png");
        assert_eq!(run["outputs"][0]["threadId"], "fixture-thread");
        assert_eq!(run["outputs"][0]["turnId"], "fixture-turn");

        let image_url = run["outputs"][0]["imageUrl"].as_str().expect("image URL");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(image_url)
                    .body(Body::empty())
                    .expect("image request"),
            )
            .await
            .expect("image response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
        let image_bytes = response
            .into_body()
            .collect()
            .await
            .expect("image body")
            .to_bytes();
        assert_eq!(
            image_bytes.as_ref(),
            BASE64_STANDARD
                .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
                .expect("fixture PNG")
        );

        let manifest_url = run["manifestUrl"].as_str().expect("manifest URL");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(manifest_url)
                    .body(Body::empty())
                    .expect("manifest request"),
            )
            .await
            .expect("manifest response");
        assert_eq!(response.status(), StatusCode::OK);
        let manifest_bytes = response
            .into_body()
            .collect()
            .await
            .expect("manifest body")
            .to_bytes();
        let manifest: Value = serde_json::from_slice(&manifest_bytes).expect("manifest JSON");
        assert_eq!(manifest["schemaVersion"], 1);
        assert_eq!(manifest["runId"], run_id);
        assert_eq!(manifest["outputs"][0]["status"], "done");

        let handoff_url = run["handoffUrl"].as_str().expect("handoff URL");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(handoff_url)
                    .body(Body::empty())
                    .expect("handoff request"),
            )
            .await
            .expect("handoff response");
        assert_eq!(response.status(), StatusCode::OK);
        let handoff = response
            .into_body()
            .collect()
            .await
            .expect("handoff body")
            .to_bytes();
        let handoff = String::from_utf8(handoff.to_vec()).expect("handoff UTF-8");
        assert!(handoff.contains("# Codex Image Grid Handoff"));
        assert!(handoff.contains("## Request"));
        assert!(handoff.contains("## Diagnostics"));
        assert!(handoff.contains("## Outputs"));

        for uri in [
            format!("/api/runs/{run_id}"),
            "/api/runs".to_owned(),
            "/api/generated".to_owned(),
            run["manifestViewUrl"]
                .as_str()
                .expect("manifest view")
                .to_owned(),
            run["handoffViewUrl"]
                .as_str()
                .expect("handoff view")
                .to_owned(),
            format!("/artifacts/{run_id}/image?file=variant-01.png"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(uri)
                        .body(Body::empty())
                        .expect("compatibility route request"),
                )
                .await
                .expect("compatibility route response");
            assert_eq!(response.status(), StatusCode::OK);
        }
    }

    #[tokio::test]
    async fn provider_free_run_batch_stages_frozen_inline_reference_once() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let server_root = temporary.path().join("server");
        let data_dir = temporary.path().join("data");
        let workspace = temporary.path().join("workspace");
        fs::create_dir_all(&server_root).expect("server root");
        fs::create_dir_all(&data_dir).expect("data root");
        fs::create_dir_all(&workspace).expect("workspace");

        let request_log = temporary.path().join("app-server-requests.jsonl");
        let captured_reference = temporary.path().join("captured-reference.jpg");
        assert!(!request_log.to_string_lossy().contains('\''));
        assert!(!captured_reference.to_string_lossy().contains('\''));
        let fake = temporary.path().join("fake-codex");
        let fake_source = r#"#!/bin/sh
test "$1" = "app-server" || exit 2
request_log='__REQUEST_LOG__'
captured_reference='__CAPTURED_REFERENCE__'
while IFS= read -r line; do
  printf '%s\n' "$line" >>"$request_log"
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":1,"result":{"userAgent":"fixture","codexHome":"/tmp/fixture","platformFamily":"unix","platformOs":"macos"}}'
      ;;
    *'"method":"initialized"'*)
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":2,"result":{"thread":{"id":"fixture-thread"}}}'
      ;;
    *'"method":"turn/start"'*)
      staged_path=$(printf '%s\n' "$line" | /usr/bin/sed -n 's/.*"path":"\([^"]*\)".*/\1/p')
      /bin/cp "$staged_path" "$captured_reference" || exit 3
      printf '%s\n' '{"id":3,"result":{"turn":{"id":"fixture-turn"}}}'
      printf '%s\n' '{"method":"item/completed","params":{"threadId":"fixture-thread","turnId":"fixture-turn","item":{"type":"imageGeneration","id":"fixture-image","status":"completed","revisedPrompt":null,"result":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="}}}'
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"fixture-thread","turn":{"id":"fixture-turn","items":[],"itemsView":"full","status":"completed","error":null,"startedAt":null,"completedAt":null,"durationMs":1}}}'
      ;;
  esac
done
"#
        .replace("__REQUEST_LOG__", &request_log.to_string_lossy())
        .replace(
            "__CAPTURED_REFERENCE__",
            &captured_reference.to_string_lossy(),
        );
        let mut file = fs::File::create(&fake).expect("fake executable");
        file.write_all(fake_source.as_bytes()).expect("fake source");
        file.flush().expect("fake source flushed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = file.metadata().expect("fake metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&fake, permissions).expect("fake executable permissions");
        }

        let config = RuntimeConfig::new(
            server_root,
            data_dir.clone(),
            Some(workspace),
            "server".to_owned(),
        );
        let app = router_with_launch_config(config, AppServerLaunchConfig::single("fixture", fake));
        let reference_bytes = [0xff, 0xd8, 0xff, 0xd9];
        let unused_http_path = temporary.path().join("must-not-be-read.png");
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/run-batch?waitMs=5000")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "prompts": ["inline reference fixture"],
                            "count": 1,
                            "mood": "warm-mascot",
                            "engine": "app-server-image",
                            "aspectRatio": "16:9",
                            "referenceImage": {
                                "dataUrl": format!(
                                    "data:image/jpeg;base64,{}",
                                    BASE64_STANDARD.encode(reference_bytes)
                                ),
                                "mimeType": "image/jpeg",
                                "name": "browser-reference.jpeg",
                                "size": reference_bytes.len()
                            },
                            "referenceImagePath": unused_http_path.to_string_lossy()
                        })
                        .to_string(),
                    ))
                    .expect("run request"),
            )
            .await
            .expect("run response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("run body")
            .to_bytes();
        let run: Value = serde_json::from_slice(&body).expect("run JSON");
        let run_id = run["runId"].as_str().expect("run id");
        assert_eq!(run["status"], "done");
        assert_eq!(run["completed"], true);
        let staged_path = fs::canonicalize(
            data_dir
                .join("generated")
                .join(run_id)
                .join("reference.jpg"),
        )
        .expect("staged reference");
        let staged_path_display = display_path(&staged_path);
        let reference_url = format!("/generated/{run_id}/reference.jpg");
        assert_eq!(
            fs::read(&staged_path).expect("staged bytes"),
            reference_bytes
        );
        assert_eq!(
            fs::read(&captured_reference).expect("captured reference"),
            reference_bytes
        );
        for output in [&run["jobs"][0], &run["outputs"][0]] {
            assert_eq!(output["referenceImagePath"], staged_path_display);
            assert_eq!(output["referenceImageUrl"], reference_url);
            assert_eq!(output["outputPath"].as_str().is_some(), true);
        }

        let request_messages = fs::read_to_string(&request_log).expect("App Server request log");
        assert!(!request_messages.contains(&display_path(&unused_http_path)));
        let messages = request_messages
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("logged JSON request"))
            .collect::<Vec<_>>();
        let turn_start = messages
            .iter()
            .find(|message| message["method"] == "turn/start")
            .expect("turn/start request");
        assert_eq!(
            turn_start["params"]["input"][1],
            json!({
                "type": "localImage",
                "path": staged_path_display
            })
        );

        let manifest_path = PathBuf::from(
            run["manifestPath"]
                .as_str()
                .expect("manifest path in response"),
        );
        let manifest: Value =
            serde_json::from_slice(&fs::read(manifest_path).expect("persisted manifest bytes"))
                .expect("manifest JSON");
        assert_eq!(
            manifest["request"]["referenceImage"],
            json!({
                "path": staged_path_display,
                "url": reference_url
            })
        );
        assert_eq!(
            manifest["outputs"][0]["referenceImagePath"],
            staged_path_display
        );
        assert_eq!(manifest["outputs"][0]["referenceImageUrl"], reference_url);
        assert_eq!(
            manifest["outputs"][0]["outputPath"],
            run["outputs"][0]["outputPath"]
        );

        let handoff = fs::read_to_string(
            run["handoffPath"]
                .as_str()
                .expect("handoff path in response"),
        )
        .expect("persisted handoff");
        assert!(handoff.contains(&format!("- Reference image: {staged_path_display}")));
        assert!(handoff.contains("## Outputs"));
        assert!(!unused_http_path.exists());
    }

    #[tokio::test]
    async fn provider_free_reference_analysis_stages_local_image_and_cleans_up() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let server_root = temporary.path().join("server");
        let data_dir = temporary.path().join("data");
        let workspace = temporary.path().join("workspace");
        fs::create_dir_all(&server_root).expect("server root");
        fs::create_dir_all(&data_dir).expect("data root");
        fs::create_dir_all(&workspace).expect("workspace");

        let reference_path = temporary.path().join("selected-reference.png");
        let reference_bytes = b"provider-free reference fixture";
        fs::write(&reference_path, reference_bytes).expect("reference fixture");
        let request_log = temporary.path().join("app-server-requests.jsonl");
        let captured_reference = temporary.path().join("captured-reference.png");
        assert!(!request_log.to_string_lossy().contains('\''));
        assert!(!captured_reference.to_string_lossy().contains('\''));

        let fake = temporary.path().join("fake-codex");
        let fake_source = r#"#!/bin/sh
test "$1" = "app-server" || exit 2
request_log='__REQUEST_LOG__'
captured_reference='__CAPTURED_REFERENCE__'
while IFS= read -r line; do
  printf '%s\n' "$line" >>"$request_log"
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":1,"result":{"userAgent":"fixture","codexHome":"/tmp/fixture","platformFamily":"unix","platformOs":"macos"}}'
      ;;
    *'"method":"initialized"'*)
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":2,"result":{"thread":{"id":"analysis-thread"}}}'
      ;;
    *'"method":"turn/start"'*)
      staged_path=$(printf '%s\n' "$line" | /usr/bin/sed -n 's/.*"path":"\([^"]*\)".*/\1/p')
      /bin/cp "$staged_path" "$captured_reference" || exit 3
      printf '%s\n' '{"id":3,"result":{"turn":{"id":"analysis-turn"}}}'
      printf '%s\n' '{"method":"item/agentMessage/delta","params":{"threadId":"analysis-thread","turnId":"analysis-turn","itemId":"analysis-item","delta":"- ignored delta\n"}}'
      printf '%s\n' '{"method":"item/completed","params":{"threadId":"analysis-thread","turnId":"analysis-turn","completedAtMs":1,"item":{"type":"agentMessage","id":"analysis-item","text":"  - 青い髪\n- 星型アクセサリー  "}}}'
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"analysis-thread","turn":{"id":"analysis-turn","items":[],"itemsView":"full","status":"completed","error":null,"startedAt":null,"completedAt":null,"durationMs":1}}}'
      ;;
  esac
done
"#
        .replace("__REQUEST_LOG__", &request_log.to_string_lossy())
        .replace(
            "__CAPTURED_REFERENCE__",
            &captured_reference.to_string_lossy(),
        );
        let mut file = fs::File::create(&fake).expect("fake executable");
        file.write_all(fake_source.as_bytes()).expect("fake source");
        file.flush().expect("fake source flushed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = file.metadata().expect("fake metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&fake, permissions).expect("fake executable permissions");
        }

        let config = RuntimeConfig::new(
            server_root,
            data_dir.clone(),
            Some(workspace.clone()),
            "server".to_owned(),
        );
        let app = router_with_launch_config(config, AppServerLaunchConfig::single("fixture", fake));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/analyze-reference")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "referenceImagePath": reference_path.to_string_lossy()
                        })
                        .to_string(),
                    ))
                    .expect("analysis request"),
            )
            .await
            .expect("analysis response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("analysis body")
            .to_bytes();
        let payload: Value = serde_json::from_slice(&body).expect("analysis JSON");
        assert_eq!(payload["premise"], "- 青い髪\n- 星型アクセサリー");
        assert_eq!(
            fs::read(&captured_reference).expect("captured staged bytes"),
            reference_bytes
        );

        let messages = fs::read_to_string(&request_log).expect("App Server request log");
        let messages = messages
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("logged JSON request"))
            .collect::<Vec<_>>();
        let thread_start = messages
            .iter()
            .find(|message| message["method"] == "thread/start")
            .expect("thread/start request");
        assert_eq!(thread_start["params"]["cwd"], display_path(&workspace));
        assert_eq!(thread_start["params"]["approvalPolicy"], "never");
        assert_eq!(thread_start["params"]["sandbox"], "read-only");
        assert_eq!(
            thread_start["params"]["serviceName"],
            "codex_image_grid_reference_analysis"
        );
        assert_eq!(thread_start["params"]["ephemeral"], true);

        let turn_start = messages
            .iter()
            .find(|message| message["method"] == "turn/start")
            .expect("turn/start request");
        assert_eq!(turn_start["params"]["threadId"], "analysis-thread");
        assert_eq!(turn_start["params"]["cwd"], display_path(&workspace));
        assert_eq!(turn_start["params"]["approvalPolicy"], "never");
        assert_eq!(turn_start["params"]["effort"], "medium");
        assert_eq!(
            turn_start["params"]["sandboxPolicy"],
            json!({ "type": "readOnly", "networkAccess": false })
        );
        assert_eq!(
            turn_start["params"]["input"][0],
            json!({
                "type": "text",
                "text": analysis::ANALYZE_PROMPT,
                "text_elements": []
            })
        );
        let staged_path = PathBuf::from(
            turn_start["params"]["input"][1]["path"]
                .as_str()
                .expect("local image path"),
        );
        assert_eq!(turn_start["params"]["input"][1]["type"], "localImage");
        let analysis_root = data_dir.join(".run").join("reference-analysis");
        assert!(staged_path.starts_with(
            fs::canonicalize(&analysis_root).expect("canonical reference-analysis root")
        ));
        assert_eq!(
            staged_path.file_name().and_then(|name| name.to_str()),
            Some("reference.png")
        );
        assert!(!staged_path.exists());

        assert!(
            fs::read_dir(analysis_root)
                .expect("analysis root")
                .next()
                .is_none()
        );
    }

    #[derive(Default)]
    struct RecordingHostOpener {
        calls: StdMutex<Vec<Vec<OsString>>>,
        fail: bool,
    }

    impl HostOpener for RecordingHostOpener {
        fn open(&self, arguments: &[OsString]) -> io::Result<()> {
            self.calls
                .lock()
                .expect("host opener calls")
                .push(arguments.to_vec());
            if self.fail {
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "fixture open command unavailable",
                ))
            } else {
                Ok(())
            }
        }
    }

    fn finder_test_job(job_id: &str, output_path: &Path) -> runtime::ImageGridJob {
        runtime::ImageGridJob {
            id: job_id.to_owned(),
            run_id: "feedface".to_owned(),
            engine: "app-server-image".to_owned(),
            model: "gpt-image-1".to_owned(),
            prompt: "finder fixture".to_owned(),
            reference_premise: String::new(),
            mood: "warm-mascot".to_owned(),
            prompt_index: 1,
            prompt_total: 1,
            variant: 1,
            total: 1,
            filename: "variant-01.png".to_owned(),
            output_path: display_path(output_path),
            aspect_ratio: "16:9".to_owned(),
            reference_image_path: None,
            reference_image_url: None,
            manifest_path: "/tmp/manifest.json".to_owned(),
            manifest_url: "/generated/feedface/manifest.json".to_owned(),
            manifest_view_url: "/artifacts/feedface/manifest".to_owned(),
            handoff_path: "/tmp/handoff.md".to_owned(),
            handoff_url: "/generated/feedface/handoff.md".to_owned(),
            handoff_view_url: "/artifacts/feedface/handoff".to_owned(),
            output_format: "png".to_owned(),
            status: "done".to_owned(),
            status_text: "Generated".to_owned(),
            image_url: Some("/generated/feedface/variant-01.png".to_owned()),
            log: String::new(),
            thread_id: None,
            turn_id: None,
            error_code: None,
            error_message: None,
            upstream_status: Some("completed".to_owned()),
            diagnostic_log: String::new(),
            retry_count: 0,
            timing: runtime::JobTiming {
                phase: "done".to_owned(),
                phase_changed_at: 1,
                enqueued_at: 1,
                dequeued_at: Some(1),
                first_started_at: Some(1),
                first_running_at: Some(1),
                completed_at: Some(1),
                queue_ms: Some(0),
                execution_ms: Some(0),
                total_ms: Some(0),
                cooldown_ms: 0,
                attempt_count: 1,
                current_attempt_started_at: None,
                last_attempt_completed_at: Some(1),
                last_attempt_ms: Some(0),
            },
            created_at: 1,
            updated_at: 1,
        }
    }

    async fn finder_test_app(
        temporary: &tempfile::TempDir,
        opener: Arc<RecordingHostOpener>,
    ) -> (Router, GenerationRuntime, RuntimeConfig) {
        let config = RuntimeConfig::new(
            temporary.path().join("server"),
            temporary.path().join("data"),
            Some(temporary.path().join("workspace")),
            "server".to_owned(),
        );
        fs::create_dir_all(&config.server_root).expect("server root");
        fs::create_dir_all(&config.workspace_dir).expect("workspace");
        let state = RuntimeState::new_with_opener(
            config.clone(),
            AppServerLaunchConfig::single("unused", temporary.path().join("unused-codex")),
            opener,
        );
        let generation = state.generation.clone();
        (router_with_state(state), generation, config)
    }

    async fn finder_request(app: &Router, uri: &str, body: impl Into<Body>) -> (StatusCode, Value) {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(body.into())
                    .expect("finder request"),
            )
            .await
            .expect("finder response");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("finder response body")
            .to_bytes();
        let body = serde_json::from_slice(&bytes).expect("finder response JSON");
        (status, body)
    }

    #[tokio::test]
    async fn finder_http_routes_use_owned_paths_and_exact_open_arguments() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let opener = Arc::new(RecordingHostOpener::default());
        let (app, generation, config) = finder_test_app(&temporary, opener.clone()).await;

        let (status, body) = finder_request(&app, "/api/open-generated-dir", Body::from("{")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            json!({
                "ok": true,
                "path": display_path(&config.generated_dir)
            })
        );
        assert!(config.generated_dir.is_dir());

        let run_directory = config.generated_dir.join("feedface");
        fs::create_dir_all(&run_directory).expect("run directory");
        let output_path = run_directory.join("variant-01.png");
        fs::write(&output_path, b"fixture image").expect("generated file");
        generation
            .insert_test_job(finder_test_job("42", &output_path))
            .await;
        let real_output = fs::canonicalize(&output_path).expect("canonical generated file");
        let caller_path = display_path(&temporary.path().join("caller-controlled.png"));

        let (status, body) = finder_request(
            &app,
            "/api/open-generated-file",
            Body::from(
                json!({
                    "jobId": 42,
                    "outputPath": caller_path
                })
                .to_string(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            json!({
                "ok": true,
                "action": "reveal",
                "path": display_path(&real_output)
            })
        );

        let (status, body) = finder_request(
            &app,
            "/api/open-generated-file",
            Body::from(json!({ "jobId": " 42 ", "action": "open" }).to_string()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["action"], "open");
        assert_eq!(body["path"], display_path(&real_output));

        let calls = opener.calls.lock().expect("host opener calls");
        assert_eq!(
            calls.as_slice(),
            &[
                vec![config.generated_dir.as_os_str().to_owned()],
                vec![OsString::from("-R"), real_output.as_os_str().to_owned()],
                vec![real_output.as_os_str().to_owned()]
            ]
        );
    }

    #[tokio::test]
    async fn finder_http_routes_reject_unowned_unsafe_and_invalid_requests() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let opener = Arc::new(RecordingHostOpener::default());
        let (app, generation, config) = finder_test_app(&temporary, opener.clone()).await;
        fs::create_dir_all(config.generated_dir.join("feedface")).expect("generated root");

        let outside_path = temporary.path().join("outside.png");
        fs::write(&outside_path, b"outside").expect("outside file");
        let traversal = config
            .generated_dir
            .join("feedface")
            .join("..")
            .join("..")
            .join("..")
            .join("outside.png");
        generation
            .insert_test_job(finder_test_job("traversal", &traversal))
            .await;

        let missing_path = config.generated_dir.join("feedface").join("missing.png");
        generation
            .insert_test_job(finder_test_job("missing", &missing_path))
            .await;

        let directory_path = config.generated_dir.join("feedface").join("directory.png");
        fs::create_dir_all(&directory_path).expect("directory output");
        generation
            .insert_test_job(finder_test_job("directory", &directory_path))
            .await;

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let outside_link = config
                .generated_dir
                .join("feedface")
                .join("outside-link.png");
            symlink(&outside_path, &outside_link).expect("outside symlink");
            generation
                .insert_test_job(finder_test_job("outside-link", &outside_link))
                .await;

            let inside_target = config
                .generated_dir
                .join("feedface")
                .join("inside-target.png");
            fs::write(&inside_target, b"inside").expect("inside target");
            let inside_link = config
                .generated_dir
                .join("feedface")
                .join("inside-link.png");
            symlink(&inside_target, &inside_link).expect("inside symlink");
            generation
                .insert_test_job(finder_test_job("inside-link", &inside_link))
                .await;
        }

        for (payload, expected_status, expected_error) in [
            (
                json!({ "jobId": "not-owned" }),
                StatusCode::NOT_FOUND,
                "job not found",
            ),
            (
                json!({ "jobId": "traversal" }),
                StatusCode::FORBIDDEN,
                "forbidden",
            ),
            (
                json!({ "jobId": "missing" }),
                StatusCode::NOT_FOUND,
                "generated file not found",
            ),
            (
                json!({ "jobId": "directory" }),
                StatusCode::NOT_FOUND,
                "generated file not found",
            ),
            (json!({}), StatusCode::BAD_REQUEST, "jobId is required"),
            (
                json!({ "jobId": "missing", "action": "preview" }),
                StatusCode::BAD_REQUEST,
                "invalid action",
            ),
        ] {
            let (status, body) = finder_request(
                &app,
                "/api/open-generated-file",
                Body::from(payload.to_string()),
            )
            .await;
            assert_eq!(status, expected_status);
            assert_eq!(body, json!({ "error": expected_error }));
        }

        #[cfg(unix)]
        for (job_id, expected_status, expected_error) in [
            ("outside-link", StatusCode::FORBIDDEN, "forbidden"),
            (
                "inside-link",
                StatusCode::NOT_FOUND,
                "generated file not found",
            ),
        ] {
            let (status, body) = finder_request(
                &app,
                "/api/open-generated-file",
                Body::from(json!({ "jobId": job_id }).to_string()),
            )
            .await;
            assert_eq!(status, expected_status);
            assert_eq!(body, json!({ "error": expected_error }));
        }

        let (status, body) =
            finder_request(&app, "/api/open-generated-file", Body::from("{")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "InvalidJsonBody");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/open-generated-file")
                    .header(
                        header::CONTENT_LENGTH,
                        http_json::DEFAULT_JSON_BODY_BYTES + 1,
                    )
                    .body(Body::empty())
                    .expect("oversized finder request"),
            )
            .await
            .expect("oversized finder response");
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let body: Value = serde_json::from_slice(
            &response
                .into_body()
                .collect()
                .await
                .expect("oversized response body")
                .to_bytes(),
        )
        .expect("oversized response JSON");
        assert_eq!(body["code"], "RequestBodyTooLarge");
        assert!(opener.calls.lock().expect("host opener calls").is_empty());
    }

    #[tokio::test]
    async fn finder_http_routes_report_host_spawn_errors_without_real_open() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let opener = Arc::new(RecordingHostOpener {
            calls: StdMutex::new(Vec::new()),
            fail: true,
        });
        let (app, generation, config) = finder_test_app(&temporary, opener).await;

        let (status, body) = finder_request(&app, "/api/open-generated-dir", Body::empty()).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"], "fixture open command unavailable");

        let run_directory = config.generated_dir.join("feedface");
        fs::create_dir_all(&run_directory).expect("run directory");
        let output_path = run_directory.join("variant-01.png");
        fs::write(&output_path, b"fixture image").expect("generated file");
        generation
            .insert_test_job(finder_test_job("owned", &output_path))
            .await;
        let (status, body) = finder_request(
            &app,
            "/api/open-generated-file",
            Body::from(json!({ "jobId": "owned" }).to_string()),
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"], "fixture open command unavailable");
    }
}
