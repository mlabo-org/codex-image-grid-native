mod app_server;

pub use app_server::{
    AppServerCandidateDiagnostic, AppServerDiagnosticError, AppServerDiagnostics,
    AppServerPreflightResponse,
};

use app_server::{AppServerBridge, AppServerLaunchConfig};
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use image_grid_core::{APP_IDENTITY, MAX_RUN_JOBS};
use serde::Serialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
        )
    }

    pub fn from_parts(
        config: &RuntimeConfig,
        diagnostics: AppServerDiagnostics,
        scheduler: SchedulerSnapshot,
    ) -> Self {
        let identity = RuntimeIdentity::from_config(config, diagnostics.clone(), scheduler);
        Self {
            ok: true,
            jobs: 0,
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
}

impl RuntimeState {
    fn new(config: RuntimeConfig, launch: AppServerLaunchConfig) -> Self {
        let app_server = AppServerBridge::new(config.workspace_dir.clone(), launch);
        Self {
            config: Arc::new(config),
            app_server,
        }
    }
}

pub fn router(config: RuntimeConfig) -> Router {
    router_with_launch_config(config, AppServerLaunchConfig::from_environment())
}

fn router_with_launch_config(config: RuntimeConfig, launch: AppServerLaunchConfig) -> Router {
    let state = RuntimeState::new(config, launch);
    Router::new()
        .route("/api/health", get(health))
        .route(
            "/api/preflight/app-server-image",
            get(preflight).post(preflight),
        )
        .with_state(state)
}

async fn health(State(state): State<RuntimeState>) -> Json<HealthResponse> {
    let diagnostics = state.app_server.diagnostics().await;
    Json(HealthResponse::from_parts(
        &state.config,
        diagnostics,
        SchedulerSnapshot::default(),
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
    use http_body_util::BodyExt;
    use std::io::Write;
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
}
