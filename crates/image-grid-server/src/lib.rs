use axum::Json;
use axum::Router;
use axum::extract::State;
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
pub struct AppServerDiagnostics {
    pub status: &'static str,
    pub ready: bool,
    pub selected_command: Option<String>,
    pub selected_source: Option<String>,
    pub candidates: Vec<String>,
    pub error: Option<String>,
    pub platform_os: Option<String>,
    pub checked_at: Option<String>,
}

impl Default for AppServerDiagnostics {
    fn default() -> Self {
        Self {
            status: "not-started",
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
    pub fn from_config(config: &RuntimeConfig) -> Self {
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
            codex_app_server: AppServerDiagnostics::default(),
            app_server_image_scheduler: SchedulerSnapshot::default(),
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
        let identity = RuntimeIdentity::from_config(config);
        Self {
            ok: true,
            jobs: 0,
            app_server_image: false,
            app_server_image_ready: false,
            app_server_image_diagnostics: AppServerDiagnostics::default(),
            identity_fields: identity.clone(),
            identity,
        }
    }
}

pub fn router(config: RuntimeConfig) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .with_state(Arc::new(config))
}

async fn health(State(config): State<Arc<RuntimeConfig>>) -> Json<HealthResponse> {
    Json(HealthResponse::from_config(&config))
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
}
