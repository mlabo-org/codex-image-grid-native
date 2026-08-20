use crate::app_server::{AppServerBridge, RUNTIME_CLOSED_MESSAGE};
use crate::{RuntimeConfig, RuntimeIdentity, SchedulerSnapshot};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use image_grid_core::{
    APP_IDENTITY, MAX_PROMPTS, MAX_REFERENCE_IMAGE_BYTES, MAX_RUN_JOBS, MAX_VARIANTS_PER_PROMPT,
    MAX_WAIT_MS, stage_reference_image,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use time::format_description::well_known::Rfc3339;
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time};
use tokio::fs;
use tokio::sync::{Mutex, Notify, RwLock, Semaphore, broadcast, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep, timeout_at};
use uuid::Uuid;

const APP_SERVER_REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
const APP_SERVER_JOB_TIMEOUT: Duration = Duration::from_secs(900);
const APP_SERVER_IMAGE_MAX_RETRIES_ENV: &str = "IMAGE_GRID_APP_SERVER_IMAGE_MAX_RETRIES";
const APP_SERVER_IMAGE_MAX_RETRIES: u32 = 1;
const APP_SERVER_IMAGE_MAX_RETRIES_LIMIT: u32 = 3;
const APP_SERVER_IMAGE_RETRY_BASE: Duration = Duration::from_secs(4);
const APP_SERVER_IMAGE_RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(45);
const APP_SERVER_IMAGE_RATE_LIMIT_COOLDOWN_MAX: Duration = Duration::from_secs(180);
const CODEX_SVG_CONCURRENCY: usize = 1;

const MOODS: [&str; 5] = [
    "warm-mascot",
    "clean-thumbnail",
    "editorial-soft",
    "cinematic",
    "minimal-product",
];
const ASPECT_RATIOS: [&str; 5] = ["16:9", "4:3", "1:1", "3:4", "9:16"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobTiming {
    pub phase: String,
    pub phase_changed_at: i64,
    pub enqueued_at: i64,
    pub dequeued_at: Option<i64>,
    pub first_started_at: Option<i64>,
    pub first_running_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub queue_ms: Option<i64>,
    pub execution_ms: Option<i64>,
    pub total_ms: Option<i64>,
    pub cooldown_ms: i64,
    pub attempt_count: u32,
    pub current_attempt_started_at: Option<i64>,
    pub last_attempt_completed_at: Option<i64>,
    pub last_attempt_ms: Option<i64>,
}

impl JobTiming {
    fn queued(now: i64) -> Self {
        Self {
            phase: "queued".to_owned(),
            phase_changed_at: now,
            enqueued_at: now,
            dequeued_at: None,
            first_started_at: None,
            first_running_at: None,
            completed_at: None,
            queue_ms: None,
            execution_ms: None,
            total_ms: None,
            cooldown_ms: 0,
            attempt_count: 0,
            current_attempt_started_at: None,
            last_attempt_completed_at: None,
            last_attempt_ms: None,
        }
    }

    fn transition(&mut self, status: &str, now: i64) {
        self.phase = status.to_owned();
        self.phase_changed_at = now;
        match status {
            "starting" => {
                self.dequeued_at.get_or_insert(now);
                self.first_started_at.get_or_insert(now);
                self.queue_ms
                    .get_or_insert(now.saturating_sub(self.enqueued_at));
                self.attempt_count = self.attempt_count.saturating_add(1);
                self.current_attempt_started_at = Some(now);
            }
            "running" => {
                self.first_running_at.get_or_insert(now);
            }
            "done" | "error" => {
                self.completed_at = Some(now);
                let started = self
                    .current_attempt_started_at
                    .or(self.first_started_at)
                    .unwrap_or(self.enqueued_at);
                self.execution_ms = Some(now.saturating_sub(started));
                self.total_ms = Some(now.saturating_sub(self.enqueued_at));
                self.last_attempt_completed_at = Some(now);
                self.last_attempt_ms = Some(now.saturating_sub(started));
                self.current_attempt_started_at = None;
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImageGridJob {
    pub id: String,
    pub run_id: String,
    pub engine: String,
    pub model: String,
    pub prompt: String,
    pub reference_premise: String,
    pub mood: String,
    pub prompt_index: usize,
    pub prompt_total: usize,
    pub variant: usize,
    pub total: usize,
    pub filename: String,
    pub output_path: String,
    pub aspect_ratio: String,
    pub reference_image_path: Option<String>,
    pub reference_image_url: Option<String>,
    pub manifest_path: String,
    pub manifest_url: String,
    pub manifest_view_url: String,
    pub handoff_path: String,
    pub handoff_url: String,
    pub handoff_view_url: String,
    pub output_format: String,
    pub status: String,
    pub status_text: String,
    pub image_url: Option<String>,
    pub log: String,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub upstream_status: Option<String>,
    pub diagnostic_log: String,
    pub retry_count: u32,
    pub timing: JobTiming,
    pub created_at: i64,
    pub updated_at: i64,
}

impl ImageGridJob {
    fn is_active(&self) -> bool {
        matches!(self.status.as_str(), "queued" | "starting" | "running")
    }

    fn is_terminal(&self) -> bool {
        matches!(self.status.as_str(), "done" | "error")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeEvent {
    pub name: String,
    pub data: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeneratedFile {
    pub run_id: String,
    pub file: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeleteRunFailure {
    pub run_id: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeleteRunsResult {
    pub ok: bool,
    pub deleted_run_ids: Vec<String>,
    pub deleted_job_count: usize,
    pub failures: Vec<DeleteRunFailure>,
}

#[derive(Debug)]
pub(crate) enum DeleteRunsError {
    ActiveRuns(Vec<String>),
    Io(io::Error),
}

#[derive(Debug, Clone)]
pub(crate) struct RunApiError {
    pub error: String,
    pub code: Option<String>,
}

#[derive(Debug)]
pub(crate) enum GeneratedJobFileError {
    JobNotFound,
    GeneratedFileNotFound,
    Forbidden,
    Io(io::Error),
}

impl RunApiError {
    fn new(error: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            code: Some(code.into()),
        }
    }

    fn message(error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            code: None,
        }
    }

    pub(crate) fn body(&self) -> Value {
        let mut body = json!({ "error": self.error });
        if let Some(code) = &self.code {
            body["code"] = Value::String(code.clone());
        }
        body
    }
}

#[derive(Debug, Clone)]
struct NormalizedRunRequest {
    prompts: Vec<String>,
    count: usize,
    mood: String,
    engine: String,
    aspect_ratio: String,
    reference_premise: String,
    inline_reference_image: Option<InlineReferenceImage>,
    reference_image_path: Option<PathBuf>,
    wait_ms: u64,
}

#[derive(Debug, Clone)]
struct InlineReferenceImage {
    bytes: Vec<u8>,
    extension: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PromptRecord {
    index: usize,
    prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ReferenceRecord {
    path: String,
    url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunRequestRecord {
    prompts: Vec<PromptRecord>,
    mood: String,
    engine: String,
    model: String,
    aspect_ratio: String,
    variants_per_prompt: usize,
    prompt_total: usize,
    reference_premise: String,
    reference_image: Option<ReferenceRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct RunArtifacts {
    manifest_path: String,
    manifest_url: String,
    manifest_view_url: String,
    handoff_path: String,
    handoff_url: String,
    handoff_view_url: String,
}

#[derive(Clone)]
struct RunRecord {
    run_id: String,
    job_ids: Vec<String>,
    initial_jobs: Vec<ImageGridJob>,
    request: RunRequestRecord,
    artifacts: RunArtifacts,
    created_at: i64,
    notify: Arc<Notify>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedRunManifest {
    schema_version: u32,
    run_id: String,
    created_at: String,
    updated_at: String,
    artifacts: RunArtifacts,
    request: RunRequestRecord,
    outputs: Vec<PersistedOutput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedOutput {
    id: String,
    prompt: String,
    prompt_index: usize,
    prompt_total: usize,
    variant: usize,
    total: usize,
    status: String,
    status_text: String,
    engine: String,
    model: String,
    mood: String,
    aspect_ratio: String,
    filename: String,
    output_format: String,
    #[serde(default)]
    output_path: Option<String>,
    #[serde(default)]
    image_url: Option<String>,
    #[serde(default)]
    reference_image_path: Option<String>,
    #[serde(default)]
    reference_image_url: Option<String>,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    turn_id: Option<String>,
    #[serde(default)]
    error_code: Option<String>,
    #[serde(default)]
    error_message: Option<String>,
    #[serde(default)]
    upstream_status: Option<String>,
    #[serde(default)]
    diagnostic_log: String,
    #[serde(default)]
    retry_count: u32,
    timing: JobTiming,
    created_at: String,
    updated_at: String,
}

#[derive(Default)]
struct RestoredRuntimeState {
    jobs: HashMap<String, ImageGridJob>,
    runs: HashMap<String, RunRecord>,
    normalized_run_ids: Vec<String>,
}

enum ExpectedFileState {
    Missing,
    Regular,
    Unsafe,
}

#[derive(Debug, Clone, Copy)]
struct RecoveryConfig {
    max_retries: u32,
    retry_base: Duration,
    rate_limit_cooldown: Duration,
    rate_limit_cooldown_max: Duration,
    job_timeout: Duration,
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            max_retries: APP_SERVER_IMAGE_MAX_RETRIES,
            retry_base: APP_SERVER_IMAGE_RETRY_BASE,
            rate_limit_cooldown: APP_SERVER_IMAGE_RATE_LIMIT_COOLDOWN,
            rate_limit_cooldown_max: APP_SERVER_IMAGE_RATE_LIMIT_COOLDOWN_MAX,
            job_timeout: APP_SERVER_JOB_TIMEOUT,
        }
    }
}

fn configured_app_server_image_max_retries() -> u32 {
    let value = std::env::var(APP_SERVER_IMAGE_MAX_RETRIES_ENV).ok();
    parse_app_server_image_max_retries(value.as_deref())
}

fn parse_app_server_image_max_retries(value: Option<&str>) -> u32 {
    value
        .and_then(|value| {
            let value = value.trim();
            if value.is_empty() {
                Some(0.0)
            } else {
                value.parse::<f64>().ok()
            }
        })
        .filter(|value| value.is_finite())
        .map(|value| {
            value
                .round()
                .clamp(0.0, f64::from(APP_SERVER_IMAGE_MAX_RETRIES_LIMIT)) as u32
        })
        .unwrap_or(APP_SERVER_IMAGE_MAX_RETRIES)
}

#[derive(Debug, Clone)]
struct AttemptState {
    id: String,
    index: u32,
    rate_limited: bool,
    explicit_failure: bool,
    missing_output_retryable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetryReason {
    RateLimit,
    MissingOutput,
}

#[derive(Debug)]
enum AttemptOutcome {
    Success { upstream_status: String },
    Failed,
}

struct RuntimeInner {
    config: Arc<RuntimeConfig>,
    app_server: AppServerBridge,
    jobs: RwLock<HashMap<String, ImageGridJob>>,
    runs: RwLock<HashMap<String, RunRecord>>,
    events: broadcast::Sender<RuntimeEvent>,
    image_slots: Arc<Semaphore>,
    svg_slots: Arc<Semaphore>,
    queued_jobs: AtomicUsize,
    artifact_write: Mutex<()>,
    recovery: RecoveryConfig,
    attempts: RwLock<HashMap<String, AttemptState>>,
    rate_limit_cooldown_until: Mutex<Instant>,
    admission_gate: RwLock<()>,
    accepting: AtomicBool,
    shutdown_complete: AtomicBool,
    shutdown_gate: Mutex<()>,
    shutdown_signal: watch::Sender<bool>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

#[derive(Clone)]
pub(crate) struct GenerationRuntime {
    inner: Arc<RuntimeInner>,
}

impl GenerationRuntime {
    pub(crate) fn new(config: Arc<RuntimeConfig>, app_server: AppServerBridge) -> Self {
        let recovery = RecoveryConfig {
            max_retries: configured_app_server_image_max_retries(),
            ..RecoveryConfig::default()
        };
        Self::new_with_recovery_config(config, app_server, recovery)
    }

    fn new_with_recovery_config(
        config: Arc<RuntimeConfig>,
        app_server: AppServerBridge,
        recovery: RecoveryConfig,
    ) -> Self {
        let restored = restore_persisted_runtime(&config);
        let (events, _) = broadcast::channel(1024);
        let (shutdown_signal, _) = watch::channel(false);
        let runtime = Self {
            inner: Arc::new(RuntimeInner {
                config,
                app_server,
                jobs: RwLock::new(restored.jobs),
                runs: RwLock::new(restored.runs),
                events,
                image_slots: Arc::new(Semaphore::new(MAX_RUN_JOBS)),
                svg_slots: Arc::new(Semaphore::new(CODEX_SVG_CONCURRENCY)),
                queued_jobs: AtomicUsize::new(0),
                artifact_write: Mutex::new(()),
                recovery,
                attempts: RwLock::new(HashMap::new()),
                rate_limit_cooldown_until: Mutex::new(Instant::now()),
                admission_gate: RwLock::new(()),
                accepting: AtomicBool::new(true),
                shutdown_complete: AtomicBool::new(false),
                shutdown_gate: Mutex::new(()),
                shutdown_signal,
                tasks: Mutex::new(Vec::new()),
            }),
        };
        runtime.persist_normalized_restorations(restored.normalized_run_ids);
        runtime
    }

    fn persist_normalized_restorations(&self, run_ids: Vec<String>) {
        if run_ids.is_empty() {
            return;
        }
        let runtime = self.clone();
        let worker = std::thread::Builder::new()
            .name("image-grid-startup-restoration".to_owned())
            .spawn(move || -> Result<(), String> {
                let executor = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| format!("could not start restoration executor: {error}"))?;
                executor.block_on(async move {
                    for run_id in run_ids {
                        runtime.write_artifacts(&run_id).await.map_err(|error| {
                            format!(
                                "could not persist restored artifacts for {run_id}: {}",
                                error.error
                            )
                        })?;
                    }
                    Ok(())
                })
            });
        match worker {
            Ok(worker) => match worker.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => eprintln!("image-grid-server: {error}"),
                Err(_) => eprintln!("image-grid-server: startup restoration thread panicked"),
            },
            Err(error) => {
                eprintln!("image-grid-server: could not start restoration thread: {error}")
            }
        }
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.inner.events.subscribe()
    }

    pub(crate) fn shutdown_receiver(&self) -> watch::Receiver<bool> {
        self.inner.shutdown_signal.subscribe()
    }

    pub(crate) fn is_closed(&self) -> bool {
        !self.inner.accepting.load(Ordering::Acquire)
    }

    pub(crate) async fn snapshot(&self) -> Vec<ImageGridJob> {
        let mut jobs = self
            .inner
            .jobs
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        jobs.sort_by(compare_jobs);
        jobs
    }

    pub(crate) async fn job_count(&self) -> usize {
        self.inner.jobs.read().await.len()
    }

    #[cfg(test)]
    pub(crate) async fn insert_test_job(&self, job: ImageGridJob) {
        self.inner.jobs.write().await.insert(job.id.clone(), job);
    }

    pub(crate) async fn delete_runs(
        &self,
        run_ids: &[String],
    ) -> Result<DeleteRunsResult, DeleteRunsError> {
        let _admission_guard = self.inner.admission_gate.write().await;
        let requested = run_ids.iter().cloned().collect::<HashSet<_>>();
        let mut active_run_ids = self
            .inner
            .jobs
            .read()
            .await
            .values()
            .filter(|job| requested.contains(&job.run_id) && job.is_active())
            .map(|job| job.run_id.clone())
            .collect::<Vec<_>>();
        active_run_ids.sort();
        active_run_ids.dedup();
        if !active_run_ids.is_empty() {
            return Err(DeleteRunsError::ActiveRuns(active_run_ids));
        }

        fs::create_dir_all(&self.inner.config.generated_dir)
            .await
            .map_err(DeleteRunsError::Io)?;
        let generated_root = fs::canonicalize(&self.inner.config.generated_dir)
            .await
            .map_err(DeleteRunsError::Io)?;

        let mut deleted_run_ids = Vec::new();
        let mut failures = Vec::new();
        for run_id in run_ids {
            let run_directory = generated_root.join(run_id);
            let metadata = match fs::symlink_metadata(&run_directory).await {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    deleted_run_ids.push(run_id.clone());
                    continue;
                }
                Err(error) => {
                    failures.push(DeleteRunFailure {
                        run_id: run_id.clone(),
                        error: format!("Could not inspect the run directory: {error}"),
                    });
                    continue;
                }
            };
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                failures.push(DeleteRunFailure {
                    run_id: run_id.clone(),
                    error: "The run path is not an owned directory.".to_owned(),
                });
                continue;
            }
            let canonical_run = match fs::canonicalize(&run_directory).await {
                Ok(path) => path,
                Err(error) => {
                    failures.push(DeleteRunFailure {
                        run_id: run_id.clone(),
                        error: format!("Could not resolve the run directory: {error}"),
                    });
                    continue;
                }
            };
            if canonical_run.parent() != Some(generated_root.as_path()) {
                failures.push(DeleteRunFailure {
                    run_id: run_id.clone(),
                    error: "The run directory is outside the generated image root.".to_owned(),
                });
                continue;
            }
            match fs::remove_dir_all(&canonical_run).await {
                Ok(()) => deleted_run_ids.push(run_id.clone()),
                Err(error) => failures.push(DeleteRunFailure {
                    run_id: run_id.clone(),
                    error: format!("Could not delete the run directory: {error}"),
                }),
            }
        }

        let deleted = deleted_run_ids.iter().cloned().collect::<HashSet<_>>();
        let removed_job_ids = {
            let mut jobs = self.inner.jobs.write().await;
            let removed = jobs
                .values()
                .filter(|job| deleted.contains(&job.run_id))
                .map(|job| job.id.clone())
                .collect::<HashSet<_>>();
            jobs.retain(|_, job| !deleted.contains(&job.run_id));
            removed
        };
        self.inner
            .runs
            .write()
            .await
            .retain(|run_id, _| !deleted.contains(run_id));
        self.inner
            .attempts
            .write()
            .await
            .retain(|job_id, _| !removed_job_ids.contains(job_id));

        if !deleted_run_ids.is_empty() {
            self.emit(
                "runs-deleted",
                json!({
                    "runIds": deleted_run_ids
                }),
            );
        }

        Ok(DeleteRunsResult {
            ok: failures.is_empty(),
            deleted_run_ids,
            deleted_job_count: removed_job_ids.len(),
            failures,
        })
    }

    pub(crate) fn scheduler_snapshot(&self) -> SchedulerSnapshot {
        let active = MAX_RUN_JOBS.saturating_sub(self.inner.image_slots.available_permits());
        SchedulerSnapshot {
            configured_max: MAX_RUN_JOBS,
            adaptive: false,
            current_limit: MAX_RUN_JOBS,
            active,
            queued: self.inner.queued_jobs.load(Ordering::Relaxed),
        }
    }

    pub(crate) async fn create_run(
        &self,
        body: &Value,
        require_prompts_array: bool,
        query_wait_ms: Option<&str>,
    ) -> Result<(bool, Value), RunApiError> {
        if self.is_closed() {
            return Err(runtime_closed_api_error());
        }
        let admission = self.inner.admission_gate.read().await;
        if self.is_closed() {
            return Err(runtime_closed_api_error());
        }
        let request = normalize_request(body, require_prompts_array, query_wait_ms)?;
        let run_id = Uuid::new_v4().to_string()[..8].to_owned();
        let run_directory = self.inner.config.generated_dir.join(&run_id);
        fs::create_dir_all(&run_directory).await.map_err(|error| {
            RunApiError::message(format!("could not create run directory: {error}"))
        })?;

        let reference = if let Some(inline) = request.inline_reference_image.clone() {
            let filename = format!("reference.{}", inline.extension);
            let staged_path = run_directory.join(&filename);
            atomic_write(&staged_path, &inline.bytes).await?;
            let staged_path = fs::canonicalize(&staged_path).await.map_err(|error| {
                RunApiError::message(format!("reference staging failed: {error}"))
            })?;
            Some(ReferenceRecord {
                path: display_path(&staged_path),
                url: format!("/generated/{run_id}/{filename}"),
            })
        } else if let Some(source_path) = request.reference_image_path.clone() {
            let staging_directory = run_directory.clone();
            let staged = tokio::task::spawn_blocking(move || {
                stage_reference_image(source_path, staging_directory)
            })
            .await
            .map_err(|error| RunApiError::message(format!("reference staging failed: {error}")))?
            .map_err(|error| RunApiError::message(error.to_string()))?;
            let filename = staged
                .staged_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("reference.png")
                .to_owned();
            Some(ReferenceRecord {
                path: display_path(&staged.staged_path),
                url: format!("/generated/{run_id}/{filename}"),
            })
        } else {
            None
        };

        let artifacts = run_artifacts(&self.inner.config.generated_dir, &run_id);
        let model = if request.engine == "app-server-image" {
            "app-server-image"
        } else {
            "codex-app-server"
        }
        .to_owned();
        let request_record = RunRequestRecord {
            prompts: request
                .prompts
                .iter()
                .enumerate()
                .map(|(index, prompt)| PromptRecord {
                    index: index + 1,
                    prompt: prompt.clone(),
                })
                .collect(),
            mood: request.mood.clone(),
            engine: request.engine.clone(),
            model: model.clone(),
            aspect_ratio: request.aspect_ratio.clone(),
            variants_per_prompt: request.count,
            prompt_total: request.prompts.len(),
            reference_premise: request.reference_premise.clone(),
            reference_image: reference.clone(),
        };

        let now = now_millis();
        let mut created = Vec::with_capacity(request.prompts.len() * request.count);
        for (prompt_index, prompt) in request.prompts.iter().enumerate() {
            for variant_index in 0..request.count {
                let extension = if request.engine == "codex-svg" {
                    "svg"
                } else {
                    "png"
                };
                let prompt_part = if request.prompts.len() > 1 {
                    format!("prompt-{:02}-", prompt_index + 1)
                } else {
                    String::new()
                };
                let filename = format!("{prompt_part}variant-{:02}.{extension}", variant_index + 1);
                created.push(ImageGridJob {
                    id: Uuid::new_v4().to_string(),
                    run_id: run_id.clone(),
                    engine: request.engine.clone(),
                    model: model.clone(),
                    prompt: prompt.clone(),
                    reference_premise: request.reference_premise.clone(),
                    mood: request.mood.clone(),
                    prompt_index: prompt_index + 1,
                    prompt_total: request.prompts.len(),
                    variant: variant_index + 1,
                    total: request.count,
                    filename: filename.clone(),
                    output_path: display_path(&run_directory.join(&filename)),
                    aspect_ratio: request.aspect_ratio.clone(),
                    reference_image_path: reference.as_ref().map(|value| value.path.clone()),
                    reference_image_url: reference.as_ref().map(|value| value.url.clone()),
                    manifest_path: artifacts.manifest_path.clone(),
                    manifest_url: artifacts.manifest_url.clone(),
                    manifest_view_url: artifacts.manifest_view_url.clone(),
                    handoff_path: artifacts.handoff_path.clone(),
                    handoff_url: artifacts.handoff_url.clone(),
                    handoff_view_url: artifacts.handoff_view_url.clone(),
                    output_format: extension.to_owned(),
                    status: "queued".to_owned(),
                    status_text: "Queued".to_owned(),
                    image_url: None,
                    log: String::new(),
                    thread_id: None,
                    turn_id: None,
                    error_code: None,
                    error_message: None,
                    upstream_status: None,
                    diagnostic_log: String::new(),
                    retry_count: 0,
                    timing: JobTiming::queued(now),
                    created_at: now,
                    updated_at: now,
                });
            }
        }

        let job_ids = created.iter().map(|job| job.id.clone()).collect::<Vec<_>>();
        {
            let mut jobs = self.inner.jobs.write().await;
            for job in &created {
                jobs.insert(job.id.clone(), job.clone());
            }
        }
        let run = RunRecord {
            run_id: run_id.clone(),
            job_ids,
            initial_jobs: created.clone(),
            request: request_record,
            artifacts,
            created_at: now,
            notify: Arc::new(Notify::new()),
        };
        self.inner.runs.write().await.insert(run_id.clone(), run);
        self.write_artifacts(&run_id).await?;
        self.emit(
            "run",
            json!({
                "runId": run_id,
                "jobs": created
            }),
        );

        for job in &created {
            let is_svg = job.engine == "codex-svg";
            if !is_svg {
                self.inner.queued_jobs.fetch_add(1, Ordering::Relaxed);
            }
            let runtime = self.clone();
            let job_id = job.id.clone();
            let mut shutdown = runtime.shutdown_receiver();
            let handle = tokio::spawn(async move {
                let permit = tokio::select! {
                    biased;
                    _ = wait_for_shutdown(&mut shutdown) => None,
                    permit = async {
                        if is_svg {
                            runtime.inner.svg_slots.clone().acquire_owned().await
                        } else {
                            runtime.inner.image_slots.clone().acquire_owned().await
                        }
                    } => permit.ok(),
                };
                if !is_svg {
                    runtime.inner.queued_jobs.fetch_sub(1, Ordering::Relaxed);
                }
                let Some(_permit) = permit else {
                    return;
                };
                runtime.run_job(&job_id).await;
            });
            self.inner.tasks.lock().await.push(handle);
        }
        drop(admission);

        if request.wait_ms > 0 {
            self.wait_for_run(&run_id, request.wait_ms).await;
        }
        let response = self
            .run_response(&run_id, true)
            .await
            .ok_or_else(|| RunApiError::message("run disappeared after creation"))?;
        let completed = response
            .get("completed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok((completed, response))
    }

    pub(crate) async fn run_response(&self, run_id: &str, include_jobs: bool) -> Option<Value> {
        let run = self.inner.runs.read().await.get(run_id).cloned()?;
        let jobs = self.jobs_for_run(&run).await;
        let (status, counts, completed) = run_status(&jobs);
        let diagnostics = diagnostic_outputs(&jobs);
        let server = self.server_identity_value().await;
        let outputs = jobs.iter().map(output_value).collect::<Vec<_>>();
        let mut response = json!({
            "runId": run_id,
            "status": status,
            "completed": completed,
            "counts": counts,
            "statusUrl": format!("/api/runs/{run_id}"),
            "manifestPath": run.artifacts.manifest_path,
            "manifestUrl": run.artifacts.manifest_url,
            "manifestViewUrl": run.artifacts.manifest_view_url,
            "handoffPath": run.artifacts.handoff_path,
            "handoffUrl": run.artifacts.handoff_url,
            "handoffViewUrl": run.artifacts.handoff_view_url,
            "server": server,
            "request": run.request,
            "diagnostics": diagnostics,
            "outputs": outputs
        });
        if include_jobs {
            response["jobs"] =
                serde_json::to_value(run.initial_jobs).unwrap_or_else(|_| Value::Array(Vec::new()));
        }
        Some(response)
    }

    pub(crate) async fn run_list(&self) -> Value {
        let run_ids = self
            .inner
            .runs
            .read()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut responses = Vec::with_capacity(run_ids.len());
        for run_id in run_ids {
            if let Some(response) = self.run_response(&run_id, false).await {
                responses.push(response);
            }
        }
        responses.sort_by(|left, right| {
            response_updated_at(right)
                .cmp(&response_updated_at(left))
                .then_with(|| right["runId"].as_str().cmp(&left["runId"].as_str()))
        });
        json!({ "data": responses })
    }

    pub(crate) async fn generated_files(&self) -> Result<Vec<GeneratedFile>, RunApiError> {
        let mut generated = Vec::new();
        let mut runs = fs::read_dir(&self.inner.config.generated_dir)
            .await
            .map_err(|error| {
                RunApiError::message(format!("could not list generated files: {error}"))
            })?;
        while let Some(run_entry) = runs.next_entry().await.map_err(|error| {
            RunApiError::message(format!("could not list generated files: {error}"))
        })? {
            let file_type = run_entry
                .file_type()
                .await
                .map_err(|error| RunApiError::message(format!("could not inspect run: {error}")))?;
            if !file_type.is_dir() {
                continue;
            }
            let run_id = run_entry.file_name().to_string_lossy().into_owned();
            let mut files = fs::read_dir(run_entry.path())
                .await
                .map_err(|error| RunApiError::message(format!("could not list run: {error}")))?;
            while let Some(file_entry) = files
                .next_entry()
                .await
                .map_err(|error| RunApiError::message(format!("could not list run: {error}")))?
            {
                let file_type = file_entry.file_type().await.map_err(|error| {
                    RunApiError::message(format!("could not inspect file: {error}"))
                })?;
                if !file_type.is_file() {
                    continue;
                }
                let file = file_entry.file_name().to_string_lossy().into_owned();
                let extension = Path::new(&file)
                    .extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if !matches!(extension.as_str(), "svg" | "png" | "jpg" | "jpeg" | "webp") {
                    continue;
                }
                generated.push(GeneratedFile {
                    run_id: run_id.clone(),
                    url: format!("/generated/{run_id}/{file}"),
                    file,
                });
            }
        }
        generated.reverse();
        Ok(generated)
    }

    pub(crate) fn generated_path(
        &self,
        run_id: &str,
        filename: &str,
    ) -> Result<PathBuf, RunApiError> {
        if !valid_run_id(run_id) {
            return Err(RunApiError::message("invalid run id"));
        }
        if filename.is_empty()
            || Path::new(filename)
                .file_name()
                .and_then(|name| name.to_str())
                != Some(filename)
        {
            return Err(RunApiError::message("invalid generated file"));
        }
        Ok(self.inner.config.generated_dir.join(run_id).join(filename))
    }

    pub(crate) async fn generated_file_for_job(
        &self,
        job_id: &str,
    ) -> Result<PathBuf, GeneratedJobFileError> {
        let job = self
            .inner
            .jobs
            .read()
            .await
            .get(job_id)
            .cloned()
            .ok_or(GeneratedJobFileError::JobNotFound)?;
        if job.output_path.is_empty() {
            return Err(GeneratedJobFileError::GeneratedFileNotFound);
        }

        let generated_root = absolute_lexical_path(&self.inner.config.generated_dir)
            .map_err(GeneratedJobFileError::Io)?;
        let candidate = absolute_lexical_path(Path::new(&job.output_path))
            .map_err(GeneratedJobFileError::Io)?;
        if candidate == generated_root || !candidate.starts_with(&generated_root) {
            return Err(GeneratedJobFileError::Forbidden);
        }

        let candidate_metadata = fs::symlink_metadata(&candidate).await.map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                GeneratedJobFileError::GeneratedFileNotFound
            } else {
                GeneratedJobFileError::Io(error)
            }
        })?;
        let real_generated_root = fs::canonicalize(&generated_root)
            .await
            .map_err(GeneratedJobFileError::Io)?;
        let real_file = fs::canonicalize(&candidate).await.map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                GeneratedJobFileError::GeneratedFileNotFound
            } else {
                GeneratedJobFileError::Io(error)
            }
        })?;
        if real_file == real_generated_root || !real_file.starts_with(&real_generated_root) {
            return Err(GeneratedJobFileError::Forbidden);
        }
        if candidate_metadata.file_type().is_symlink()
            || !fs::metadata(&real_file)
                .await
                .map_err(GeneratedJobFileError::Io)?
                .is_file()
        {
            return Err(GeneratedJobFileError::GeneratedFileNotFound);
        }
        Ok(real_file)
    }

    pub(crate) fn artifact_path(
        &self,
        run_id: &str,
        artifact: &str,
    ) -> Result<(PathBuf, &'static str), RunApiError> {
        if !valid_run_id(run_id) {
            return Err(RunApiError::message("invalid run id"));
        }
        match artifact {
            "manifest" => Ok((
                self.inner
                    .config
                    .generated_dir
                    .join(run_id)
                    .join("manifest.json"),
                "manifest.json",
            )),
            "handoff" => Ok((
                self.inner
                    .config
                    .generated_dir
                    .join(run_id)
                    .join("handoff.md"),
                "handoff.md",
            )),
            _ => Err(RunApiError::message("artifact not found")),
        }
    }

    async fn run_job(&self, job_id: &str) {
        let _operation = self.inner.app_server.bind_operation();
        let Some(job) = self.job(job_id).await else {
            return;
        };
        if job.engine == "codex-svg" {
            self.run_codex_svg_job(job_id).await;
            return;
        }
        self.run_app_server_image_job_with_retries(job_id).await;
    }

    async fn run_app_server_image_job_with_retries(&self, job_id: &str) {
        let recovery = self.inner.recovery;
        for attempt_index in 0..=recovery.max_retries {
            if !self.wait_for_rate_limit_cooldown(job_id).await {
                return;
            }
            if attempt_index > 0
                && let Some(job) = self.job(job_id).await
            {
                let _ = fs::remove_file(&job.output_path).await;
            }

            let Some(attempt_id) = self.begin_attempt(job_id, attempt_index).await else {
                return;
            };
            let deadline = Instant::now() + recovery.job_timeout;
            let mut shutdown = self.shutdown_receiver();
            let timed_outcome = tokio::select! {
                biased;
                _ = wait_for_shutdown(&mut shutdown) => {
                    self.retire_attempt(job_id, &attempt_id).await;
                    return;
                }
                outcome = timeout_at(
                    deadline,
                    self.run_app_server_image_attempt(job_id, &attempt_id),
                ) => outcome,
            };
            let outcome = match timed_outcome {
                Ok(outcome) => outcome,
                Err(_) => {
                    self.interrupt_timed_out_attempt(job_id, &attempt_id).await;
                    self.record_attempt_failure(
                        job_id,
                        &attempt_id,
                        "ImageGenerationTimeout",
                        &format!(
                            "Timed out waiting for app-server-image job completion after {}ms",
                            recovery.job_timeout.as_millis()
                        ),
                        None,
                    )
                    .await;
                    AttemptOutcome::Failed
                }
            };

            if let AttemptOutcome::Success { upstream_status } = outcome {
                self.finalize_attempt_success(job_id, &attempt_id, &upstream_status)
                    .await;
                self.retire_attempt(job_id, &attempt_id).await;
                return;
            }

            let retry_reason = self.retry_reason(job_id, &attempt_id).await;
            if retry_reason == Some(RetryReason::RateLimit) {
                self.start_rate_limit_cooldown(job_id, attempt_index + 1)
                    .await;
            }
            if let Some(reason) = retry_reason
                && attempt_index < recovery.max_retries
            {
                self.finish_attempt_for_retry(job_id, &attempt_id).await;
                self.retire_attempt(job_id, &attempt_id).await;
                let reason_text = match reason {
                    RetryReason::RateLimit => "rate-limit",
                    RetryReason::MissingOutput => "missing-output",
                };
                self.update_job(job_id, |job, _| {
                    job.status = "queued".to_owned();
                    job.status_text = if reason == RetryReason::MissingOutput {
                        "Queued for a bounded retry after missing image output...".to_owned()
                    } else {
                        "Queued for retry after upstream rate limit cooldown...".to_owned()
                    };
                    job.retry_count = attempt_index + 1;
                    append_job_diagnostic(
                        job,
                        format!("[retry] attempt={} reason={reason_text}", attempt_index + 1),
                    );
                })
                .await;
                continue;
            }

            self.finalize_attempt_failure(job_id, &attempt_id).await;
            self.retire_attempt(job_id, &attempt_id).await;
            return;
        }
    }

    async fn run_app_server_image_attempt(&self, job_id: &str, attempt_id: &str) -> AttemptOutcome {
        let client = match self.inner.app_server.ready_client().await {
            Ok(client) => client,
            Err(diagnostics) => {
                let error = diagnostics
                    .error
                    .unwrap_or_else(|| crate::AppServerDiagnosticError {
                        code: "AppServerUnavailable".to_owned(),
                        message: "Codex App Server is not ready".to_owned(),
                    });
                self.record_attempt_failure(job_id, attempt_id, &error.code, &error.message, None)
                    .await;
                return AttemptOutcome::Failed;
            }
        };
        let thread_result = client
            .request(
                "thread/start",
                json!({
                    "cwd": display_path(&self.inner.config.workspace_dir),
                    "approvalPolicy": "never",
                    "sandbox": "read-only",
                    "serviceName": "codex_image_grid",
                    "ephemeral": true
                }),
                APP_SERVER_REQUEST_TIMEOUT,
            )
            .await;
        let thread_result = match thread_result {
            Ok(result) => result,
            Err(error) => {
                self.record_attempt_failure(job_id, attempt_id, &error.code, &error.message, None)
                    .await;
                return AttemptOutcome::Failed;
            }
        };
        let Some(thread_id) = thread_result
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            self.record_attempt_failure(
                job_id,
                attempt_id,
                "AppServerThreadStartFailed",
                "thread/start response did not include thread.id",
                None,
            )
            .await;
            return AttemptOutcome::Failed;
        };
        let mut notifications = client.subscribe_thread(&thread_id);
        if !self
            .update_job_for_attempt(job_id, attempt_id, |job, now| {
                job.thread_id = Some(thread_id.clone());
                job.status = "running".to_owned();
                job.status_text = "Waiting for image_generation_call...".to_owned();
                job.timing.transition("running", now);
            })
            .await
        {
            return AttemptOutcome::Failed;
        }

        let Some(current_job) = self.job(job_id).await else {
            return AttemptOutcome::Failed;
        };
        let mut input = vec![json!({
            "type": "text",
            "text": build_image_prompt(&current_job),
            "text_elements": []
        })];
        if let Some(reference_path) = &current_job.reference_image_path {
            input.push(json!({
                "type": "localImage",
                "path": reference_path
            }));
        }
        let turn_result = client
            .request(
                "turn/start",
                json!({
                    "threadId": thread_id,
                    "input": input,
                    "cwd": display_path(&self.inner.config.workspace_dir),
                    "approvalPolicy": "never",
                    "sandboxPolicy": {
                        "type": "readOnly",
                        "networkAccess": false
                    },
                    "effort": "medium"
                }),
                APP_SERVER_REQUEST_TIMEOUT,
            )
            .await;
        let turn_result = match turn_result {
            Ok(result) => result,
            Err(error) => {
                self.record_attempt_failure(job_id, attempt_id, &error.code, &error.message, None)
                    .await;
                return AttemptOutcome::Failed;
            }
        };
        if let Some(turn_id) = turn_result
            .get("turn")
            .and_then(|turn| turn.get("id"))
            .and_then(Value::as_str)
        {
            let turn_id = turn_id.to_owned();
            self.update_job_for_attempt(job_id, attempt_id, |job, _| {
                job.turn_id = Some(turn_id);
            })
            .await;
        }

        let mut shutdown = self.shutdown_receiver();
        loop {
            let received = tokio::select! {
                biased;
                _ = wait_for_shutdown(&mut shutdown) => return AttemptOutcome::Failed,
                received = notifications.recv() => received,
            };
            let message = match received {
                Some(message) => message,
                None => {
                    self.record_attempt_failure(
                        job_id,
                        attempt_id,
                        "AppServerClosed",
                        "Codex App Server notification stream closed",
                        None,
                    )
                    .await;
                    return AttemptOutcome::Failed;
                }
            };
            let method = message
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match method {
                "item/completed" => {
                    let item = &message["params"]["item"];
                    if item.get("type").and_then(Value::as_str) != Some("imageGeneration") {
                        continue;
                    }
                    if let Some(result) = item.get("result").and_then(Value::as_str)
                        && let Err(error) = self.write_job_image(job_id, attempt_id, result).await
                    {
                        self.record_attempt_failure(
                            job_id,
                            attempt_id,
                            "ImageWriteFailed",
                            &error.error,
                            item.get("status").and_then(Value::as_str),
                        )
                        .await;
                        return AttemptOutcome::Failed;
                    }
                }
                // Compatibility only. Stable imageGeneration completion is the primary route.
                "rawResponseItem/completed" => {
                    let item = &message["params"]["item"];
                    if item.get("type").and_then(Value::as_str) != Some("image_generation_call") {
                        continue;
                    }
                    if let Some(result) = item.get("result").and_then(Value::as_str) {
                        if let Err(error) = self.write_job_image(job_id, attempt_id, result).await {
                            self.record_attempt_failure(
                                job_id,
                                attempt_id,
                                "ImageWriteFailed",
                                &error.error,
                                item.get("status").and_then(Value::as_str),
                            )
                            .await;
                            return AttemptOutcome::Failed;
                        }
                    } else if item
                        .get("status")
                        .and_then(Value::as_str)
                        .is_some_and(is_failure_status)
                    {
                        let (code, message) = item_error(item);
                        self.record_attempt_failure(
                            job_id,
                            attempt_id,
                            &code,
                            &message,
                            item.get("status").and_then(Value::as_str),
                        )
                        .await;
                    }
                }
                "turn/started" => {
                    if let Some(started_turn_id) = message["params"]["turn"]["id"].as_str() {
                        let started_turn_id = started_turn_id.to_owned();
                        self.update_job_for_attempt(job_id, attempt_id, |job, _| {
                            job.turn_id = Some(started_turn_id);
                        })
                        .await;
                    }
                }
                "error" => {
                    let error = &message["params"]["error"];
                    let error_message = error["message"]
                        .as_str()
                        .unwrap_or("Codex App Server reported an error");
                    let error_code = codex_error_info_code(&error["codexErrorInfo"])
                        .unwrap_or_else(|| "AppServerImageFailed".to_owned());
                    self.record_attempt_failure(
                        job_id,
                        attempt_id,
                        &error_code,
                        error_message,
                        None,
                    )
                    .await;
                    if !message["params"]["willRetry"].as_bool().unwrap_or(false) {
                        return AttemptOutcome::Failed;
                    }
                }
                "turn/completed" => {
                    let turn = &message["params"]["turn"];
                    let turn_status = turn["status"].as_str().unwrap_or("completed");
                    if self.job_output_exists_for_attempt(job_id, attempt_id).await {
                        return AttemptOutcome::Success {
                            upstream_status: turn_status.to_owned(),
                        };
                    }
                    if is_failure_status(turn_status) {
                        let (code, error_message) = turn_error(turn);
                        self.record_attempt_failure(
                            job_id,
                            attempt_id,
                            &code,
                            &error_message,
                            Some(turn_status),
                        )
                        .await;
                    } else if self
                        .attempt_allows_missing_output_retry(job_id, attempt_id)
                        .await
                    {
                        self.record_missing_output(job_id, attempt_id).await;
                    }
                    return AttemptOutcome::Failed;
                }
                "server-status"
                    if matches!(
                        message["params"]["status"].as_str(),
                        Some("error" | "stopped")
                    ) =>
                {
                    let status_message = message["params"]["message"]
                        .as_str()
                        .unwrap_or("Codex App Server stopped");
                    self.record_attempt_failure(
                        job_id,
                        attempt_id,
                        "AppServerClosed",
                        status_message,
                        None,
                    )
                    .await;
                    return AttemptOutcome::Failed;
                }
                _ => {}
            }
        }
    }

    async fn run_codex_svg_job(&self, job_id: &str) {
        self.update_job(job_id, |job, now| {
            job.status = "starting".to_owned();
            job.status_text = "Starting Codex thread...".to_owned();
            job.timing.transition("starting", now);
        })
        .await;

        let client = match self.inner.app_server.ready_client().await {
            Ok(client) => client,
            Err(diagnostics) => {
                let error = diagnostics
                    .error
                    .unwrap_or_else(|| crate::AppServerDiagnosticError {
                        code: "AppServerUnavailable".to_owned(),
                        message: "Codex App Server is not ready".to_owned(),
                    });
                self.fail_job(job_id, &error.code, &error.message).await;
                return;
            }
        };
        let thread_result = client
            .request(
                "thread/start",
                json!({
                    "cwd": display_path(&self.inner.config.workspace_dir),
                    "approvalPolicy": "never",
                    "sandbox": "workspace-write",
                    "serviceName": "codex_image_grid",
                    "ephemeral": true
                }),
                APP_SERVER_REQUEST_TIMEOUT,
            )
            .await;
        let thread_result = match thread_result {
            Ok(result) => result,
            Err(error) => {
                self.fail_job(job_id, &error.code, &error.message).await;
                return;
            }
        };
        let Some(thread_id) = thread_result
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            self.fail_job(
                job_id,
                "AppServerThreadStartFailed",
                "thread/start response did not include thread.id",
            )
            .await;
            return;
        };
        let mut notifications = client.subscribe_thread(&thread_id);
        self.update_job(job_id, |job, now| {
            job.thread_id = Some(thread_id.clone());
            job.status = "running".to_owned();
            job.status_text = "Generating...".to_owned();
            job.timing.transition("running", now);
        })
        .await;

        let Some(current_job) = self.job(job_id).await else {
            return;
        };
        let turn_result = client
            .request(
                "turn/start",
                json!({
                    "threadId": thread_id,
                    "input": [{
                        "type": "text",
                        "text": build_svg_prompt(&current_job),
                        "text_elements": []
                    }],
                    "cwd": display_path(&self.inner.config.workspace_dir),
                    "approvalPolicy": "never",
                    "sandboxPolicy": {
                        "type": "workspaceWrite",
                        "writableRoots": [display_path(&self.inner.config.data_dir)],
                        "networkAccess": false,
                        "excludeTmpdirEnvVar": false,
                        "excludeSlashTmp": false
                    },
                    "effort": "medium"
                }),
                APP_SERVER_REQUEST_TIMEOUT,
            )
            .await;
        let turn_result = match turn_result {
            Ok(result) => result,
            Err(error) => {
                self.fail_job(job_id, &error.code, &error.message).await;
                return;
            }
        };
        let turn_id = turn_result
            .get("turn")
            .and_then(|turn| turn.get("id"))
            .or_else(|| turn_result.get("id"))
            .or_else(|| turn_result.get("turnId"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(turn_id) = &turn_id {
            self.update_job(job_id, |job, _| {
                job.turn_id = Some(turn_id.clone());
            })
            .await;
        }

        let deadline = Instant::now() + APP_SERVER_JOB_TIMEOUT;
        let mut shutdown = self.shutdown_receiver();
        loop {
            let received = tokio::select! {
                biased;
                _ = wait_for_shutdown(&mut shutdown) => return,
                received = timeout_at(deadline, notifications.recv()) => received,
            };
            let message = match received {
                Ok(Some(message)) => message,
                Ok(None) => {
                    self.fail_job(
                        job_id,
                        "AppServerClosed",
                        "Codex App Server notification stream closed",
                    )
                    .await;
                    return;
                }
                Err(_) => {
                    if let Some(turn_id) = &turn_id {
                        let _ = client
                            .request(
                                "turn/interrupt",
                                json!({
                                    "threadId": thread_id,
                                    "turnId": turn_id
                                }),
                                Duration::from_secs(5),
                            )
                            .await;
                    }
                    self.fail_job(
                        job_id,
                        "CodexSvgTimeout",
                        "App Server codex-svg generation timed out",
                    )
                    .await;
                    return;
                }
            };
            let method = message
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match method {
                "item/agentMessage/delta" => {
                    let delta = message["params"]["delta"].as_str().unwrap_or_default();
                    self.update_job(job_id, |job, _| {
                        job.log.push_str(delta);
                        if job.log.len() > 2400 {
                            let mut split = job.log.len() - 2400;
                            while !job.log.is_char_boundary(split) {
                                split += 1;
                            }
                            job.log = job.log.split_off(split);
                        }
                        job.status_text = "Codex is writing the asset...".to_owned();
                    })
                    .await;
                }
                "item/completed" => {
                    let item = &message["params"]["item"];
                    match item.get("type").and_then(Value::as_str).unwrap_or_default() {
                        "commandExecution" => {
                            let detail = item
                                .get("command")
                                .and_then(Value::as_str)
                                .map(|command| format!("Command: {command}"))
                                .unwrap_or_else(|| "Command completed".to_owned());
                            self.update_job(job_id, |job, _| {
                                job.status_text = detail;
                            })
                            .await;
                        }
                        "fileChange" => {
                            self.update_job(job_id, |job, _| {
                                job.status_text = "File change detected".to_owned();
                            })
                            .await;
                        }
                        _ => {}
                    }
                }
                "turn/started" => {
                    if let Some(started_turn_id) = message["params"]["turn"]["id"].as_str() {
                        self.update_job(job_id, |job, _| {
                            job.turn_id = Some(started_turn_id.to_owned());
                            job.status_text = "Turn started".to_owned();
                        })
                        .await;
                    }
                }
                "turn/completed" => {
                    self.complete_svg_job(job_id, &message["params"]["turn"])
                        .await;
                    return;
                }
                "server-status"
                    if matches!(
                        message["params"]["status"].as_str(),
                        Some("error" | "stopped")
                    ) =>
                {
                    self.fail_job(
                        job_id,
                        "AppServerClosed",
                        message["params"]["message"]
                            .as_str()
                            .unwrap_or("Codex App Server stopped"),
                    )
                    .await;
                    return;
                }
                _ => {}
            }
        }
    }

    async fn complete_svg_job(&self, job_id: &str, turn: &Value) {
        let Some(job) = self.job(job_id).await else {
            return;
        };
        let turn_status = turn
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("completed");
        let turn_id = turn
            .get("id")
            .or_else(|| turn.get("turnId"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or(job.turn_id.clone());
        let file_exists = self.job_output_exists(job_id).await;
        let failed_upstream = is_failure_status(turn_status);
        let turn_error = turn
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str);

        if file_exists && !failed_upstream {
            let image_url = format!("/generated/{}/{}", job.run_id, job.filename);
            self.update_job(job_id, |job, now| {
                job.status = "done".to_owned();
                job.status_text = "Generated".to_owned();
                job.image_url = Some(image_url);
                job.turn_id = turn_id;
                job.error_code = None;
                job.error_message = None;
                job.upstream_status = None;
                job.timing.transition("done", now);
            })
            .await;
            return;
        }

        if file_exists {
            let image_url = format!("/generated/{}/{}", job.run_id, job.filename);
            let status_text = turn_error
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Upstream image generation {turn_status}"));
            self.update_job(job_id, |job, now| {
                job.status = "error".to_owned();
                job.status_text = status_text;
                job.image_url = Some(image_url);
                job.turn_id = turn_id;
                job.error_code = None;
                job.error_message = None;
                job.upstream_status = Some(turn_status.to_owned());
                job.diagnostic_log = format!(
                    "[turn/completed] status={turn_status}{}",
                    turn_error
                        .map(|message| format!(" message={message}"))
                        .unwrap_or_default()
                );
                job.timing.transition("error", now);
            })
            .await;
            return;
        }

        let (error_code, error_message) = if failed_upstream {
            (
                "UpstreamImageGenerationFailed",
                turn_error.unwrap_or("Upstream image generation failed"),
            )
        } else {
            (
                "ImageOutputMissing",
                "App Server turn completed without writing the requested image file",
            )
        };
        self.update_job(job_id, |job, now| {
            job.status = "error".to_owned();
            job.status_text = format!("{error_message}; No output file was written");
            job.image_url = None;
            job.turn_id = turn_id;
            job.error_code = Some(error_code.to_owned());
            job.error_message = Some(error_message.to_owned());
            job.upstream_status = failed_upstream.then(|| turn_status.to_owned());
            job.diagnostic_log = format!(
                "[turn/completed] status={turn_status} message={error_message}; \
                 [missing-output] code={error_code}"
            );
            job.timing.transition("error", now);
        })
        .await;
    }

    async fn begin_attempt(&self, job_id: &str, attempt_index: u32) -> Option<String> {
        if self.job(job_id).await.is_none() {
            return None;
        }
        let attempt_id = format!("{job_id}:{attempt_index}:{}", Uuid::new_v4());
        self.inner.attempts.write().await.insert(
            job_id.to_owned(),
            AttemptState {
                id: attempt_id.clone(),
                index: attempt_index,
                rate_limited: false,
                explicit_failure: false,
                missing_output_retryable: false,
            },
        );
        self.update_job(job_id, |job, now| {
            job.status = "starting".to_owned();
            job.status_text = "Starting App Server image thread...".to_owned();
            job.log = format!(
                "image tool via codex app-server\naspect={}\nreference={}",
                job.aspect_ratio,
                if job.reference_image_path.is_some() {
                    "attached"
                } else {
                    "none"
                }
            );
            job.thread_id = None;
            job.turn_id = None;
            job.image_url = None;
            job.timing.completed_at = None;
            job.timing.execution_ms = None;
            job.timing.total_ms = None;
            job.timing.last_attempt_completed_at = None;
            job.timing.last_attempt_ms = None;
            job.timing.transition("starting", now);
        })
        .await;
        Some(attempt_id)
    }

    async fn is_current_attempt(&self, job_id: &str, attempt_id: &str) -> bool {
        self.inner
            .attempts
            .read()
            .await
            .get(job_id)
            .is_some_and(|attempt| attempt.id == attempt_id)
    }

    async fn update_job_for_attempt<F>(&self, job_id: &str, attempt_id: &str, update: F) -> bool
    where
        F: FnOnce(&mut ImageGridJob, i64),
    {
        if !self.is_current_attempt(job_id, attempt_id).await {
            return false;
        }
        self.update_job(job_id, update).await;
        true
    }

    async fn record_attempt_failure(
        &self,
        job_id: &str,
        attempt_id: &str,
        code: &str,
        message: &str,
        upstream_status: Option<&str>,
    ) {
        if !self.is_current_attempt(job_id, attempt_id).await {
            return;
        }
        let rate_limit_code = rate_limit_code(code, message);
        {
            let mut attempts = self.inner.attempts.write().await;
            let Some(attempt) = attempts
                .get_mut(job_id)
                .filter(|attempt| attempt.id == attempt_id)
            else {
                return;
            };
            attempt.explicit_failure = true;
            if rate_limit_code.is_some() {
                attempt.rate_limited = true;
            }
        }
        let recorded_code = rate_limit_code.unwrap_or_else(|| code.to_owned());
        let recorded_message = message.to_owned();
        let upstream_status = upstream_status.map(str::to_owned);
        self.update_job_for_attempt(job_id, attempt_id, move |job, _| {
            let preserve_existing = job.error_code.as_deref().is_some_and(|existing| {
                !is_generic_error_code(existing) && is_generic_error_code(&recorded_code)
            });
            if !preserve_existing {
                job.error_code = Some(recorded_code.clone());
                job.error_message = Some(recorded_message.clone());
            }
            if let Some(status) = upstream_status {
                job.upstream_status = Some(status);
            }
            let display_code = job.error_code.as_deref().unwrap_or(&recorded_code);
            let display_message = job.error_message.as_deref().unwrap_or(&recorded_message);
            job.status_text = format!("{display_code}: {display_message}");
            append_job_diagnostic(
                job,
                format!(
                    "[upstream] code={recorded_code} message={recorded_message}{}",
                    job.upstream_status
                        .as_deref()
                        .map(|status| format!(" status={status}"))
                        .unwrap_or_default()
                ),
            );
        })
        .await;
    }

    async fn attempt_allows_missing_output_retry(&self, job_id: &str, attempt_id: &str) -> bool {
        self.inner
            .attempts
            .read()
            .await
            .get(job_id)
            .filter(|attempt| attempt.id == attempt_id)
            .is_some_and(|attempt| !attempt.rate_limited && !attempt.explicit_failure)
    }

    async fn record_missing_output(&self, job_id: &str, attempt_id: &str) {
        let attempt_index = {
            let mut attempts = self.inner.attempts.write().await;
            let Some(attempt) = attempts
                .get_mut(job_id)
                .filter(|attempt| attempt.id == attempt_id)
            else {
                return;
            };
            attempt.missing_output_retryable = true;
            attempt.index
        };
        self.update_job_for_attempt(job_id, attempt_id, move |job, _| {
            if job.error_code.is_none() || job.error_code.as_deref().is_some_and(is_generic_error_code)
            {
                job.error_code = Some("ImageOutputMissing".to_owned());
                job.error_message = Some(
                    "App Server turn completed without writing the requested image file".to_owned(),
                );
            }
            append_job_diagnostic(
                job,
                format!(
                    "[missing-output] code=ImageOutputMissing retryable=true attempt={attempt_index}"
                ),
            );
        })
        .await;
    }

    async fn retry_reason(&self, job_id: &str, attempt_id: &str) -> Option<RetryReason> {
        self.inner
            .attempts
            .read()
            .await
            .get(job_id)
            .filter(|attempt| attempt.id == attempt_id)
            .and_then(|attempt| {
                if attempt.rate_limited {
                    Some(RetryReason::RateLimit)
                } else if attempt.missing_output_retryable {
                    Some(RetryReason::MissingOutput)
                } else {
                    None
                }
            })
    }

    async fn wait_for_rate_limit_cooldown(&self, job_id: &str) -> bool {
        let started = Instant::now();
        let mut waited = false;
        let mut shutdown = self.shutdown_receiver();
        loop {
            if *shutdown.borrow() {
                return false;
            }
            let deadline = *self.inner.rate_limit_cooldown_until.lock().await;
            let now = Instant::now();
            if deadline <= now {
                break;
            }
            waited = true;
            let remaining = deadline.saturating_duration_since(now);
            self.update_job(job_id, |job, _| {
                job.status = "queued".to_owned();
                job.status_text = format!(
                    "Waiting {}s for app-server-image rate-limit cooldown...",
                    remaining.as_secs_f64().ceil() as u64
                );
            })
            .await;
            tokio::select! {
                biased;
                _ = wait_for_shutdown(&mut shutdown) => return false,
                _ = sleep(remaining) => {}
            }
        }
        if waited {
            let elapsed = started.elapsed().as_millis().min(i64::MAX as u128) as i64;
            self.update_job(job_id, |job, _| {
                job.timing.cooldown_ms = job.timing.cooldown_ms.saturating_add(elapsed);
            })
            .await;
        }
        true
    }

    async fn start_rate_limit_cooldown(&self, job_id: &str, attempt: u32) {
        let recovery = self.inner.recovery;
        let base = recovery.retry_base.max(recovery.rate_limit_cooldown);
        if base.is_zero() {
            return;
        }
        let maximum = base.max(recovery.rate_limit_cooldown_max);
        let multiplier = 2_u32.saturating_pow(attempt.saturating_sub(1));
        let delay = base.saturating_mul(multiplier).min(maximum);
        {
            let mut current = self.inner.rate_limit_cooldown_until.lock().await;
            *current = (*current).max(Instant::now() + delay);
        }
        self.update_job(job_id, |job, _| {
            append_job_diagnostic(
                job,
                format!(
                    "[rate-limit-cooldown] attempt={attempt} delayMs={}",
                    delay.as_millis()
                ),
            );
        })
        .await;
    }

    async fn finish_attempt_for_retry(&self, job_id: &str, attempt_id: &str) {
        self.update_job_for_attempt(job_id, attempt_id, |job, now| {
            let started = job.timing.current_attempt_started_at.unwrap_or(now);
            job.timing.last_attempt_completed_at = Some(now);
            job.timing.last_attempt_ms = Some(now.saturating_sub(started));
            job.timing.current_attempt_started_at = None;
        })
        .await;
    }

    async fn finalize_attempt_success(
        &self,
        job_id: &str,
        attempt_id: &str,
        upstream_status: &str,
    ) {
        let upstream_status = upstream_status.to_owned();
        self.update_job_for_attempt(job_id, attempt_id, move |job, now| {
            job.status = "done".to_owned();
            job.status_text = "Generated".to_owned();
            job.error_code = None;
            job.error_message = None;
            job.upstream_status = is_failure_status(&upstream_status).then_some(upstream_status);
            job.timing.transition("done", now);
        })
        .await;
    }

    async fn finalize_attempt_failure(&self, job_id: &str, attempt_id: &str) {
        self.update_job_for_attempt(job_id, attempt_id, |job, now| {
            job.status = "error".to_owned();
            let message = job
                .error_message
                .clone()
                .unwrap_or_else(|| "Upstream image generation failed".to_owned());
            job.status_text = append_missing_output_evidence(&format!(
                "{}: {message}",
                job.error_code
                    .as_deref()
                    .unwrap_or("UpstreamImageGenerationFailed")
            ));
            job.image_url = None;
            job.timing.transition("error", now);
        })
        .await;
    }

    async fn interrupt_timed_out_attempt(&self, job_id: &str, attempt_id: &str) {
        if !self.is_current_attempt(job_id, attempt_id).await {
            return;
        }
        let Some(job) = self.job(job_id).await else {
            return;
        };
        let (Some(thread_id), Some(turn_id), Some(client)) = (
            job.thread_id,
            job.turn_id,
            self.inner.app_server.current_client().await,
        ) else {
            return;
        };
        let result = client
            .request(
                "turn/interrupt",
                json!({ "threadId": thread_id, "turnId": turn_id }),
                Duration::from_secs(5),
            )
            .await;
        self.update_job_for_attempt(job_id, attempt_id, move |job, _| match result {
            Ok(_) => append_job_diagnostic(job, "[turn/interrupt] app-server-image timeout"),
            Err(error) => append_job_diagnostic(
                job,
                format!(
                    "[turn/interrupt-failed] app-server-image timeout: {}",
                    error.message
                ),
            ),
        })
        .await;
    }

    async fn retire_attempt(&self, job_id: &str, attempt_id: &str) {
        let mut attempts = self.inner.attempts.write().await;
        if attempts
            .get(job_id)
            .is_some_and(|attempt| attempt.id == attempt_id)
        {
            attempts.remove(job_id);
        }
    }

    async fn write_job_image(
        &self,
        job_id: &str,
        attempt_id: &str,
        encoded: &str,
    ) -> Result<(), RunApiError> {
        if !self.is_current_attempt(job_id, attempt_id).await {
            return Ok(());
        }
        let job = self
            .job(job_id)
            .await
            .ok_or_else(|| RunApiError::message("job not found"))?;
        let bytes = BASE64_STANDARD
            .decode(encoded)
            .map_err(|error| RunApiError::message(format!("invalid image result: {error}")))?;
        validate_generated_image_bytes(&bytes, &job.output_format)?;
        let output_path = PathBuf::from(&job.output_path);
        let temporary_path =
            output_path.with_extension(format!("{}.{}.tmp", job.output_format, Uuid::new_v4()));
        fs::write(&temporary_path, bytes)
            .await
            .map_err(|error| RunApiError::message(format!("could not write image: {error}")))?;
        if !self.is_current_attempt(job_id, attempt_id).await {
            let _ = fs::remove_file(&temporary_path).await;
            return Ok(());
        }
        fs::rename(&temporary_path, &output_path)
            .await
            .map_err(|error| RunApiError::message(format!("could not install image: {error}")))?;
        let image_url = format!("/generated/{}/{}", job.run_id, job.filename);
        self.update_job_for_attempt(job_id, attempt_id, |job, _| {
            job.image_url = Some(image_url);
            job.status_text = "Image generated; waiting for turn completion...".to_owned();
        })
        .await;
        Ok(())
    }

    async fn job_output_exists_for_attempt(&self, job_id: &str, attempt_id: &str) -> bool {
        if !self.is_current_attempt(job_id, attempt_id).await {
            return false;
        }
        let Some(job) = self.job(job_id).await else {
            return false;
        };
        fs::symlink_metadata(&job.output_path)
            .await
            .is_ok_and(|metadata| metadata.is_file())
    }

    async fn job_output_exists(&self, job_id: &str) -> bool {
        let Some(job) = self.job(job_id).await else {
            return false;
        };
        fs::metadata(&job.output_path)
            .await
            .is_ok_and(|metadata| metadata.is_file())
    }

    async fn job(&self, job_id: &str) -> Option<ImageGridJob> {
        self.inner.jobs.read().await.get(job_id).cloned()
    }

    async fn fail_job(&self, job_id: &str, code: &str, message: &str) {
        self.update_job(job_id, |job, now| {
            job.status = "error".to_owned();
            job.status_text = message.to_owned();
            job.error_code = Some(code.to_owned());
            job.error_message = Some(message.to_owned());
            job.diagnostic_log = format!("{code}: {message}");
            job.timing.transition("error", now);
        })
        .await;
    }

    async fn update_job<F>(&self, job_id: &str, update: F)
    where
        F: FnOnce(&mut ImageGridJob, i64),
    {
        if self.is_closed() {
            return;
        }
        let (updated, run_id, terminal) = {
            let mut jobs = self.inner.jobs.write().await;
            let Some(job) = jobs.get_mut(job_id) else {
                return;
            };
            let now = now_millis();
            update(job, now);
            job.updated_at = now;
            (job.clone(), job.run_id.clone(), job.is_terminal())
        };
        self.emit("job", serde_json::to_value(&updated).unwrap_or(Value::Null));
        self.write_artifacts_or_emit(&run_id).await;
        if terminal && let Some(run) = self.inner.runs.read().await.get(&run_id) {
            run.notify.notify_waiters();
        }
    }

    async fn jobs_for_run(&self, run: &RunRecord) -> Vec<ImageGridJob> {
        let jobs = self.inner.jobs.read().await;
        run.job_ids
            .iter()
            .filter_map(|id| jobs.get(id).cloned())
            .collect()
    }

    async fn wait_for_run(&self, run_id: &str, wait_ms: u64) {
        let Some(run) = self.inner.runs.read().await.get(run_id).cloned() else {
            return;
        };
        let deadline = Instant::now() + Duration::from_millis(wait_ms);
        loop {
            let jobs = self.jobs_for_run(&run).await;
            if jobs.iter().all(ImageGridJob::is_terminal) {
                return;
            }
            if timeout_at(deadline, run.notify.notified()).await.is_err() {
                return;
            }
        }
    }

    async fn write_artifacts(&self, run_id: &str) -> Result<(), RunApiError> {
        let _guard = self.inner.artifact_write.lock().await;
        let run = self
            .inner
            .runs
            .read()
            .await
            .get(run_id)
            .cloned()
            .ok_or_else(|| RunApiError::message("run not found"))?;
        let jobs = self.jobs_for_run(&run).await;
        let updated_at = jobs
            .iter()
            .map(|job| job.updated_at)
            .max()
            .unwrap_or(run.created_at);
        let server = self.server_identity_value().await;
        let manifest = json!({
            "schemaVersion": 1,
            "app": APP_IDENTITY,
            "runId": run.run_id,
            "createdAt": iso_time(run.created_at),
            "updatedAt": iso_time(updated_at),
            "cwd": display_path(&self.inner.config.workspace_dir),
            "server": server,
            "artifacts": run.artifacts,
            "request": run.request,
            "diagnostics": diagnostic_outputs(&jobs),
            "outputs": jobs.iter().map(output_value).collect::<Vec<_>>()
        });
        let mut manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| {
            RunApiError::message(format!("manifest serialization failed: {error}"))
        })?;
        manifest_bytes.push(b'\n');
        atomic_write(Path::new(&run.artifacts.manifest_path), &manifest_bytes).await?;
        let handoff = build_handoff(&run, &jobs, updated_at);
        atomic_write(Path::new(&run.artifacts.handoff_path), handoff.as_bytes()).await
    }

    async fn write_artifacts_or_emit(&self, run_id: &str) {
        if let Err(error) = self.write_artifacts(run_id).await {
            self.emit(
                "server-log",
                json!({
                    "stream": "artifact",
                    "text": error.error
                }),
            );
        }
    }

    async fn server_identity_value(&self) -> Value {
        let diagnostics = self.inner.app_server.diagnostics().await;
        serde_json::to_value(RuntimeIdentity::from_config(
            &self.inner.config,
            diagnostics,
            self.scheduler_snapshot(),
        ))
        .unwrap_or(Value::Null)
    }

    fn emit(&self, name: &str, data: Value) {
        let _ = self.inner.events.send(RuntimeEvent {
            name: name.to_owned(),
            data,
        });
    }

    pub(crate) async fn shutdown(&self) {
        self.inner.accepting.store(false, Ordering::Release);
        let _shutdown_guard = self.inner.shutdown_gate.lock().await;
        if self.inner.shutdown_complete.load(Ordering::Acquire) {
            return;
        }

        let _admission_guard = self.inner.admission_gate.write().await;
        self.inner.image_slots.close();
        self.inner.svg_slots.close();
        let _ = self.inner.shutdown_signal.send(true);
        self.inner.queued_jobs.store(0, Ordering::Release);
        self.inner.attempts.write().await.clear();

        let (closed_jobs, affected_runs) = {
            let mut jobs = self.inner.jobs.write().await;
            let mut closed_jobs = Vec::new();
            let mut affected_runs = HashSet::new();
            for job in jobs.values_mut().filter(|job| job.is_active()) {
                let now = now_millis();
                job.status = "error".to_owned();
                job.status_text = "Runtime stopped before generation completed".to_owned();
                job.error_code
                    .get_or_insert_with(|| "RuntimeClosed".to_owned());
                job.error_message
                    .get_or_insert_with(|| RUNTIME_CLOSED_MESSAGE.to_owned());
                job.image_url = None;
                append_job_diagnostic(job, format!("RuntimeClosed: {RUNTIME_CLOSED_MESSAGE}"));
                job.timing.transition("error", now);
                job.updated_at = now;
                affected_runs.insert(job.run_id.clone());
                closed_jobs.push(job.clone());
            }
            (closed_jobs, affected_runs.into_iter().collect::<Vec<_>>())
        };
        for job in closed_jobs {
            self.emit("job", serde_json::to_value(&job).unwrap_or(Value::Null));
            if let Some(run) = self.inner.runs.read().await.get(&job.run_id) {
                run.notify.notify_waiters();
            }
        }
        for run_id in &affected_runs {
            self.write_artifacts_or_emit(run_id).await;
        }

        self.inner.app_server.shutdown().await;
        loop {
            let handles = {
                let mut tasks = self.inner.tasks.lock().await;
                std::mem::take(&mut *tasks)
            };
            if handles.is_empty() {
                break;
            }
            for handle in handles {
                let _ = handle.await;
            }
        }
        for run_id in &affected_runs {
            self.write_artifacts_or_emit(run_id).await;
        }
        self.inner.shutdown_complete.store(true, Ordering::Release);
    }
}

fn validate_generated_image_bytes(bytes: &[u8], output_format: &str) -> Result<(), RunApiError> {
    let valid = match output_format {
        "png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpg" | "jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "webp" => {
            bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP"
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(RunApiError::message(format!(
            "invalid image result: decoded data is not a valid {output_format} image"
        )))
    }
}

fn runtime_closed_api_error() -> RunApiError {
    RunApiError::new(RUNTIME_CLOSED_MESSAGE, "RuntimeClosed")
}

async fn wait_for_shutdown(receiver: &mut watch::Receiver<bool>) {
    if *receiver.borrow() {
        return;
    }
    while receiver.changed().await.is_ok() {
        if *receiver.borrow() {
            return;
        }
    }
}

fn restore_persisted_runtime(config: &RuntimeConfig) -> RestoredRuntimeState {
    let Ok(generated_root) = std::fs::canonicalize(&config.generated_dir) else {
        return RestoredRuntimeState::default();
    };
    let Ok(entries) = std::fs::read_dir(&generated_root) else {
        return RestoredRuntimeState::default();
    };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());

    let mut restored = RestoredRuntimeState::default();
    let mut restored_job_ids = HashSet::new();
    for entry in entries {
        let Some(run_id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !valid_restored_run_id(&run_id)
            || !entry
                .file_type()
                .is_ok_and(|file_type| file_type.is_dir() && !file_type.is_symlink())
        {
            continue;
        }
        let Ok(run_directory) = std::fs::canonicalize(entry.path()) else {
            continue;
        };
        if run_directory.parent() != Some(generated_root.as_path()) {
            continue;
        }
        let Some((run, jobs, normalized)) = restore_persisted_run(config, &run_id, &run_directory)
        else {
            continue;
        };
        if jobs
            .iter()
            .any(|job| restored_job_ids.contains(job.id.as_str()))
        {
            continue;
        }
        restored_job_ids.extend(jobs.iter().map(|job| job.id.clone()));
        for job in jobs {
            restored.jobs.insert(job.id.clone(), job);
        }
        if normalized {
            restored.normalized_run_ids.push(run_id.clone());
        }
        restored.runs.insert(run_id, run);
    }
    restored
}

fn restore_persisted_run(
    config: &RuntimeConfig,
    run_id: &str,
    run_directory: &Path,
) -> Option<(RunRecord, Vec<ImageGridJob>, bool)> {
    let manifest_path = run_directory.join("manifest.json");
    if !matches!(
        expected_file_state(&manifest_path),
        ExpectedFileState::Regular
    ) {
        return None;
    }
    if matches!(
        expected_file_state(&run_directory.join("handoff.md")),
        ExpectedFileState::Unsafe
    ) {
        return None;
    }
    let manifest: PersistedRunManifest =
        serde_json::from_slice(&std::fs::read(&manifest_path).ok()?).ok()?;
    if manifest.schema_version != 1 || manifest.run_id != run_id {
        return None;
    }
    let artifacts = run_artifacts(&config.generated_dir, run_id);
    if manifest.artifacts != artifacts {
        return None;
    }
    let created_at = parse_iso_millis(&manifest.created_at)?;
    let updated_at = parse_iso_millis(&manifest.updated_at)?;
    if updated_at < created_at {
        return None;
    }
    let request = validate_restored_request(run_id, run_directory, manifest.request)?;
    if manifest.outputs.is_empty()
        || manifest.outputs.len()
            != request
                .prompts
                .len()
                .checked_mul(request.variants_per_prompt)?
    {
        return None;
    }

    let restoration_time = now_millis();
    let mut normalized = false;
    let mut jobs = Vec::with_capacity(manifest.outputs.len());
    let mut job_ids = HashSet::new();
    let mut filenames = HashSet::new();
    let mut positions = HashSet::new();
    for output in manifest.outputs {
        let (job, job_normalized) = restore_persisted_output(
            run_id,
            run_directory,
            &request,
            &artifacts,
            output,
            restoration_time,
        )?;
        if !job_ids.insert(job.id.clone())
            || !filenames.insert(job.filename.clone())
            || !positions.insert((job.prompt_index, job.variant))
        {
            return None;
        }
        normalized |= job_normalized;
        jobs.push(job);
    }
    jobs.sort_by(compare_jobs);
    let job_ids = jobs.iter().map(|job| job.id.clone()).collect::<Vec<_>>();
    let run = RunRecord {
        run_id: run_id.to_owned(),
        job_ids,
        initial_jobs: jobs.clone(),
        request,
        artifacts,
        created_at,
        notify: Arc::new(Notify::new()),
    };
    Some((run, jobs, normalized))
}

fn validate_restored_request(
    run_id: &str,
    run_directory: &Path,
    mut request: RunRequestRecord,
) -> Option<RunRequestRecord> {
    if request.prompts.is_empty()
        || request.prompts.len() > MAX_PROMPTS
        || request.prompt_total != request.prompts.len()
        || !(1..=usize::from(MAX_VARIANTS_PER_PROMPT)).contains(&request.variants_per_prompt)
        || request
            .prompts
            .len()
            .checked_mul(request.variants_per_prompt)?
            > MAX_RUN_JOBS
        || !matches!(request.engine.as_str(), "app-server-image" | "codex-svg")
        || request.model.is_empty()
        || !MOODS.contains(&request.mood.as_str())
        || !ASPECT_RATIOS.contains(&request.aspect_ratio.as_str())
    {
        return None;
    }
    for (index, prompt) in request.prompts.iter().enumerate() {
        if prompt.index != index + 1 || prompt.prompt.trim().is_empty() {
            return None;
        }
    }
    request.reference_image = match request.reference_image {
        Some(reference) => Some(validate_restored_reference(
            run_id,
            run_directory,
            reference,
        )?),
        None => None,
    };
    Some(request)
}

fn validate_restored_reference(
    run_id: &str,
    run_directory: &Path,
    reference: ReferenceRecord,
) -> Option<ReferenceRecord> {
    let url_prefix = format!("/generated/{run_id}/");
    let url_filename = reference.url.strip_prefix(&url_prefix)?;
    let filename = safe_filename(url_filename)?;
    if !matches!(
        Path::new(filename)
            .extension()
            .and_then(|extension| extension.to_str()),
        Some("png" | "jpg" | "jpeg" | "webp")
    ) {
        return None;
    }
    let expected_path = run_directory.join(filename);
    if Path::new(&reference.path) != expected_path
        || matches!(
            expected_file_state(&expected_path),
            ExpectedFileState::Unsafe
        )
    {
        return None;
    }
    Some(ReferenceRecord {
        path: display_path(&expected_path),
        url: format!("{url_prefix}{filename}"),
    })
}

fn restore_persisted_output(
    run_id: &str,
    run_directory: &Path,
    request: &RunRequestRecord,
    artifacts: &RunArtifacts,
    output: PersistedOutput,
    restoration_time: i64,
) -> Option<(ImageGridJob, bool)> {
    if output.id.is_empty()
        || !matches!(
            output.status.as_str(),
            "queued" | "starting" | "running" | "done" | "error"
        )
        || output.timing.phase != output.status
        || output.prompt_index == 0
        || output.prompt_index > request.prompts.len()
        || output.prompt_total != request.prompt_total
        || output.variant == 0
        || output.variant > request.variants_per_prompt
        || output.total != request.variants_per_prompt
        || output.engine != request.engine
        || output.model != request.model
        || output.mood != request.mood
        || output.aspect_ratio != request.aspect_ratio
        || output.prompt != request.prompts[output.prompt_index - 1].prompt
    {
        return None;
    }

    let filename = safe_filename(&output.filename)?;
    let extension = Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())?;
    let extension_matches_engine = if output.engine == "codex-svg" {
        extension == "svg"
    } else {
        matches!(extension, "png" | "jpg" | "jpeg" | "webp")
    };
    if !extension_matches_engine || output.output_format != extension {
        return None;
    }
    let expected_output_path = run_directory.join(filename);
    if output
        .output_path
        .as_deref()
        .is_some_and(|path| Path::new(path) != expected_output_path)
    {
        return None;
    }
    let expected_image_url = format!("/generated/{run_id}/{filename}");
    if output
        .image_url
        .as_deref()
        .is_some_and(|url| url != expected_image_url)
    {
        return None;
    }

    let expected_reference_path = request
        .reference_image
        .as_ref()
        .map(|reference| reference.path.as_str());
    let expected_reference_url = request
        .reference_image
        .as_ref()
        .map(|reference| reference.url.as_str());
    if output.reference_image_path.as_deref() != expected_reference_path
        || output.reference_image_url.as_deref() != expected_reference_url
    {
        return None;
    }

    let created_at = parse_iso_millis(&output.created_at)?;
    let mut updated_at = parse_iso_millis(&output.updated_at)?;
    if updated_at < created_at {
        return None;
    }
    let file_state = expected_file_state(&expected_output_path);
    if matches!(file_state, ExpectedFileState::Unsafe) {
        return None;
    }
    let file_exists = matches!(file_state, ExpectedFileState::Regular);
    let was_active = matches!(output.status.as_str(), "queued" | "starting" | "running");
    let claimed_done_without_file = output.status == "done" && !file_exists;
    let mut normalized = was_active || claimed_done_without_file || output.output_path.is_none();

    let mut status = output.status;
    let mut status_text = output.status_text;
    let mut image_url = output.image_url;
    let error_code = output.error_code;
    let mut error_message = output.error_message;
    let mut timing = output.timing;
    if was_active {
        if file_exists {
            status = "done".to_owned();
            status_text = "Restored after restart".to_owned();
            image_url = Some(expected_image_url.clone());
        } else {
            status = "error".to_owned();
            status_text = "Interrupted by restart".to_owned();
            image_url = None;
            error_message = Some("Interrupted by restart".to_owned());
        }
        timing.transition(&status, restoration_time);
        updated_at = restoration_time;
    } else if claimed_done_without_file {
        status = "error".to_owned();
        status_text = "Interrupted by restart".to_owned();
        image_url = None;
        error_message = Some("Interrupted by restart".to_owned());
        timing.transition("error", restoration_time);
        updated_at = restoration_time;
    } else if status == "done" && image_url.is_none() {
        image_url = Some(expected_image_url.clone());
        normalized = true;
    } else if status == "error" && !file_exists && image_url.is_some() {
        image_url = None;
        normalized = true;
    }

    Some((
        ImageGridJob {
            id: output.id,
            run_id: run_id.to_owned(),
            engine: output.engine,
            model: output.model,
            prompt: output.prompt,
            reference_premise: request.reference_premise.clone(),
            mood: output.mood,
            prompt_index: output.prompt_index,
            prompt_total: output.prompt_total,
            variant: output.variant,
            total: output.total,
            filename: filename.to_owned(),
            output_path: display_path(&expected_output_path),
            aspect_ratio: output.aspect_ratio,
            reference_image_path: expected_reference_path.map(str::to_owned),
            reference_image_url: expected_reference_url.map(str::to_owned),
            manifest_path: artifacts.manifest_path.clone(),
            manifest_url: artifacts.manifest_url.clone(),
            manifest_view_url: artifacts.manifest_view_url.clone(),
            handoff_path: artifacts.handoff_path.clone(),
            handoff_url: artifacts.handoff_url.clone(),
            handoff_view_url: artifacts.handoff_view_url.clone(),
            output_format: output.output_format,
            status,
            status_text,
            image_url,
            log: String::new(),
            thread_id: output.thread_id,
            turn_id: output.turn_id,
            error_code,
            error_message,
            upstream_status: output.upstream_status,
            diagnostic_log: output.diagnostic_log,
            retry_count: output.retry_count,
            timing,
            created_at,
            updated_at,
        },
        normalized,
    ))
}

fn valid_restored_run_id(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f' | b'-'))
}

fn safe_filename(value: &str) -> Option<&str> {
    if value.is_empty()
        || value.contains('/')
        || value.contains('\\')
        || Path::new(value)
            .file_name()
            .and_then(|filename| filename.to_str())
            != Some(value)
        || matches!(value, "." | ".." | "manifest.json" | "handoff.md")
    {
        return None;
    }
    Some(value)
}

fn absolute_lexical_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(std::path::MAIN_SEPARATOR.to_string()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::Normal(value) => normalized.push(value),
        }
    }
    Ok(normalized)
}

fn expected_file_state(path: &Path) -> ExpectedFileState {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => ExpectedFileState::Regular,
        Ok(_) => ExpectedFileState::Unsafe,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ExpectedFileState::Missing,
        Err(_) => ExpectedFileState::Unsafe,
    }
}

fn normalize_request(
    body: &Value,
    require_prompts_array: bool,
    query_wait_ms: Option<&str>,
) -> Result<NormalizedRunRequest, RunApiError> {
    let prompts_value = body.get("prompts");
    let prompts = if require_prompts_array {
        let prompts = prompts_value
            .and_then(Value::as_array)
            .ok_or_else(|| RunApiError::new("prompts array is required", "prompts_not_array"))?;
        normalize_prompts(prompts)?
    } else if let Some(prompts) = prompts_value.and_then(Value::as_array) {
        normalize_prompts(prompts)?
    } else {
        let prompt = body
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned();
        if prompt.is_empty() {
            return Err(RunApiError::new(
                "at least one prompt is required",
                "prompts_required",
            ));
        }
        vec![prompt]
    };
    if prompts.len() > MAX_PROMPTS {
        return Err(RunApiError::new(
            format!("prompt batch is limited to {MAX_PROMPTS} prompts"),
            "too_many_prompts",
        ));
    }

    let count = match body.get("count") {
        None => 1,
        Some(value) => {
            let Some(count) = strict_usize(value) else {
                return Err(RunApiError::new(
                    "count must be an integer",
                    "count_not_integer",
                ));
            };
            count
        }
    };
    if !(1..=usize::from(MAX_VARIANTS_PER_PROMPT)).contains(&count) {
        return Err(RunApiError::new(
            format!("count must be between 1 and {MAX_VARIANTS_PER_PROMPT}"),
            "count_out_of_range",
        ));
    }
    if prompts.len() * count > MAX_RUN_JOBS {
        return Err(RunApiError::new(
            format!("a run is limited to {MAX_RUN_JOBS} total jobs"),
            "too_many_jobs",
        ));
    }

    let mood = body
        .get("mood")
        .and_then(Value::as_str)
        .filter(|value| MOODS.contains(value))
        .unwrap_or("warm-mascot")
        .to_owned();
    let engine = if body.get("engine").and_then(Value::as_str) == Some("codex-svg") {
        "codex-svg"
    } else {
        "app-server-image"
    }
    .to_owned();
    let aspect_ratio = body
        .get("aspectRatio")
        .and_then(Value::as_str)
        .filter(|value| ASPECT_RATIOS.contains(value))
        .unwrap_or("16:9")
        .to_owned();
    let reference_premise = body
        .get("referencePremise")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    let inline_reference_image = normalize_inline_reference_image(body.get("referenceImage"))?;
    let reference_image_path = inline_reference_image.is_none().then(|| {
        body.get("referenceImagePath")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    });
    let reference_image_path = reference_image_path.flatten();
    let wait_ms = normalize_wait_ms(body.get("waitMs"), query_wait_ms);

    Ok(NormalizedRunRequest {
        prompts,
        count,
        mood,
        engine,
        aspect_ratio,
        reference_premise,
        inline_reference_image,
        reference_image_path,
        wait_ms,
    })
}

fn normalize_inline_reference_image(
    reference_image: Option<&Value>,
) -> Result<Option<InlineReferenceImage>, RunApiError> {
    let Some(reference_image) = reference_image else {
        return Ok(None);
    };
    let Some(data_url_value) = reference_image.get("dataUrl") else {
        return Ok(None);
    };
    if !json_value_is_truthy(data_url_value) {
        return Ok(None);
    }
    let data_url = data_url_value
        .as_str()
        .ok_or_else(invalid_reference_image)?;
    let (mime_type, encoded, extension) =
        if let Some(encoded) = data_url.strip_prefix("data:image/png;base64,") {
            ("image/png", encoded, "png")
        } else if let Some(encoded) = data_url.strip_prefix("data:image/jpeg;base64,") {
            ("image/jpeg", encoded, "jpg")
        } else if let Some(encoded) = data_url.strip_prefix("data:image/webp;base64,") {
            ("image/webp", encoded, "webp")
        } else {
            return Err(invalid_reference_image());
        };
    if encoded.is_empty()
        || !encoded
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
    {
        return Err(invalid_reference_image());
    }

    let declared_mime_type = reference_image
        .get("mimeType")
        .and_then(Value::as_str)
        .filter(|value| matches!(*value, "image/png" | "image/jpeg" | "image/webp"));
    if declared_mime_type.is_some_and(|declared| declared != mime_type) {
        return Err(invalid_reference_image());
    }

    let bytes = BASE64_STANDARD
        .decode(encoded)
        .map_err(|_| invalid_reference_image())?;
    if bytes.len() as u64 > MAX_REFERENCE_IMAGE_BYTES {
        return Err(RunApiError::message(
            "reference image is too large; keep it under 100 MB",
        ));
    }
    Ok(Some(InlineReferenceImage { bytes, extension }))
}

fn json_value_is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn invalid_reference_image() -> RunApiError {
    RunApiError::message("reference image must be PNG, JPEG, or WebP")
}

fn normalize_prompts(values: &[Value]) -> Result<Vec<String>, RunApiError> {
    if values.is_empty() {
        return Err(RunApiError::new(
            "prompts array must contain at least one prompt",
            "prompts_required",
        ));
    }
    let mut prompts = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let prompt = value.as_str().ok_or_else(|| {
            RunApiError::new(
                format!("prompt {} must be a string", index + 1),
                "prompt_not_string",
            )
        })?;
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err(RunApiError::new(
                format!("prompt {} must not be empty", index + 1),
                "prompt_empty",
            ));
        }
        prompts.push(prompt.to_owned());
    }
    Ok(prompts)
}

fn strict_usize(value: &Value) -> Option<usize> {
    value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .or_else(|| value.as_i64().and_then(|value| usize::try_from(value).ok()))
}

fn normalize_wait_ms(body_value: Option<&Value>, query_value: Option<&str>) -> u64 {
    let numeric = body_value
        .and_then(Value::as_f64)
        .or_else(|| query_value.and_then(|value| value.parse::<f64>().ok()))
        .filter(|value| value.is_finite())
        .unwrap_or(0.0)
        .round()
        .clamp(0.0, MAX_WAIT_MS as f64);
    numeric as u64
}

fn run_artifacts(generated_dir: &Path, run_id: &str) -> RunArtifacts {
    let run_dir = generated_dir.join(run_id);
    RunArtifacts {
        manifest_path: display_path(&run_dir.join("manifest.json")),
        manifest_url: format!("/generated/{run_id}/manifest.json"),
        manifest_view_url: format!("/artifacts/{run_id}/manifest"),
        handoff_path: display_path(&run_dir.join("handoff.md")),
        handoff_url: format!("/generated/{run_id}/handoff.md"),
        handoff_view_url: format!("/artifacts/{run_id}/handoff"),
    }
}

fn run_status(jobs: &[ImageGridJob]) -> (&'static str, Value, bool) {
    let done = jobs.iter().filter(|job| job.status == "done").count();
    let running = jobs.iter().filter(|job| job.is_active()).count();
    let failed = jobs.iter().filter(|job| job.status == "error").count();
    let status = if running > 0 {
        "running"
    } else if failed > 0 {
        "error"
    } else if done == jobs.len() {
        "done"
    } else {
        "unknown"
    };
    (
        status,
        json!({
            "total": jobs.len(),
            "done": done,
            "running": running,
            "failed": failed
        }),
        running == 0,
    )
}

fn diagnostic_outputs(jobs: &[ImageGridJob]) -> Vec<Value> {
    jobs.iter()
        .filter(|job| {
            job.error_code.is_some()
                || job.error_message.is_some()
                || job
                    .upstream_status
                    .as_deref()
                    .is_some_and(is_failure_status)
                || !job.diagnostic_log.is_empty()
        })
        .map(|job| {
            json!({
                "id": job.id,
                "promptIndex": job.prompt_index,
                "variant": job.variant,
                "status": job.status,
                "statusText": job.status_text,
                "errorCode": job.error_code,
                "errorMessage": job.error_message,
                "upstreamStatus": job.upstream_status,
                "threadId": job.thread_id,
                "turnId": job.turn_id,
                "diagnosticLog": job.diagnostic_log
            })
        })
        .collect()
}

fn output_value(job: &ImageGridJob) -> Value {
    json!({
        "id": job.id,
        "prompt": job.prompt,
        "promptIndex": job.prompt_index,
        "promptTotal": job.prompt_total,
        "variant": job.variant,
        "total": job.total,
        "status": job.status,
        "statusText": job.status_text,
        "engine": job.engine,
        "model": job.model,
        "mood": job.mood,
        "aspectRatio": job.aspect_ratio,
        "filename": job.filename,
        "outputFormat": job.output_format,
        "outputPath": job.output_path,
        "imageUrl": job.image_url,
        "referenceImagePath": job.reference_image_path,
        "referenceImageUrl": job.reference_image_url,
        "threadId": job.thread_id,
        "turnId": job.turn_id,
        "errorCode": job.error_code,
        "errorMessage": job.error_message,
        "upstreamStatus": job.upstream_status,
        "diagnosticLog": job.diagnostic_log,
        "retryCount": job.retry_count,
        "timing": job.timing,
        "createdAt": iso_time(job.created_at),
        "updatedAt": iso_time(job.updated_at)
    })
}

fn build_handoff(run: &RunRecord, jobs: &[ImageGridJob], updated_at: i64) -> String {
    let done = jobs.iter().filter(|job| job.status == "done").count();
    let running = jobs.iter().filter(|job| job.is_active()).count();
    let failed = jobs.iter().filter(|job| job.status == "error").count();
    let mut lines = vec![
        "# Codex Image Grid Handoff".to_owned(),
        String::new(),
        format!("- Run ID: {}", run.run_id),
        format!("- Created: {}", iso_time(run.created_at)),
        format!("- Updated: {}", iso_time(updated_at)),
        format!("- Status: {done} done / {running} running / {failed} failed"),
        format!("- Manifest: {}", run.artifacts.manifest_path),
        format!("- Handoff: {}", run.artifacts.handoff_path),
        String::new(),
        "## Request".to_owned(),
        String::new(),
        format!("- Engine: {}", run.request.engine),
        format!("- Mood: {}", run.request.mood),
        format!("- Aspect ratio: {}", run.request.aspect_ratio),
        format!("- Variants per prompt: {}", run.request.variants_per_prompt),
        format!(
            "- Reference image: {}",
            run.request
                .reference_image
                .as_ref()
                .map(|value| value.path.as_str())
                .unwrap_or("none")
        ),
        String::new(),
        "## Diagnostics".to_owned(),
        String::new(),
    ];
    let diagnostic_jobs = jobs
        .iter()
        .filter(|job| {
            job.error_code.is_some()
                || job.error_message.is_some()
                || !job.diagnostic_log.is_empty()
        })
        .collect::<Vec<_>>();
    if diagnostic_jobs.is_empty() {
        lines.push("- none".to_owned());
    } else {
        for job in diagnostic_jobs {
            lines.push(format!(
                "- Prompt {} variant {}: {} - {}",
                job.prompt_index,
                job.variant,
                job.error_code.as_deref().unwrap_or("diagnostic"),
                job.error_message
                    .as_deref()
                    .unwrap_or(job.status_text.as_str())
            ));
            if job.thread_id.is_some() || job.turn_id.is_some() {
                lines.push(format!(
                    "  - Thread/turn: {} / {}",
                    job.thread_id.as_deref().unwrap_or("unknown"),
                    job.turn_id.as_deref().unwrap_or("unknown")
                ));
            }
            if !job.diagnostic_log.is_empty() {
                lines.push(format!("  - Log: {}", job.diagnostic_log));
            }
        }
    }
    lines.extend([
        String::new(),
        "### Reference Premise".to_owned(),
        String::new(),
        if run.request.reference_premise.is_empty() {
            "none".to_owned()
        } else {
            run.request.reference_premise.clone()
        },
        String::new(),
        "### Prompts".to_owned(),
        String::new(),
    ]);
    for prompt in &run.request.prompts {
        lines.push(format!("{}. {}", prompt.index, prompt.prompt));
        lines.push(String::new());
    }
    lines.push("## Outputs".to_owned());
    lines.push(String::new());
    for output in jobs {
        lines.extend([
            format!(
                "### Prompt {}/{} - Variant {}/{}",
                output.prompt_index, output.prompt_total, output.variant, output.total
            ),
            String::new(),
            format!("- Status: {} ({})", output.status, output.status_text),
            format!("- File: {}", output.output_path),
            format!(
                "- Browser URL: {}",
                output.image_url.as_deref().unwrap_or("not written yet")
            ),
        ]);
        if let Some(code) = &output.error_code {
            lines.push(format!("- Error code: {code}"));
        }
        if let Some(message) = &output.error_message {
            lines.push(format!("- Error message: {message}"));
        }
        if let Some(upstream_status) = &output.upstream_status {
            lines.push(format!("- Upstream status: {upstream_status}"));
        }
        if output.thread_id.is_some() || output.turn_id.is_some() {
            lines.push(format!(
                "- Thread/turn: {} / {}",
                output.thread_id.as_deref().unwrap_or("unknown"),
                output.turn_id.as_deref().unwrap_or("unknown")
            ));
        }
        if !output.diagnostic_log.is_empty() {
            lines.push(format!("- Diagnostic log: {}", output.diagnostic_log));
        }
        lines.push(format!("- Prompt: {}", output.prompt));
        lines.push(String::new());
    }
    format!("{}\n", lines.join("\n").trim_end())
}

async fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), RunApiError> {
    let parent = path
        .parent()
        .ok_or_else(|| RunApiError::message("artifact path has no parent directory"))?;
    fs::create_dir_all(parent).await.map_err(|error| {
        RunApiError::message(format!("could not create artifact directory: {error}"))
    })?;
    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("artifact");
    let temporary_path = parent.join(format!(".{filename}.{}.tmp", Uuid::new_v4()));
    fs::write(&temporary_path, bytes)
        .await
        .map_err(|error| RunApiError::message(format!("could not write artifact: {error}")))?;
    fs::rename(&temporary_path, path)
        .await
        .map_err(|error| RunApiError::message(format!("could not install artifact: {error}")))
}

fn build_image_prompt(job: &ImageGridJob) -> String {
    let mood = mood_direction(&job.mood);
    let reference = if job.reference_image_path.is_some() {
        "Use the attached local reference image as the visual identity anchor for the character, while creating a fresh composition."
    } else {
        "No reference image is attached."
    };
    let premise = if job.reference_premise.is_empty() {
        "No analyzed reference premise is provided."
    } else {
        &job.reference_premise
    };
    let prompt_label = if job.prompt_total > 1 {
        format!(
            "Prompt {} of {}; Variant {} of {}",
            job.prompt_index, job.prompt_total, job.variant, job.total
        )
    } else {
        format!("Variant {} of {}", job.variant, job.total)
    };
    format!(
        "Use the image generation tool to create exactly one polished thumbnail image.\n\n\
Reference premise:\n{premise}\n\n\
User concept:\n{}\n\n\
Overall mood:\n{mood}\n\n\
Reference image:\n{reference}\n\n\
Batch position:\n{prompt_label}\n\n\
Composition requirements:\n\
- Make this variant visibly distinct from sibling variants: vary layout, focal object, accent color, camera distance, and typographic rhythm.\n\
- This is for a thumbnail grid. Use a strong focal hierarchy and leave safe margins for cropping.\n\
- Target aspect ratio: {}.\n\
- Preserve the reference character's recognizable traits when a reference image is attached.\n\
- If using Japanese or English text, keep it short, readable, and non-overlapping.\n\
- Avoid generic stock-photo blandness and avoid scammy information-product styling.\n\
- Do not create SVG or HTML. Generate the image through the image generation tool.\n\
- Do not ask follow-up questions.\n",
        job.prompt, job.aspect_ratio
    )
}

fn build_svg_prompt(job: &ImageGridJob) -> String {
    let mood = mood_direction(&job.mood);
    let premise = if job.reference_premise.is_empty() {
        "No analyzed reference premise is provided."
    } else {
        &job.reference_premise
    };
    let prompt_label = if job.prompt_total > 1 {
        format!(
            "Prompt {} of {}; Variant {} of {}",
            job.prompt_index, job.prompt_total, job.variant, job.total
        )
    } else {
        format!("Variant {} of {}", job.variant, job.total)
    };
    format!(
        "Create one polished thumbnail-like image as a self-contained SVG file.\n\n\
Write the SVG to this exact absolute path:\n{}\n\n\
User concept:\n{}\n\n\
Reference premise:\n{premise}\n\n\
Style direction:\n{mood}\n\n\
Batch position:\n{prompt_label}\n\n\
Hard requirements:\n\
- Create exactly one valid SVG file at the path above.\n\
- Use a 16:9 canvas: viewBox=\"0 0 1280 720\".\n\
- Keep all assets inline. Do not use remote images, external fonts, or network downloads.\n\
- Make the composition visibly different from the other variants: vary layout, focal object, accent color, and typographic rhythm.\n\
- Use readable Japanese text only when useful; keep it short and non-overlapping.\n\
- Include subtle texture, depth, and a clear focal hierarchy.\n\
- Do not ask follow-up questions.\n\
- Finish with one concise sentence that includes \"{}\".\n",
        job.output_path, job.prompt, job.filename
    )
}

fn mood_direction(mood: &str) -> &'static str {
    match mood {
        "clean-thumbnail" => {
            "clean Japanese thumbnail, crisp focal subject, readable composition, bright but restrained accents, creator-friendly polish"
        }
        "editorial-soft" => {
            "soft editorial illustration, calm magazine-like composition, subtle depth, refined color, quiet premium atmosphere"
        }
        "cinematic" => {
            "cinematic lighting, dramatic but tasteful contrast, strong focal hierarchy, immersive atmosphere, expressive framing"
        }
        "minimal-product" => {
            "minimal product-style composition, generous negative space, precise lighting, simple background, elegant presentation"
        }
        _ => {
            "warm anime blog mascot portrait, soft desk lighting, charming and wholesome, gentle expression, polished illustration"
        }
    }
}

fn append_job_diagnostic(job: &mut ImageGridJob, entry: impl Into<String>) {
    let entry = entry.into();
    let combined = if job.diagnostic_log.is_empty() {
        entry
    } else {
        format!("{}\n{entry}", job.diagnostic_log)
    };
    let reversed = combined.chars().rev().take(2400).collect::<String>();
    job.diagnostic_log = reversed.chars().rev().collect();
}

fn rate_limit_code(code: &str, message: &str) -> Option<String> {
    let combined = format!("{code} {message}").to_ascii_lowercase();
    if combined.contains("toomanyrequests")
        || combined.contains("too many requests")
        || combined
            .split(|character: char| !character.is_ascii_digit())
            .any(|part| part == "429")
    {
        return Some("TooManyRequests".to_owned());
    }
    if combined.contains("rate_limit")
        || combined.contains("rate-limit")
        || combined.contains("rate limit")
        || combined.contains("ratelimit")
        || combined.contains("usagelimitexceeded")
    {
        return Some("RateLimit".to_owned());
    }
    None
}

fn is_generic_error_code(code: &str) -> bool {
    matches!(
        code,
        "UpstreamImageGenerationFailed"
            | "AppServerImageFailed"
            | "AppServerImageRunnerError"
            | "AppServerClosed"
            | "ImageGenerationTimeout"
            | "ImageWriteFailed"
            | "ImageOutputMissing"
    )
}

fn codex_error_info_code(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned).or_else(|| {
        value
            .as_object()
            .and_then(|object| object.keys().next().cloned())
    })
}

fn item_error(item: &Value) -> (String, String) {
    let error = item
        .get("error")
        .or_else(|| {
            item.get("status_details")
                .and_then(|value| value.get("error"))
        })
        .or_else(|| {
            item.get("statusDetails")
                .and_then(|value| value.get("error"))
        });
    let code = error
        .and_then(|value| value.get("code").or_else(|| value.get("type")))
        .and_then(Value::as_str)
        .unwrap_or("UpstreamImageGenerationFailed")
        .to_owned();
    let message = error
        .and_then(|value| value.get("message"))
        .and_then(Value::as_str)
        .or_else(|| item.get("message").and_then(Value::as_str))
        .unwrap_or("Upstream image generation failed")
        .to_owned();
    (code, message)
}

fn turn_error(turn: &Value) -> (String, String) {
    let error = &turn["error"];
    let code = codex_error_info_code(&error["codexErrorInfo"])
        .or_else(|| error["code"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "UpstreamImageGenerationFailed".to_owned());
    let message = error["message"]
        .as_str()
        .unwrap_or("Upstream image generation failed")
        .to_owned();
    (code, message)
}

fn append_missing_output_evidence(status_text: &str) -> String {
    if status_text.contains("No output file was written") {
        status_text.to_owned()
    } else if status_text.is_empty() {
        "No output file was written".to_owned()
    } else {
        format!("{status_text}; No output file was written")
    }
}

fn is_failure_status(status: &str) -> bool {
    matches!(
        status.to_ascii_lowercase().as_str(),
        "failed" | "error" | "cancelled" | "canceled" | "interrupted"
    )
}

fn compare_jobs(left: &ImageGridJob, right: &ImageGridJob) -> std::cmp::Ordering {
    left.run_id
        .cmp(&right.run_id)
        .then_with(|| left.prompt_index.cmp(&right.prompt_index))
        .then_with(|| left.variant.cmp(&right.variant))
}

fn response_updated_at(response: &Value) -> String {
    response["outputs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|output| output["updatedAt"].as_str())
        .max()
        .unwrap_or_default()
        .to_owned()
}

pub(crate) fn valid_run_id(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

pub(crate) fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "json" => "application/json; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        _ => "application/octet-stream",
    }
}

pub(crate) fn render_artifact_page(run_id: &str, label: &str, content: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{label}</title>\
<style>body{{font:14px -apple-system,sans-serif;margin:0;background:#1c1c1e;color:#f5f5f7}}\
main{{max-width:1100px;margin:auto;padding:32px}}pre{{white-space:pre-wrap;overflow-wrap:anywhere;\
padding:18px;border-radius:8px;background:#2c2c2e}}</style></head><body><main>\
<h1>{label}</h1><p>Run {run_id}</p><pre>{content}</pre></main></body></html>",
        label = escape_html(label),
        run_id = escape_html(run_id),
        content = escape_html(content)
    )
}

pub(crate) fn render_image_page(run_id: &str, filename: &str, image_url: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{filename}</title>\
<style>body{{font:14px -apple-system,sans-serif;margin:0;background:#1c1c1e;color:#f5f5f7}}\
main{{padding:32px}}img{{max-width:100%;max-height:calc(100vh - 140px);object-fit:contain}}</style>\
</head><body><main><h1>{filename}</h1><p>Run {run_id}</p><img src=\"{image_url}\" \
alt=\"{filename}\"></main></body></html>",
        filename = escape_html(filename),
        run_id = escape_html(run_id),
        image_url = escape_html(image_url)
    )
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn now_millis() -> i64 {
    let value = OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn iso_time(milliseconds: i64) -> String {
    let Ok(value) = OffsetDateTime::from_unix_timestamp_nanos(i128::from(milliseconds) * 1_000_000)
    else {
        return milliseconds.to_string();
    };
    value
        .format(&Rfc3339)
        .unwrap_or_else(|_| milliseconds.to_string())
}

fn parse_iso_millis(value: &str) -> Option<i64> {
    let value = value.strip_suffix('Z')?;
    let (date, time) = value.split_once('T')?;
    let mut date_parts = date.split('-');
    let year = date_parts.next()?.parse::<i32>().ok()?;
    let month = date_parts.next()?.parse::<u8>().ok()?;
    let day = date_parts.next()?.parse::<u8>().ok()?;
    if date_parts.next().is_some() {
        return None;
    }
    let mut time_parts = time.split(':');
    let hour = time_parts.next()?.parse::<u8>().ok()?;
    let minute = time_parts.next()?.parse::<u8>().ok()?;
    let seconds = time_parts.next()?;
    if time_parts.next().is_some() {
        return None;
    }
    let (second, nanosecond) = if let Some((second, fraction)) = seconds.split_once('.') {
        if fraction.is_empty()
            || fraction.len() > 9
            || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let scale = 10_u32.pow(u32::try_from(9_usize.saturating_sub(fraction.len())).ok()?);
        (
            second.parse::<u8>().ok()?,
            fraction.parse::<u32>().ok()?.checked_mul(scale)?,
        )
    } else {
        (seconds.parse::<u8>().ok()?, 0)
    };
    let date = Date::from_calendar_date(year, Month::try_from(month).ok()?, day).ok()?;
    let time = Time::from_hms_nano(hour, minute, second, nanosecond).ok()?;
    let milliseconds = PrimitiveDateTime::new(date, time)
        .assume_utc()
        .unix_timestamp_nanos()
        / 1_000_000;
    i64::try_from(milliseconds).ok()
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_server::AppServerLaunchConfig;
    use std::fs as std_fs;
    use std::io::Write;

    #[test]
    fn app_server_image_max_retries_matches_frozen_bounded_integer_contract() {
        assert_eq!(
            parse_app_server_image_max_retries(None),
            APP_SERVER_IMAGE_MAX_RETRIES
        );
        assert_eq!(
            parse_app_server_image_max_retries(Some("invalid")),
            APP_SERVER_IMAGE_MAX_RETRIES
        );
        assert_eq!(parse_app_server_image_max_retries(Some("0")), 0);
        assert_eq!(parse_app_server_image_max_retries(Some("3")), 3);
        assert_eq!(parse_app_server_image_max_retries(Some("4")), 3);
    }

    fn provider_free_runtime(
        temporary: &tempfile::TempDir,
        script: &str,
        recovery: RecoveryConfig,
    ) -> (GenerationRuntime, Arc<RuntimeConfig>) {
        let server_root = temporary.path().join("server");
        let data_dir = temporary.path().join("data");
        let workspace = temporary.path().join("workspace");
        std_fs::create_dir_all(&server_root).expect("server root");
        std_fs::create_dir_all(&workspace).expect("workspace");
        let fake = temporary.path().join("fake-codex");
        let mut fake_file = std_fs::File::create(&fake).expect("fake executable");
        fake_file.write_all(script.as_bytes()).expect("fake source");
        fake_file.flush().expect("fake source flushed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fake_file.metadata().expect("fake metadata").permissions();
            permissions.set_mode(0o755);
            std_fs::set_permissions(&fake, permissions).expect("fake executable permissions");
        }
        let config = Arc::new(RuntimeConfig::new(
            server_root,
            data_dir,
            Some(workspace.clone()),
            "server".to_owned(),
        ));
        config.prepare_directories().expect("runtime directories");
        let app_server =
            AppServerBridge::new(workspace, AppServerLaunchConfig::single("fixture", fake));
        (
            GenerationRuntime::new_with_recovery_config(config.clone(), app_server, recovery),
            config,
        )
    }

    fn svg_fixture_job(run_id: &str, output_path: &Path, now: i64) -> ImageGridJob {
        let run_directory = output_path.parent().expect("SVG run directory");
        let artifacts = run_artifacts(run_directory.parent().expect("generated directory"), run_id);
        ImageGridJob {
            id: "svg-job".to_owned(),
            run_id: run_id.to_owned(),
            engine: "codex-svg".to_owned(),
            model: "codex-app-server".to_owned(),
            prompt: "A calm SVG mascot".to_owned(),
            reference_premise: "Keep the round glasses and blue scarf.".to_owned(),
            mood: "warm-mascot".to_owned(),
            prompt_index: 1,
            prompt_total: 1,
            variant: 1,
            total: 2,
            filename: "variant-01.svg".to_owned(),
            output_path: display_path(output_path),
            aspect_ratio: "16:9".to_owned(),
            reference_image_path: None,
            reference_image_url: None,
            manifest_path: artifacts.manifest_path,
            manifest_url: artifacts.manifest_url,
            manifest_view_url: artifacts.manifest_view_url,
            handoff_path: artifacts.handoff_path,
            handoff_url: artifacts.handoff_url,
            handoff_view_url: artifacts.handoff_view_url,
            output_format: "svg".to_owned(),
            status: "queued".to_owned(),
            status_text: "Queued".to_owned(),
            image_url: None,
            log: String::new(),
            thread_id: None,
            turn_id: None,
            error_code: None,
            error_message: None,
            upstream_status: None,
            diagnostic_log: String::new(),
            retry_count: 0,
            timing: JobTiming::queued(now),
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn request_validation_preserves_batch_limits_and_http_defaults() {
        let request = normalize_request(
            &json!({
                "prompts": [" first ", "second"],
                "count": 2,
                "mood": "unknown",
                "engine": "unknown",
                "aspectRatio": "unknown",
                "waitMs": 12.6
            }),
            true,
            None,
        )
        .expect("normalized request");
        assert_eq!(request.prompts, vec!["first", "second"]);
        assert_eq!(request.count, 2);
        assert_eq!(request.mood, "warm-mascot");
        assert_eq!(request.engine, "app-server-image");
        assert_eq!(request.aspect_ratio, "16:9");
        assert_eq!(request.wait_ms, 13);

        let error = normalize_request(
            &json!({
                "prompts": ["one", "two", "three", "four", "five"],
                "count": 5
            }),
            true,
            None,
        )
        .expect_err("too many jobs");
        assert_eq!(error.code.as_deref(), Some("too_many_jobs"));
    }

    #[test]
    fn prompt_builder_matches_the_frozen_primary_contract() {
        let now = now_millis();
        let job = ImageGridJob {
            id: "job".to_owned(),
            run_id: "run".to_owned(),
            engine: "app-server-image".to_owned(),
            model: "app-server-image".to_owned(),
            prompt: "A calm mascot".to_owned(),
            reference_premise: String::new(),
            mood: "warm-mascot".to_owned(),
            prompt_index: 1,
            prompt_total: 1,
            variant: 1,
            total: 2,
            filename: "variant-01.png".to_owned(),
            output_path: "/tmp/variant-01.png".to_owned(),
            aspect_ratio: "16:9".to_owned(),
            reference_image_path: None,
            reference_image_url: None,
            manifest_path: "/tmp/manifest.json".to_owned(),
            manifest_url: "/generated/run/manifest.json".to_owned(),
            manifest_view_url: "/artifacts/run/manifest".to_owned(),
            handoff_path: "/tmp/handoff.md".to_owned(),
            handoff_url: "/generated/run/handoff.md".to_owned(),
            handoff_view_url: "/artifacts/run/handoff".to_owned(),
            output_format: "png".to_owned(),
            status: "queued".to_owned(),
            status_text: "Queued".to_owned(),
            image_url: None,
            log: String::new(),
            thread_id: None,
            turn_id: None,
            error_code: None,
            error_message: None,
            upstream_status: None,
            diagnostic_log: String::new(),
            retry_count: 0,
            timing: JobTiming::queued(now),
            created_at: now,
            updated_at: now,
        };
        let prompt = build_image_prompt(&job);
        assert!(prompt.starts_with(
            "Use the image generation tool to create exactly one polished thumbnail image."
        ));
        assert!(prompt.contains("Batch position:\nVariant 1 of 2"));
        assert!(prompt.contains("Target aspect ratio: 16:9."));
        assert!(prompt.ends_with("- Do not ask follow-up questions.\n"));
    }

    #[test]
    fn codex_svg_prompt_builder_matches_the_frozen_contract() {
        let output_path = Path::new("/tmp/variant-01.svg");
        let job = svg_fixture_job("feedface", output_path, now_millis());

        assert_eq!(
            build_svg_prompt(&job),
            "Create one polished thumbnail-like image as a self-contained SVG file.\n\n\
Write the SVG to this exact absolute path:\n/tmp/variant-01.svg\n\n\
User concept:\nA calm SVG mascot\n\n\
Reference premise:\nKeep the round glasses and blue scarf.\n\n\
Style direction:\nwarm anime blog mascot portrait, soft desk lighting, charming and wholesome, gentle expression, polished illustration\n\n\
Batch position:\nVariant 1 of 2\n\n\
Hard requirements:\n\
- Create exactly one valid SVG file at the path above.\n\
- Use a 16:9 canvas: viewBox=\"0 0 1280 720\".\n\
- Keep all assets inline. Do not use remote images, external fonts, or network downloads.\n\
- Make the composition visibly different from the other variants: vary layout, focal object, accent color, and typographic rhythm.\n\
- Use readable Japanese text only when useful; keep it short and non-overlapping.\n\
- Include subtle texture, depth, and a clear focal hierarchy.\n\
- Do not ask follow-up questions.\n\
- Finish with one concise sentence that includes \"variant-01.svg\".\n"
        );
    }

    #[tokio::test]
    async fn provider_free_codex_svg_preserves_public_identity_in_manifest() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let server_root = temporary.path().join("server");
        let data_dir = temporary.path().join("data");
        let workspace = temporary.path().join("workspace");
        std_fs::create_dir_all(&server_root).expect("server root");
        std_fs::create_dir_all(&workspace).expect("workspace");
        let config = Arc::new(RuntimeConfig::new(
            server_root,
            data_dir.clone(),
            Some(workspace.clone()),
            "server".to_owned(),
        ));
        config.prepare_directories().expect("runtime directories");

        let run_id = "feedface";
        let run_directory = config.generated_dir.join(run_id);
        std_fs::create_dir_all(&run_directory).expect("run directory");
        let output_path = run_directory.join("variant-01.svg");
        let request_log = temporary.path().join("requests.jsonl");
        let expected_svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1280 720\"><rect width=\"1280\" height=\"720\" fill=\"#123456\"/></svg>\n";
        let fake = temporary.path().join("fake-codex");
        let script = r##"#!/bin/sh
test "$1" = "app-server" || exit 2
svg_path="__SVG_PATH__"
request_log="__REQUEST_LOG__"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$request_log"
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":1,"result":{"userAgent":"fixture","codexHome":"/tmp/fixture","platformFamily":"unix","platformOs":"macos"}}'
      ;;
    *'"method":"initialized"'*)
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":2,"result":{"thread":{"id":"svg-thread"}}}'
      ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1280 720"><rect width="1280" height="720" fill="#123456"/></svg>' > "$svg_path"
      printf '%s\n' '{"id":3,"result":{"turn":{"id":"svg-turn"}}}'
      printf '%s\n' '{"method":"item/completed","params":{"threadId":"svg-thread","turnId":"svg-turn","completedAtMs":1,"item":{"type":"agentMessage","id":"message","text":"<svg>assistant text must not replace the requested file</svg>"}}}'
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"svg-thread","turn":{"id":"svg-turn","items":[],"itemsView":"full","status":"completed","error":null,"startedAt":null,"completedAt":null,"durationMs":1}}}'
      ;;
  esac
done
"##
        .replace("__SVG_PATH__", &display_path(&output_path))
        .replace("__REQUEST_LOG__", &display_path(&request_log));
        let mut fake_file = std_fs::File::create(&fake).expect("fake executable");
        fake_file.write_all(script.as_bytes()).expect("fake source");
        fake_file.flush().expect("fake source flushed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fake_file.metadata().expect("fake metadata").permissions();
            permissions.set_mode(0o755);
            std_fs::set_permissions(&fake, permissions).expect("fake executable permissions");
        }

        let app_server =
            AppServerBridge::new(workspace, AppServerLaunchConfig::single("fixture", fake));
        let runtime = GenerationRuntime::new(config.clone(), app_server);
        let now = now_millis();
        let job = svg_fixture_job(run_id, &output_path, now);
        let artifacts = run_artifacts(&config.generated_dir, run_id);
        let request = RunRequestRecord {
            prompts: vec![PromptRecord {
                index: 1,
                prompt: job.prompt.clone(),
            }],
            mood: job.mood.clone(),
            engine: job.engine.clone(),
            model: job.model.clone(),
            aspect_ratio: job.aspect_ratio.clone(),
            variants_per_prompt: job.total,
            prompt_total: job.prompt_total,
            reference_premise: job.reference_premise.clone(),
            reference_image: None,
        };
        runtime
            .inner
            .jobs
            .write()
            .await
            .insert(job.id.clone(), job.clone());
        runtime.inner.runs.write().await.insert(
            run_id.to_owned(),
            RunRecord {
                run_id: run_id.to_owned(),
                job_ids: vec![job.id.clone()],
                initial_jobs: vec![job.clone()],
                request,
                artifacts: artifacts.clone(),
                created_at: now,
                notify: Arc::new(Notify::new()),
            },
        );
        runtime
            .write_artifacts(run_id)
            .await
            .expect("initial artifacts");

        runtime.run_job(&job.id).await;

        let completed = runtime.job(&job.id).await.expect("completed SVG job");
        assert_eq!(completed.status, "done");
        assert_eq!(completed.status_text, "Generated");
        assert_eq!(completed.thread_id.as_deref(), Some("svg-thread"));
        assert_eq!(completed.turn_id.as_deref(), Some("svg-turn"));
        assert_eq!(
            completed.image_url.as_deref(),
            Some("/generated/feedface/variant-01.svg")
        );
        assert_eq!(
            std_fs::read_to_string(&output_path).expect("generated SVG"),
            expected_svg
        );

        let requests = std_fs::read_to_string(&request_log).expect("App Server request log");
        let requests = requests
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("request JSON"))
            .collect::<Vec<_>>();
        let thread_start = requests
            .iter()
            .find(|request| request["method"] == "thread/start")
            .expect("thread/start request");
        assert_eq!(thread_start["params"]["sandbox"], "workspace-write");
        let turn_start = requests
            .iter()
            .find(|request| request["method"] == "turn/start")
            .expect("turn/start request");
        assert_eq!(
            turn_start["params"]["sandboxPolicy"],
            json!({
                "type": "workspaceWrite",
                "writableRoots": [display_path(&data_dir)],
                "networkAccess": false,
                "excludeTmpdirEnvVar": false,
                "excludeSlashTmp": false
            })
        );
        assert_eq!(
            turn_start["params"]["input"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(
            turn_start["params"]["input"][0]["text"],
            build_svg_prompt(&job)
        );

        let manifest: Value = serde_json::from_slice(
            &std_fs::read(&artifacts.manifest_path).expect("manifest artifact"),
        )
        .expect("manifest JSON");
        assert_eq!(manifest["app"], "codex-image-grid");
        assert_eq!(manifest["request"]["engine"], "codex-svg");
        assert_eq!(manifest["outputs"][0]["status"], "done");
        assert_eq!(manifest["outputs"][0]["outputFormat"], "svg");
        assert_eq!(
            manifest["outputs"][0]["imageUrl"],
            "/generated/feedface/variant-01.svg"
        );
        let handoff = std_fs::read_to_string(&artifacts.handoff_path).expect("handoff artifact");
        assert!(handoff.contains("- Engine: codex-svg"));
        assert!(handoff.contains("- Status: done (Generated)"));
        assert!(handoff.contains(&display_path(&output_path)));
    }

    #[test]
    fn terminal_failed_runs_are_completed_but_not_done() {
        let now = now_millis();
        let mut timing = JobTiming::queued(now);
        timing.transition("error", now);
        let job = ImageGridJob {
            id: "job".to_owned(),
            run_id: "run".to_owned(),
            engine: "app-server-image".to_owned(),
            model: "app-server-image".to_owned(),
            prompt: "prompt".to_owned(),
            reference_premise: String::new(),
            mood: "warm-mascot".to_owned(),
            prompt_index: 1,
            prompt_total: 1,
            variant: 1,
            total: 1,
            filename: "variant-01.png".to_owned(),
            output_path: "/tmp/variant-01.png".to_owned(),
            aspect_ratio: "16:9".to_owned(),
            reference_image_path: None,
            reference_image_url: None,
            manifest_path: "/tmp/manifest.json".to_owned(),
            manifest_url: "/generated/run/manifest.json".to_owned(),
            manifest_view_url: "/artifacts/run/manifest".to_owned(),
            handoff_path: "/tmp/handoff.md".to_owned(),
            handoff_url: "/generated/run/handoff.md".to_owned(),
            handoff_view_url: "/artifacts/run/handoff".to_owned(),
            output_format: "png".to_owned(),
            status: "error".to_owned(),
            status_text: "failed".to_owned(),
            image_url: None,
            log: String::new(),
            thread_id: None,
            turn_id: None,
            error_code: Some("Failure".to_owned()),
            error_message: Some("failed".to_owned()),
            upstream_status: Some("failed".to_owned()),
            diagnostic_log: "failure".to_owned(),
            retry_count: 0,
            timing,
            created_at: now,
            updated_at: now,
        };
        let (status, counts, completed) = run_status(&[job]);
        assert_eq!(status, "error");
        assert!(completed);
        assert_eq!(counts["failed"], 1);
    }

    #[tokio::test]
    async fn recovery_contract_missing_output_retries_once_and_succeeds_without_cooldown() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let script = r#"#!/bin/sh
test "$1" = "app-server" || exit 2
attempt=0
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":1,"result":{"userAgent":"fixture","codexHome":"/tmp/fixture","platformFamily":"unix","platformOs":"macos"}}'
      ;;
    *'"method":"initialized"'*)
      ;;
    *'"method":"thread/start"'*)
      attempt=$((attempt + 1))
      if [ "$attempt" -eq 1 ]; then
        printf '%s\n' '{"id":2,"result":{"thread":{"id":"missing-thread-1"}}}'
      else
        printf '%s\n' '{"id":4,"result":{"thread":{"id":"missing-thread-2"}}}'
      fi
      ;;
    *'"method":"turn/start"'*)
      if [ "$attempt" -eq 1 ]; then
        printf '%s\n' '{"id":3,"result":{"turn":{"id":"missing-turn-1"}}}'
        printf '%s\n' '{"method":"turn/completed","params":{"threadId":"missing-thread-1","turn":{"id":"missing-turn-1","items":[],"itemsView":"full","status":"completed","error":null,"startedAt":null,"completedAt":null,"durationMs":1}}}'
      else
        printf '%s\n' '{"id":5,"result":{"turn":{"id":"missing-turn-2"}}}'
        printf '%s\n' '{"method":"item/completed","params":{"threadId":"missing-thread-2","turnId":"missing-turn-2","completedAtMs":1,"item":{"type":"imageGeneration","id":"image-2","status":"completed","revisedPrompt":null,"result":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="}}}'
        printf '%s\n' '{"method":"turn/completed","params":{"threadId":"missing-thread-2","turn":{"id":"missing-turn-2","items":[],"itemsView":"full","status":"completed","error":null,"startedAt":null,"completedAt":null,"durationMs":1}}}'
      fi
      ;;
  esac
done
"#;
        let (runtime, _) = provider_free_runtime(
            &temporary,
            script,
            RecoveryConfig {
                max_retries: 1,
                retry_base: Duration::ZERO,
                rate_limit_cooldown: Duration::ZERO,
                rate_limit_cooldown_max: Duration::ZERO,
                job_timeout: Duration::from_secs(5),
            },
        );
        let (_, response) = runtime
            .create_run(
                &json!({
                    "prompts": ["recover missing output"],
                    "count": 1,
                    "waitMs": 7000
                }),
                true,
                None,
            )
            .await
            .expect("provider-free run");
        let output = &response["outputs"][0];
        assert_eq!(output["status"], "done");
        assert_eq!(output["retryCount"], 1);
        assert_eq!(output["timing"]["attemptCount"], 2);
        assert_eq!(output["timing"]["cooldownMs"], 0);
        assert_eq!(output["errorCode"], Value::Null);
        assert!(
            output["diagnosticLog"]
                .as_str()
                .is_some_and(|log| log.contains("reason=missing-output"))
        );
        assert!(
            output["outputPath"]
                .as_str()
                .is_some_and(|path| Path::new(path).is_file())
        );
    }

    #[tokio::test]
    async fn empty_image_result_is_rejected_without_publishing_an_artifact() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let script = r#"#!/bin/sh
test "$1" = "app-server" || exit 2
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":1,"result":{"userAgent":"fixture","codexHome":"/tmp/fixture","platformFamily":"unix","platformOs":"macos"}}'
      ;;
    *'"method":"initialized"'*)
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":2,"result":{"thread":{"id":"empty-image-thread"}}}'
      ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":3,"result":{"turn":{"id":"empty-image-turn"}}}'
      printf '%s\n' '{"method":"item/completed","params":{"threadId":"empty-image-thread","turnId":"empty-image-turn","completedAtMs":1,"item":{"type":"imageGeneration","id":"image-empty","status":"completed","revisedPrompt":null,"result":""}}}'
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"empty-image-thread","turn":{"id":"empty-image-turn","items":[],"itemsView":"full","status":"completed","error":null,"startedAt":null,"completedAt":null,"durationMs":1}}}'
      ;;
  esac
done
"#;
        let (runtime, _) = provider_free_runtime(
            &temporary,
            script,
            RecoveryConfig {
                max_retries: 0,
                retry_base: Duration::ZERO,
                rate_limit_cooldown: Duration::ZERO,
                rate_limit_cooldown_max: Duration::ZERO,
                job_timeout: Duration::from_secs(5),
            },
        );
        let (_, response) = runtime
            .create_run(
                &json!({
                    "prompts": ["reject empty image output"],
                    "count": 1,
                    "waitMs": 7000
                }),
                true,
                None,
            )
            .await
            .expect("provider-free run");
        let output = &response["outputs"][0];
        assert_eq!(output["status"], "error");
        assert_eq!(output["errorCode"], "ImageWriteFailed");
        assert!(
            output["errorMessage"]
                .as_str()
                .is_some_and(|message| message.contains("not a valid png image"))
        );
        assert!(
            output["outputPath"]
                .as_str()
                .is_some_and(|path| !Path::new(path).exists())
        );
        assert_eq!(output["imageUrl"], Value::Null);
    }

    #[tokio::test]
    async fn recovery_contract_stable_error_maps_rate_limit_and_waits_for_global_cooldown() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let script = r#"#!/bin/sh
test "$1" = "app-server" || exit 2
attempt=0
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":1,"result":{"userAgent":"fixture","codexHome":"/tmp/fixture","platformFamily":"unix","platformOs":"macos"}}'
      ;;
    *'"method":"initialized"'*)
      ;;
    *'"method":"thread/start"'*)
      attempt=$((attempt + 1))
      if [ "$attempt" -eq 1 ]; then
        printf '%s\n' '{"id":2,"result":{"thread":{"id":"rate-thread-1"}}}'
      else
        printf '%s\n' '{"id":4,"result":{"thread":{"id":"rate-thread-2"}}}'
      fi
      ;;
    *'"method":"turn/start"'*)
      if [ "$attempt" -eq 1 ]; then
        printf '%s\n' '{"id":3,"result":{"turn":{"id":"rate-turn-1"}}}'
        printf '%s\n' '{"method":"error","params":{"error":{"message":"Too many requests for image generation (429)","codexErrorInfo":{"responseTooManyFailedAttempts":{"httpStatusCode":429}},"additionalDetails":null},"willRetry":false,"threadId":"rate-thread-1","turnId":"rate-turn-1"}}'
      else
        printf '%s\n' '{"id":5,"result":{"turn":{"id":"rate-turn-2"}}}'
        printf '%s\n' '{"method":"item/completed","params":{"threadId":"rate-thread-2","turnId":"rate-turn-2","completedAtMs":1,"item":{"type":"imageGeneration","id":"image-2","status":"completed","revisedPrompt":null,"result":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="}}}'
        printf '%s\n' '{"method":"turn/completed","params":{"threadId":"rate-thread-2","turn":{"id":"rate-turn-2","items":[],"itemsView":"full","status":"completed","error":null,"startedAt":null,"completedAt":null,"durationMs":1}}}'
      fi
      ;;
  esac
done
"#;
        let (runtime, _) = provider_free_runtime(
            &temporary,
            script,
            RecoveryConfig {
                max_retries: 1,
                retry_base: Duration::ZERO,
                rate_limit_cooldown: Duration::from_millis(15),
                rate_limit_cooldown_max: Duration::from_millis(15),
                job_timeout: Duration::from_secs(5),
            },
        );
        let (_, response) = runtime
            .create_run(
                &json!({
                    "prompts": ["recover rate limit"],
                    "count": 1,
                    "waitMs": 7000
                }),
                true,
                None,
            )
            .await
            .expect("provider-free run");
        let output = &response["outputs"][0];
        assert_eq!(output["status"], "done");
        assert_eq!(output["retryCount"], 1);
        assert!(
            output["timing"]["cooldownMs"]
                .as_i64()
                .is_some_and(|milliseconds| milliseconds >= 10)
        );
        let diagnostic = output["diagnosticLog"].as_str().expect("diagnostic log");
        assert!(diagnostic.contains("code=TooManyRequests"));
        assert!(diagnostic.contains("rate-limit-cooldown"));
        assert!(diagnostic.contains("reason=rate-limit"));
    }

    #[tokio::test]
    async fn recovery_contract_whole_attempt_timeout_interrupts_owned_turn_and_releases_job() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let request_log = temporary.path().join("requests.jsonl");
        let script = r#"#!/bin/sh
test "$1" = "app-server" || exit 2
request_log="__REQUEST_LOG__"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$request_log"
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":1,"result":{"userAgent":"fixture","codexHome":"/tmp/fixture","platformFamily":"unix","platformOs":"macos"}}'
      ;;
    *'"method":"initialized"'*)
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":2,"result":{"thread":{"id":"timeout-thread"}}}'
      ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"id":3,"result":{"turn":{"id":"timeout-turn"}}}'
      ;;
    *'"method":"turn/interrupt"'*)
      printf '%s\n' '{"id":4,"result":{}}'
      ;;
  esac
done
"#
        .replace("__REQUEST_LOG__", &display_path(&request_log));
        let (runtime, _) = provider_free_runtime(
            &temporary,
            &script,
            RecoveryConfig {
                max_retries: 0,
                retry_base: Duration::ZERO,
                rate_limit_cooldown: Duration::ZERO,
                rate_limit_cooldown_max: Duration::ZERO,
                job_timeout: Duration::from_secs(1),
            },
        );
        let diagnostics = runtime.inner.app_server.ensure_ready().await;
        assert!(
            diagnostics.ready,
            "timeout fixture App Server must initialize before the attempt deadline: {diagnostics:?}"
        );
        let (_, response) = runtime
            .create_run(
                &json!({
                    "prompts": ["timeout"],
                    "count": 1,
                    "waitMs": 3000
                }),
                true,
                None,
            )
            .await
            .expect("provider-free run");
        let output = &response["outputs"][0];
        assert_eq!(output["status"], "error");
        assert_eq!(output["errorCode"], "ImageGenerationTimeout");
        let diagnostic = output["diagnosticLog"].as_str().unwrap_or_default();
        let requests = std_fs::read_to_string(request_log).expect("request log");
        assert!(
            diagnostic.contains("[turn/interrupt]"),
            "diagnostic={diagnostic:?}; requests={requests:?}"
        );
        assert!(requests.contains("\"method\":\"turn/interrupt\""));
        assert_eq!(runtime.scheduler_snapshot().active, 0);
    }

    #[tokio::test]
    async fn shutdown_contract_closes_active_queued_cooldown_rpc_writes_and_artifacts_once() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let request_log = temporary.path().join("shutdown-requests.jsonl");
        let script = r#"#!/bin/sh
test "$1" = "app-server" || exit 2
request_log="__REQUEST_LOG__"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$request_log"
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"id":1,"result":{"userAgent":"fixture","codexHome":"/tmp/fixture","platformFamily":"unix","platformOs":"macos"}}'
      ;;
    *'"method":"initialized"'*)
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"id":2,"result":{"thread":{"id":"shutdown-thread"}}}'
      ;;
    *'"method":"turn/start"'*)
      ;;
  esac
done
"#
        .replace("__REQUEST_LOG__", &display_path(&request_log));
        let (runtime, _) = provider_free_runtime(
            &temporary,
            &script,
            RecoveryConfig {
                max_retries: 1,
                retry_base: Duration::from_secs(10),
                rate_limit_cooldown: Duration::from_secs(10),
                rate_limit_cooldown_max: Duration::from_secs(10),
                job_timeout: Duration::from_secs(30),
            },
        );
        let diagnostics = runtime.inner.app_server.ensure_ready().await;
        assert!(
            diagnostics.ready,
            "shutdown fixture App Server must initialize before job scheduling: {diagnostics:?}"
        );
        let client = runtime
            .inner
            .app_server
            .current_client()
            .await
            .expect("owned App Server child");

        let (_, first_response) = runtime
            .create_run(
                &json!({
                    "prompts": ["active pending RPC"],
                    "count": 1,
                    "waitMs": 0
                }),
                true,
                None,
            )
            .await
            .expect("first run");
        let first_run_id = first_response["runId"].as_str().expect("first run id");
        let first_job_id = first_response["outputs"][0]["id"]
            .as_str()
            .expect("first job id")
            .to_owned();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let running = runtime
                    .job(&first_job_id)
                    .await
                    .is_some_and(|job| job.status == "running");
                let pending_turn = std_fs::read_to_string(&request_log)
                    .ok()
                    .is_some_and(|requests| requests.contains("\"method\":\"turn/start\""));
                if running && pending_turn {
                    break;
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("active pending RPC");
        assert!(client.is_running().await);

        *runtime.inner.rate_limit_cooldown_until.lock().await =
            Instant::now() + Duration::from_secs(10);
        let (_, queued_response) = runtime
            .create_run(
                &json!({
                    "prompts": ["queued in cooldown"],
                    "count": 1,
                    "waitMs": 0
                }),
                true,
                None,
            )
            .await
            .expect("queued run");
        let queued_run_id = queued_response["runId"]
            .as_str()
            .expect("queued run id")
            .to_owned();
        let queued_job_id = queued_response["outputs"][0]["id"]
            .as_str()
            .expect("queued job id")
            .to_owned();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if runtime
                    .job(&queued_job_id)
                    .await
                    .is_some_and(|job| job.status_text.contains("rate-limit cooldown"))
                {
                    break;
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("job entered cooldown");

        let attempt_id = runtime
            .inner
            .attempts
            .read()
            .await
            .get(&first_job_id)
            .expect("active attempt")
            .id
            .clone();
        let artifact_gate = runtime.inner.clone();
        let (writer_ready, writer_started) = tokio::sync::oneshot::channel();
        let pending_writer = tokio::spawn(async move {
            let _guard = artifact_gate.artifact_write.lock().await;
            let _ = writer_ready.send(());
            sleep(Duration::from_millis(30)).await;
        });
        writer_started.await.expect("pending artifact writer ready");

        let late_runtime = runtime.clone();
        let late_job_id = first_job_id.clone();
        let late_write = tokio::spawn(async move {
            let mut shutdown = late_runtime.shutdown_receiver();
            wait_for_shutdown(&mut shutdown).await;
            late_runtime
                .write_job_image(
                    &late_job_id,
                    &attempt_id,
                    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
                )
                .await
                .expect("retired late write is ignored");
        });

        let started = Instant::now();
        let left = runtime.clone();
        let right = runtime.clone();
        tokio::join!(left.shutdown(), right.shutdown());
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "shutdown must cancel the ten-second cooldown promptly"
        );
        pending_writer.await.expect("pending writer drained");
        late_write.await.expect("late writer drained");

        for (run_id, job_id) in [
            (first_run_id.to_owned(), first_job_id),
            (queued_run_id, queued_job_id),
        ] {
            let job = runtime.job(&job_id).await.expect("closed job");
            assert_eq!(job.status, "error");
            assert_eq!(
                job.status_text,
                "Runtime stopped before generation completed"
            );
            assert_eq!(job.error_code.as_deref(), Some("RuntimeClosed"));
            assert_eq!(job.error_message.as_deref(), Some(RUNTIME_CLOSED_MESSAGE));
            assert!(!Path::new(&job.output_path).exists());

            let manifest_path = runtime
                .inner
                .config
                .generated_dir
                .join(run_id)
                .join("manifest.json");
            let manifest: Value =
                serde_json::from_slice(&std_fs::read(&manifest_path).expect("final manifest"))
                    .expect("manifest JSON");
            assert_eq!(manifest["outputs"][0]["errorCode"], "RuntimeClosed");
            assert!(
                manifest["outputs"][0]["statusText"]
                    .as_str()
                    .is_some_and(|text| text.contains("Runtime stopped"))
            );
            let handoff = std_fs::read_to_string(
                manifest_path
                    .parent()
                    .expect("run directory")
                    .join("handoff.md"),
            )
            .expect("final handoff");
            assert!(handoff.contains("RuntimeClosed"));
        }
        assert!(!client.is_running().await);
        assert!(runtime.inner.app_server.current_client().await.is_none());
        assert!(runtime.inner.attempts.read().await.is_empty());
        assert_eq!(runtime.scheduler_snapshot().active, 0);
        assert_eq!(runtime.scheduler_snapshot().queued, 0);

        let requests = std_fs::read_to_string(request_log).expect("request log");
        assert_eq!(requests.matches("\"method\":\"turn/start\"").count(), 1);
        assert!(!requests.contains("\"method\":\"turn/interrupt\""));
    }

    #[tokio::test]
    async fn shutdown_contract_artifact_update_failure_emits_one_exact_public_log() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let (runtime, config) =
            provider_free_runtime(&temporary, "#!/bin/sh\nexit 0\n", RecoveryConfig::default());
        let run_id = "deadbeef";
        let run_directory = config.generated_dir.join(run_id);
        std_fs::create_dir_all(&run_directory).expect("run directory");
        let now = now_millis();
        let job = svg_fixture_job(run_id, &run_directory.join("variant-01.svg"), now);
        let artifacts = run_artifacts(&config.generated_dir, run_id);
        let request = RunRequestRecord {
            prompts: vec![PromptRecord {
                index: 1,
                prompt: job.prompt.clone(),
            }],
            mood: job.mood.clone(),
            engine: job.engine.clone(),
            model: job.model.clone(),
            aspect_ratio: job.aspect_ratio.clone(),
            variants_per_prompt: job.total,
            prompt_total: job.prompt_total,
            reference_premise: job.reference_premise.clone(),
            reference_image: None,
        };
        runtime
            .inner
            .jobs
            .write()
            .await
            .insert(job.id.clone(), job.clone());
        runtime.inner.runs.write().await.insert(
            run_id.to_owned(),
            RunRecord {
                run_id: run_id.to_owned(),
                job_ids: vec![job.id.clone()],
                initial_jobs: vec![job.clone()],
                request,
                artifacts,
                created_at: now,
                notify: Arc::new(Notify::new()),
            },
        );
        runtime
            .write_artifacts(run_id)
            .await
            .expect("initial artifacts");

        let preserved_directory = config.generated_dir.join("preserved-deadbeef");
        std_fs::rename(&run_directory, &preserved_directory).expect("preserve run directory");
        std_fs::write(&run_directory, b"blocks artifact directory").expect("blocking regular file");
        let mut events = runtime.subscribe();
        runtime
            .update_job(&job.id, |job, _| {
                job.status_text = "Terminal update".to_owned();
            })
            .await;

        let mut artifact_logs = Vec::new();
        while let Ok(event) = events.try_recv() {
            if event.name == "server-log" {
                artifact_logs.push(event.data);
            }
        }
        assert_eq!(artifact_logs.len(), 1);
        assert_eq!(artifact_logs[0]["stream"], "artifact");
        assert!(
            artifact_logs[0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("artifact directory"))
        );
    }

    #[tokio::test]
    async fn startup_restoration_normalizes_interrupted_jobs_without_requeueing_unsafe_artifacts() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let temporary_root =
            std_fs::canonicalize(temporary.path()).expect("canonical temporary directory");
        let server_root = temporary_root.join("server");
        let data_dir = temporary_root.join("data");
        let workspace = temporary_root.join("workspace");
        std_fs::create_dir_all(&server_root).expect("server root");
        std_fs::create_dir_all(&workspace).expect("workspace");
        let config = Arc::new(RuntimeConfig::new(
            server_root,
            data_dir,
            Some(workspace.clone()),
            "server".to_owned(),
        ));
        config.prepare_directories().expect("runtime directories");
        let fake = temporary.path().join("unused-codex");
        std_fs::write(&fake, "#!/bin/sh\nexit 97\n").expect("fake executable");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std_fs::metadata(&fake)
                .expect("fake metadata")
                .permissions();
            permissions.set_mode(0o755);
            std_fs::set_permissions(&fake, permissions).expect("fake executable permissions");
        }

        let manifest_for = |run_id: &str, jobs: &[ImageGridJob]| {
            let artifacts = run_artifacts(&config.generated_dir, run_id);
            let first = &jobs[0];
            let request = RunRequestRecord {
                prompts: vec![PromptRecord {
                    index: 1,
                    prompt: first.prompt.clone(),
                }],
                mood: first.mood.clone(),
                engine: first.engine.clone(),
                model: first.model.clone(),
                aspect_ratio: first.aspect_ratio.clone(),
                variants_per_prompt: jobs.len(),
                prompt_total: 1,
                reference_premise: first.reference_premise.clone(),
                reference_image: None,
            };
            json!({
                "schemaVersion": 1,
                "app": APP_IDENTITY,
                "runId": run_id,
                "createdAt": iso_time(jobs.iter().map(|job| job.created_at).min().unwrap()),
                "updatedAt": iso_time(jobs.iter().map(|job| job.updated_at).max().unwrap()),
                "cwd": display_path(&workspace),
                "server": {},
                "artifacts": artifacts,
                "request": request,
                "diagnostics": [],
                "outputs": jobs.iter().map(output_value).collect::<Vec<_>>()
            })
        };
        let write_manifest = |run_id: &str, manifest: &Value| {
            let run_directory = config.generated_dir.join(run_id);
            std_fs::create_dir_all(&run_directory).expect("run directory");
            let mut bytes = serde_json::to_vec_pretty(manifest).expect("manifest JSON");
            bytes.push(b'\n');
            std_fs::write(run_directory.join("manifest.json"), bytes).expect("manifest fixture");
        };

        let now = now_millis();
        let safe_run_id = "abc123ef";
        let safe_run_directory = config.generated_dir.join(safe_run_id);
        std_fs::create_dir_all(&safe_run_directory).expect("safe run directory");
        let mut active_with_file =
            svg_fixture_job(safe_run_id, &safe_run_directory.join("variant-01.svg"), now);
        active_with_file.id = "active-with-file".to_owned();
        active_with_file.prompt = "restore persisted jobs".to_owned();
        active_with_file.total = 4;
        active_with_file.status = "running".to_owned();
        active_with_file.status_text = "Generating".to_owned();
        active_with_file.timing.transition("starting", now);
        active_with_file.timing.transition("running", now);
        std_fs::write(&active_with_file.output_path, "<svg/>").expect("completed output fixture");

        let mut active_without_file = active_with_file.clone();
        active_without_file.id = "active-without-file".to_owned();
        active_without_file.variant = 2;
        active_without_file.filename = "variant-02.svg".to_owned();
        active_without_file.output_path =
            display_path(&safe_run_directory.join(&active_without_file.filename));
        active_without_file.status = "queued".to_owned();
        active_without_file.status_text = "Queued".to_owned();
        active_without_file.image_url = None;
        active_without_file.timing = JobTiming::queued(now);

        let mut claimed_done_without_file = active_without_file.clone();
        claimed_done_without_file.id = "claimed-done-without-file".to_owned();
        claimed_done_without_file.variant = 3;
        claimed_done_without_file.filename = "variant-03.svg".to_owned();
        claimed_done_without_file.output_path =
            display_path(&safe_run_directory.join(&claimed_done_without_file.filename));
        claimed_done_without_file.status = "done".to_owned();
        claimed_done_without_file.status_text = "Generated".to_owned();
        claimed_done_without_file.timing.transition("done", now);

        let mut terminal_error = active_without_file.clone();
        terminal_error.id = "terminal-error".to_owned();
        terminal_error.variant = 4;
        terminal_error.filename = "variant-04.svg".to_owned();
        terminal_error.output_path =
            display_path(&safe_run_directory.join(&terminal_error.filename));
        terminal_error.status = "error".to_owned();
        terminal_error.status_text = "Upstream failed".to_owned();
        terminal_error.error_message = Some("fixture failure".to_owned());
        terminal_error.timing.transition("error", now);

        let safe_jobs = vec![
            active_with_file,
            active_without_file,
            claimed_done_without_file,
            terminal_error,
        ];
        write_manifest(safe_run_id, &manifest_for(safe_run_id, &safe_jobs));

        let unsafe_run_id = "deadbeef";
        let unsafe_run_directory = config.generated_dir.join(unsafe_run_id);
        let unsafe_output_path = unsafe_run_directory.join("variant-01.svg");
        let mut unsafe_job = svg_fixture_job(unsafe_run_id, &unsafe_output_path, now);
        unsafe_job.id = "unsafe-output-path".to_owned();
        unsafe_job.prompt = "unsafe persisted path".to_owned();
        unsafe_job.total = 1;
        let mut unsafe_manifest = manifest_for(unsafe_run_id, &[unsafe_job]);
        unsafe_manifest["outputs"][0]["outputPath"] =
            Value::String(display_path(&temporary_root.join("outside.svg")));
        write_manifest(unsafe_run_id, &unsafe_manifest);

        let uppercase_run_id = "ABCDEF01";
        let uppercase_run_directory = config.generated_dir.join(uppercase_run_id);
        let mut uppercase_job = svg_fixture_job(
            uppercase_run_id,
            &uppercase_run_directory.join("variant-01.svg"),
            now,
        );
        uppercase_job.id = "uppercase-run".to_owned();
        uppercase_job.prompt = "uppercase run".to_owned();
        uppercase_job.total = 1;
        write_manifest(
            uppercase_run_id,
            &manifest_for(uppercase_run_id, &[uppercase_job]),
        );

        let app_server =
            AppServerBridge::new(workspace, AppServerLaunchConfig::single("fixture", fake));
        let runtime = GenerationRuntime::new_with_recovery_config(
            config.clone(),
            app_server,
            RecoveryConfig::default(),
        );

        let jobs = runtime.snapshot().await;
        assert_eq!(jobs.len(), 4);
        let restored_with_file = jobs
            .iter()
            .find(|job| job.id == "active-with-file")
            .expect("active job with output");
        assert_eq!(restored_with_file.status, "done");
        assert_eq!(restored_with_file.status_text, "Restored after restart");
        assert_eq!(
            restored_with_file.image_url.as_deref(),
            Some("/generated/abc123ef/variant-01.svg")
        );
        let restored_without_file = jobs
            .iter()
            .find(|job| job.id == "active-without-file")
            .expect("active job without output");
        assert_eq!(restored_without_file.status, "error");
        assert_eq!(restored_without_file.status_text, "Interrupted by restart");
        assert_eq!(
            restored_without_file.error_message.as_deref(),
            Some("Interrupted by restart")
        );
        let restored_claimed_done = jobs
            .iter()
            .find(|job| job.id == "claimed-done-without-file")
            .expect("claimed done job");
        assert_eq!(restored_claimed_done.status, "error");
        assert_eq!(
            restored_claimed_done.error_message.as_deref(),
            Some("Interrupted by restart")
        );
        let restored_terminal_error = jobs
            .iter()
            .find(|job| job.id == "terminal-error")
            .expect("terminal error job");
        assert_eq!(restored_terminal_error.status, "error");
        assert_eq!(restored_terminal_error.status_text, "Upstream failed");
        assert_eq!(
            restored_terminal_error.error_message.as_deref(),
            Some("fixture failure")
        );
        assert!(runtime.run_response(unsafe_run_id, false).await.is_none());
        assert!(
            runtime
                .run_response(uppercase_run_id, false)
                .await
                .is_none()
        );
        assert_eq!(runtime.scheduler_snapshot().queued, 0);
        assert_eq!(runtime.scheduler_snapshot().active, 0);
        assert!(runtime.inner.attempts.read().await.is_empty());

        let persisted: Value = serde_json::from_slice(
            &std_fs::read(safe_run_directory.join("manifest.json"))
                .expect("persisted restored manifest"),
        )
        .expect("persisted restored JSON");
        assert_eq!(persisted["outputs"][0]["status"], "done");
        assert_eq!(
            persisted["outputs"][0]["statusText"],
            "Restored after restart"
        );
        assert_eq!(persisted["outputs"][1]["status"], "error");
        assert_eq!(
            persisted["outputs"][1]["errorMessage"],
            "Interrupted by restart"
        );
        let handoff =
            std_fs::read_to_string(safe_run_directory.join("handoff.md")).expect("handoff");
        assert!(handoff.contains("Restored after restart"));
        assert!(handoff.contains("Interrupted by restart"));
        assert!(
            std_fs::read_dir(&safe_run_directory)
                .expect("safe run entries")
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
        );
    }
}
