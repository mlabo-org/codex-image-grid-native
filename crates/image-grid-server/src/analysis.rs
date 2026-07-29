use crate::RuntimeConfig;
use crate::app_server::{AppServerBridge, AppServerDiagnostics, RUNTIME_CLOSED_MESSAGE};
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use image_grid_core::{MAX_REFERENCE_IMAGE_BYTES, stage_reference_image};
use serde_json::{Value, json};
use std::fs as std_fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::fs;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::timeout;
use uuid::Uuid;

const ANALYSIS_TIMEOUT: Duration = Duration::from_secs(180);
pub(crate) const ANALYZE_PROMPT: &str = "\
Analyze the attached local reference image for image generation.\n\
\n\
Return only concise Japanese notes. Do not include an introduction.\n\
Focus on stable visual identity and generation-useful cues:\n\
- character or object identity\n\
- hair, eyes, clothing, accessories, expression, pose, silhouette\n\
- color palette and lighting\n\
- mood, setting, composition\n\
- traits that should be preserved\n\
\n\
Keep it under 8 short bullet points.";

#[derive(Clone)]
pub(crate) struct ReferenceAnalysisRuntime {
    config: Arc<RuntimeConfig>,
    app_server: AppServerBridge,
}

impl ReferenceAnalysisRuntime {
    pub(crate) fn new(config: Arc<RuntimeConfig>, app_server: AppServerBridge) -> Self {
        Self { config, app_server }
    }

    pub(crate) async fn analyze(&self, body: Value) -> Result<String, AnalysisError> {
        if self.app_server.is_closed() {
            return Err(AnalysisError::runtime_closed());
        }
        let _operation = self.app_server.bind_operation();
        let source = ReferenceSource::from_body(body)?;
        let analysis_directory = self
            .config
            .run_dir
            .join("reference-analysis")
            .join(&Uuid::new_v4().to_string()[..8]);

        let result = async {
            fs::create_dir_all(&analysis_directory)
                .await
                .map_err(|error| {
                    AnalysisError::new(format!(
                        "reference analysis directory could not be created: {error}"
                    ))
                })?;

            let staging_directory = analysis_directory.clone();
            let staged_path = tokio::task::spawn_blocking(move || source.stage(&staging_directory))
                .await
                .map_err(|error| {
                    AnalysisError::new(format!("reference image staging failed: {error}"))
                })??;

            let mut shutdown = self.app_server.shutdown_receiver();
            tokio::select! {
                biased;
                _ = wait_for_shutdown(&mut shutdown) => Err(AnalysisError::runtime_closed()),
                result = self.run_app_server_analysis(&staged_path) => result,
            }
        }
        .await;

        let cleanup = fs::remove_dir_all(&analysis_directory).await;
        match cleanup {
            Ok(()) => result,
            Err(error) if error.kind() == io::ErrorKind::NotFound => result,
            Err(error) => Err(AnalysisError::new(format!(
                "reference analysis cleanup failed: {error}"
            ))),
        }
    }

    async fn run_app_server_analysis(&self, staged_path: &Path) -> Result<String, AnalysisError> {
        let client = self
            .app_server
            .ready_client()
            .await
            .map_err(app_server_unavailable)?;
        let workspace = display_path(&self.config.workspace_dir);
        let thread_result = client
            .request(
                "thread/start",
                json!({
                    "cwd": workspace,
                    "approvalPolicy": "never",
                    "sandbox": "read-only",
                    "serviceName": "codex_image_grid_reference_analysis",
                    "ephemeral": true
                }),
                ANALYSIS_TIMEOUT,
            )
            .await
            .map_err(|error| AnalysisError::new(error.message))?;
        let thread_id = thread_result
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AnalysisError::new("thread/start returned no thread id"))?
            .to_owned();

        let mut notifications = client.subscribe();
        let turn_client = client.clone();
        let turn_thread_id = thread_id.clone();
        let turn_workspace = display_path(&self.config.workspace_dir);
        let local_image_path = display_path(staged_path);
        let mut turn_start = tokio::spawn(async move {
            turn_client
                .request(
                    "turn/start",
                    json!({
                        "threadId": turn_thread_id,
                        "input": [
                            {
                                "type": "text",
                                "text": ANALYZE_PROMPT,
                                "text_elements": []
                            },
                            {
                                "type": "localImage",
                                "path": local_image_path
                            }
                        ],
                        "cwd": turn_workspace,
                        "approvalPolicy": "never",
                        "sandboxPolicy": {
                            "type": "readOnly",
                            "networkAccess": false
                        },
                        "effort": "medium"
                    }),
                    ANALYSIS_TIMEOUT,
                )
                .await
        });
        let completion = collect_analysis_notifications(&mut notifications, &thread_id);
        tokio::pin!(completion);

        match timeout(ANALYSIS_TIMEOUT, async {
            tokio::select! {
                result = &mut completion => result,
                turn_result = &mut turn_start => {
                    turn_result
                        .map_err(|error| AnalysisError::new(format!("turn/start task failed: {error}")))?
                        .map_err(|error| AnalysisError::new(error.message))?;
                    completion.await
                }
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(AnalysisError::new("reference analysis timed out")),
        }
    }
}

async fn collect_analysis_notifications(
    notifications: &mut tokio::sync::broadcast::Receiver<Value>,
    thread_id: &str,
) -> Result<String, AnalysisError> {
    let mut text = String::new();
    loop {
        let message = match notifications.recv().await {
            Ok(message) => message,
            Err(RecvError::Lagged(_)) => {
                return Err(AnalysisError::new(
                    "reference analysis notification stream lagged",
                ));
            }
            Err(RecvError::Closed) => {
                return Err(AnalysisError::new(
                    "Codex App Server notification stream closed",
                ));
            }
        };
        let params = message.get("params").unwrap_or(&Value::Null);
        if notification_thread_id(params) != Some(thread_id) {
            continue;
        }

        match message.get("method").and_then(Value::as_str) {
            Some("item/agentMessage/delta") => {
                if let Some(delta) = params.get("delta").and_then(Value::as_str) {
                    text.push_str(delta);
                }
            }
            Some("item/completed") => {
                let item = params.get("item").unwrap_or(&Value::Null);
                if item.get("type").and_then(Value::as_str) == Some("agentMessage")
                    && let Some(completed_text) = item
                        .get("text")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                {
                    text.clear();
                    text.push_str(completed_text);
                }
            }
            Some("turn/completed") => {
                let premise = text.trim();
                if premise.is_empty() {
                    return Err(AnalysisError::new("reference analysis returned no text"));
                }
                return Ok(premise.to_owned());
            }
            Some("error") if params.get("willRetry").and_then(Value::as_bool) != Some(true) => {
                let message = params
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("reference analysis failed");
                return Err(AnalysisError::new(message));
            }
            Some("server-status")
                if matches!(
                    params.get("status").and_then(Value::as_str),
                    Some("error" | "stopped")
                ) =>
            {
                return Err(AnalysisError::new(
                    params
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("Codex App Server stopped"),
                ));
            }
            _ => {}
        }
    }
}

fn notification_thread_id(params: &Value) -> Option<&str> {
    params
        .get("threadId")
        .and_then(Value::as_str)
        .or_else(|| params.pointer("/thread/id").and_then(Value::as_str))
}

enum ReferenceSource {
    LocalPath(PathBuf),
    BrowserUpload {
        data_url: String,
        declared_mime_type: Option<String>,
    },
}

impl ReferenceSource {
    fn from_body(mut body: Value) -> Result<Self, AnalysisError> {
        let object = body
            .as_object_mut()
            .ok_or_else(|| AnalysisError::new("reference image is required"))?;

        if let Some(path_value) = object.remove("referenceImagePath") {
            match path_value {
                Value::String(path) if !path.is_empty() => {
                    return Ok(Self::LocalPath(PathBuf::from(path)));
                }
                Value::String(_) | Value::Null => {}
                _ => {
                    return Err(AnalysisError::new("referenceImagePath must be a string"));
                }
            }
        }

        let Some(Value::Object(mut reference_image)) = object.remove("referenceImage") else {
            return Err(AnalysisError::new("reference image is required"));
        };
        let data_url = match reference_image.remove("dataUrl") {
            Some(Value::String(value)) if !value.is_empty() => value,
            Some(Value::String(_)) | None | Some(Value::Null) => {
                return Err(AnalysisError::new("reference image is required"));
            }
            Some(_) => {
                return Err(AnalysisError::new(
                    "reference image must be PNG, JPEG, or WebP",
                ));
            }
        };
        let declared_mime_type = reference_image
            .remove("mimeType")
            .and_then(|value| value.as_str().map(str::to_owned))
            .filter(|value| is_supported_mime_type(value));

        Ok(Self::BrowserUpload {
            data_url,
            declared_mime_type,
        })
    }

    fn stage(self, analysis_directory: &Path) -> Result<PathBuf, AnalysisError> {
        match self {
            Self::LocalPath(source_path) => stage_reference_image(source_path, analysis_directory)
                .map(|staged| staged.staged_path)
                .map_err(|error| AnalysisError::new(error.to_string())),
            Self::BrowserUpload {
                data_url,
                declared_mime_type,
            } => stage_browser_upload(&data_url, declared_mime_type.as_deref(), analysis_directory),
        }
    }
}

fn stage_browser_upload(
    data_url: &str,
    declared_mime_type: Option<&str>,
    analysis_directory: &Path,
) -> Result<PathBuf, AnalysisError> {
    let (metadata, encoded) = data_url
        .strip_prefix("data:")
        .and_then(|value| value.split_once(','))
        .ok_or_else(invalid_browser_image)?;
    let (mime_type, extension) = match metadata {
        "image/png;base64" => ("image/png", "png"),
        "image/jpeg;base64" => ("image/jpeg", "jpg"),
        "image/webp;base64" => ("image/webp", "webp"),
        _ => return Err(invalid_browser_image()),
    };
    if declared_mime_type.is_some_and(|declared| declared != mime_type) {
        return Err(invalid_browser_image());
    }
    if encoded.is_empty()
        || !encoded
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
    {
        return Err(invalid_browser_image());
    }

    let bytes = BASE64_STANDARD
        .decode(encoded)
        .map_err(|_| invalid_browser_image())?;
    if bytes.len() as u64 > MAX_REFERENCE_IMAGE_BYTES {
        return Err(AnalysisError::new(
            "reference image is too large; keep it under 100 MB",
        ));
    }

    let output_path = analysis_directory.join(format!("reference.{extension}"));
    std_fs::write(&output_path, bytes).map_err(|error| {
        AnalysisError::new(format!(
            "reference image could not be staged at {} ({error})",
            output_path.display()
        ))
    })?;
    std_fs::canonicalize(&output_path).map_err(|error| {
        AnalysisError::new(format!(
            "reference image could not be staged at {} ({error})",
            output_path.display()
        ))
    })
}

fn invalid_browser_image() -> AnalysisError {
    AnalysisError::new("reference image must be PNG, JPEG, or WebP")
}

fn is_supported_mime_type(value: &str) -> bool {
    matches!(value, "image/png" | "image/jpeg" | "image/webp")
}

fn app_server_unavailable(diagnostics: AppServerDiagnostics) -> AnalysisError {
    match diagnostics.error {
        Some(error) if error.code == "RuntimeClosed" => AnalysisError::runtime_closed(),
        Some(error) => AnalysisError::new(error.message),
        None => AnalysisError::new("Codex App Server is unavailable"),
    }
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[derive(Debug)]
pub(crate) struct AnalysisError {
    message: String,
    code: Option<&'static str>,
    status: StatusCode,
}

impl AnalysisError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
            status: StatusCode::BAD_REQUEST,
        }
    }

    fn runtime_closed() -> Self {
        Self {
            message: RUNTIME_CLOSED_MESSAGE.to_owned(),
            code: Some("RuntimeClosed"),
            status: StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}

impl IntoResponse for AnalysisError {
    fn into_response(self) -> Response {
        let mut body = json!({ "error": self.message });
        if let Some(code) = self.code {
            body["code"] = Value::String(code.to_owned());
        }
        (self.status, Json(body)).into_response()
    }
}

async fn wait_for_shutdown(receiver: &mut tokio::sync::watch::Receiver<bool>) {
    if *receiver.borrow() {
        return;
    }
    while receiver.changed().await.is_ok() {
        if *receiver.borrow() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_browser_payload_stages_bytes_and_rejects_empty_decoding() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let expected = b"browser reference bytes";
        let source = ReferenceSource::from_body(json!({
            "referenceImage": {
                "dataUrl": format!(
                    "data:image/png;base64,{}",
                    BASE64_STANDARD.encode(expected)
                ),
                "mimeType": "image/png",
                "name": "reference.png",
                "size": expected.len()
            }
        }))
        .expect("valid browser payload");

        let staged = source
            .stage(temporary.path())
            .expect("stage browser payload");
        assert_eq!(std_fs::read(staged).expect("read staged image"), expected);

        let error = stage_browser_upload(
            "data:image/png;base64,",
            Some("image/png"),
            temporary.path(),
        )
        .expect_err("empty encoded image must be rejected");
        assert_eq!(error.message, "reference image must be PNG, JPEG, or WebP");
    }

    #[test]
    fn browser_payload_only_enforces_a_supported_declared_mime_type() {
        let encoded = BASE64_STANDARD.encode(b"browser reference bytes");
        let data_url = format!("data:image/png;base64,{encoded}");
        for payload in [
            json!({"referenceImage": {"dataUrl": &data_url}}),
            json!({"referenceImage": {"dataUrl": &data_url, "mimeType": "image/gif"}}),
            json!({"referenceImage": {"dataUrl": &data_url, "mimeType": 1}}),
        ] {
            let temporary = tempfile::tempdir().expect("temporary directory");
            ReferenceSource::from_body(payload)
                .and_then(|source| source.stage(temporary.path()))
                .expect("missing or unsupported MIME is ignored by the frozen server");
        }

        let temporary = tempfile::tempdir().expect("temporary directory");
        let error = ReferenceSource::from_body(json!({
            "referenceImage": {
                "dataUrl": &data_url,
                "mimeType": "image/jpeg"
            }
        }))
        .and_then(|source| source.stage(temporary.path()))
        .expect_err("a supported declared MIME must match the data URL");
        assert_eq!(error.message, "reference image must be PNG, JPEG, or WebP");
    }
}
