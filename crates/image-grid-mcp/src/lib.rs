use image_grid_core::{
    MAX_PROMPTS, MAX_RUN_JOBS, MAX_VARIANTS_PER_PROMPT, MAX_WAIT_MS, validate_reference_image,
};
use serde_json::{Value, json};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
pub const TOOL_NAME: &str = "generate_image_grid";

const TOOL_DESCRIPTION: &str = "Generate project-ready image variants from Prompt Batch input. \
Auto-launches the local Image Grid app or web server when possible, then returns handoff.md, \
absolute output paths, display-safe image URLs, and Codex Markdown.";
const SERVER_INSTRUCTIONS: &str = "Use generate_image_grid when the user needs project-specific \
thumbnails, visual variants, or Prompt Batch image generation. Return and reuse handoff.md, \
absolute output paths, imageUrls, and codexMarkdown.";
const DEFAULT_IMAGE_GRID_URL: &str = "http://127.0.0.1:4322";
const EXPECTED_APP_IDENTITY: &str = "codex-image-grid-native";
const MAX_JSON_RESPONSE_BYTES: u64 = 5 * 1024 * 1024;

pub fn serve<R: BufRead, W: Write>(reader: R, mut writer: W) -> io::Result<()> {
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle_input(&message),
            Err(error) => Some(rpc_error(
                Value::Null,
                -32700,
                format!("parse error: {error}"),
            )),
        };
        if let Some(response) = response {
            serde_json::to_writer(&mut writer, &response).map_err(io::Error::other)?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
    }
    Ok(())
}

pub fn handle_input(message: &Value) -> Option<Value> {
    if let Some(batch) = message.as_array() {
        let responses: Vec<Value> = batch.iter().filter_map(handle_request).collect();
        return (!responses.is_empty()).then_some(Value::Array(responses));
    }
    handle_request(message)
}

fn handle_request(message: &Value) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();

    match method {
        "initialize" => Some(rpc_result(id, initialize_result(message.get("params")))),
        "ping" => Some(rpc_result(id, json!({}))),
        "tools/list" => Some(rpc_result(id, json!({ "tools": [tool_record()] }))),
        "tools/call" => Some(handle_tool_call(id, message.get("params"))),
        _ => Some(rpc_error(id, -32601, format!("unknown method: {method}"))),
    }
}

fn initialize_result(params: Option<&Value>) -> Value {
    let requested_protocol = params
        .and_then(|value| value.get("protocolVersion"))
        .filter(|value| javascript_truthy(value))
        .cloned()
        .unwrap_or_else(|| Value::String(MCP_PROTOCOL_VERSION.to_owned()));

    json!({
        "protocolVersion": requested_protocol,
        "capabilities": {
            "tools": {
                "listChanged": false
            }
        },
        "serverInfo": {
            "name": "codex-image-grid-native",
            "title": "Codex Image Grid Native",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": SERVER_INSTRUCTIONS
    })
}

fn handle_tool_call(id: Value, params: Option<&Value>) -> Value {
    let name = params
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if name != TOOL_NAME {
        let displayed_name = if name.is_empty() { "(missing)" } else { name };
        return rpc_error(id, -32602, format!("unknown tool: {displayed_name}"));
    }

    let arguments = params
        .and_then(|value| value.get("arguments"))
        .unwrap_or(&Value::Null);
    let result = match call_generate_image_grid(arguments) {
        Ok(result) => result,
        Err(message) => tool_error(message),
    };
    rpc_result(id, result)
}

#[derive(Debug, Clone)]
struct NormalizedArguments {
    prompts: Vec<String>,
    count: usize,
    mood: String,
    engine: String,
    aspect_ratio: String,
    reference_premise: Option<String>,
    reference_image_path: Option<String>,
    wait_ms: u64,
}

#[derive(Debug, Clone)]
struct BridgeConfig {
    endpoint: HttpEndpoint,
    image_grid_url: String,
    app_dir: Option<PathBuf>,
    launch_plan: Option<LaunchPlan>,
    launch_timeout: Duration,
    health_timeout: Duration,
    preflight_timeout: Duration,
    run_timeout: Duration,
    launch_probe: Duration,
}

impl BridgeConfig {
    fn from_environment() -> Result<Self, String> {
        let image_grid_url = first_nonempty_env(&["IMAGE_GRID_URL", "IMAGE_GRID_NATIVE_URL"])
            .unwrap_or_else(|| DEFAULT_IMAGE_GRID_URL.to_owned());
        let endpoint = HttpEndpoint::parse(&image_grid_url)?;
        let app_dir = first_nonempty_env(&["IMAGE_GRID_APP_DIR", "IMAGE_GRID_NATIVE_APP_DIR"])
            .map(PathBuf::from)
            .map(|path| canonical_directory(&path, "IMAGE_GRID_APP_DIR"))
            .transpose()?;

        let start_command = first_nonempty_env(&[
            "IMAGE_GRID_START_COMMAND",
            "IMAGE_GRID_NATIVE_START_COMMAND",
        ]);
        let native_server_bin = first_nonempty_env(&["IMAGE_GRID_NATIVE_SERVER_BIN"]);
        let launch_plan = if let Some(command) = start_command {
            let cwd = app_dir.clone().ok_or_else(|| {
                "IMAGE_GRID_START_COMMAND requires IMAGE_GRID_APP_DIR; refusing an unscoped native launch"
                    .to_owned()
            })?;
            Some(LaunchPlan {
                label: "IMAGE_GRID_START_COMMAND".to_owned(),
                program: PathBuf::from("/bin/sh"),
                arguments: vec!["-lc".to_owned(), command],
                cwd: Some(cwd),
                environment: Vec::new(),
            })
        } else if let Some(binary) = native_server_bin {
            let binary = resolve_executable(&PathBuf::from(binary))?;
            Some(LaunchPlan {
                label: "IMAGE_GRID_NATIVE_SERVER_BIN".to_owned(),
                program: binary,
                arguments: Vec::new(),
                cwd: app_dir.clone(),
                environment: Vec::new(),
            })
        } else {
            None
        };

        Ok(Self {
            endpoint,
            image_grid_url: image_grid_url.trim_end_matches('/').to_owned(),
            app_dir,
            launch_plan,
            launch_timeout: bounded_env_duration(
                "IMAGE_GRID_LAUNCH_TIMEOUT_MS",
                15_000,
                1_000,
                60_000,
            ),
            health_timeout: bounded_env_duration(
                "IMAGE_GRID_HEALTH_REQUEST_TIMEOUT_MS",
                1_000,
                100,
                10_000,
            ),
            preflight_timeout: bounded_env_duration(
                "IMAGE_GRID_PREFLIGHT_TIMEOUT_MS",
                20_000,
                1_000,
                60_000,
            ),
            run_timeout: bounded_env_duration(
                "IMAGE_GRID_RUN_REQUEST_TIMEOUT_MS",
                150_000,
                1_000,
                180_000,
            ),
            launch_probe: bounded_env_duration("IMAGE_GRID_LAUNCH_PROBE_MS", 250, 25, 2_000),
        })
    }
}

#[derive(Debug, Clone)]
struct LaunchPlan {
    label: String,
    program: PathBuf,
    arguments: Vec<String>,
    cwd: Option<PathBuf>,
    environment: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
struct ServerStartup {
    started: bool,
    launch_plan: Option<String>,
    health: Value,
}

#[derive(Debug, Clone)]
struct HttpEndpoint {
    host: String,
    host_header: String,
    port: u16,
    base_path: String,
}

impl HttpEndpoint {
    fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        let remainder = value.strip_prefix("http://").ok_or_else(|| {
            "IMAGE_GRID_URL must use http:// and address a loopback host".to_owned()
        })?;
        if remainder.contains(['?', '#', '@']) {
            return Err(
                "IMAGE_GRID_URL must not contain credentials, a query, or a fragment".to_owned(),
            );
        }
        let (authority, path) = remainder
            .split_once('/')
            .map(|(authority, path)| (authority, format!("/{path}")))
            .unwrap_or((remainder, String::new()));
        if authority.is_empty() {
            return Err("IMAGE_GRID_URL is missing a host".to_owned());
        }

        let (host, port) = if let Some(ipv6) = authority.strip_prefix('[') {
            let close = ipv6
                .find(']')
                .ok_or_else(|| "IMAGE_GRID_URL contains an invalid IPv6 host".to_owned())?;
            let host = &ipv6[..close];
            let suffix = &ipv6[close + 1..];
            let port = suffix
                .strip_prefix(':')
                .filter(|value| !value.is_empty())
                .unwrap_or("80")
                .parse::<u16>()
                .map_err(|_| "IMAGE_GRID_URL contains an invalid port".to_owned())?;
            (host.to_owned(), port)
        } else {
            let mut parts = authority.rsplitn(2, ':');
            let possible_port = parts.next().unwrap_or_default();
            let possible_host = parts.next();
            match possible_host {
                Some(host) if !host.contains(':') => (
                    host.to_owned(),
                    possible_port
                        .parse::<u16>()
                        .map_err(|_| "IMAGE_GRID_URL contains an invalid port".to_owned())?,
                ),
                _ => (authority.to_owned(), 80),
            }
        };
        if host != "localhost"
            && host
                .parse::<IpAddr>()
                .map(|address| !address.is_loopback())
                .unwrap_or(true)
        {
            return Err("IMAGE_GRID_URL must address localhost or a loopback IP".to_owned());
        }
        let base_path = path.trim_end_matches('/').to_owned();
        let host_header = if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };
        Ok(Self {
            host,
            host_header,
            port,
            base_path,
        })
    }

    fn request_path(&self, path: &str) -> String {
        format!("{}{}", self.base_path, path)
    }

    fn connect_address(&self) -> Result<SocketAddr, String> {
        (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|error| format!("could not resolve Image Grid loopback address: {error}"))?
            .find(|address| address.ip().is_loopback())
            .ok_or_else(|| "IMAGE_GRID_URL did not resolve to a loopback address".to_owned())
    }
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    data: Value,
}

fn call_generate_image_grid(arguments: &Value) -> Result<Value, String> {
    validate_tool_arguments(arguments)?;
    let normalized = normalize_tool_arguments(arguments);
    let config = BridgeConfig::from_environment()?;
    call_generate_image_grid_with_config(&normalized, &config)
}

fn call_generate_image_grid_with_config(
    arguments: &NormalizedArguments,
    config: &BridgeConfig,
) -> Result<Value, String> {
    let mut server = start_or_join_native_server(config)?;
    server.health = assert_engine_ready(config, server.health, &arguments.engine)?;

    let mut body = serde_json::Map::new();
    body.insert("prompts".to_owned(), json!(arguments.prompts));
    body.insert("count".to_owned(), json!(arguments.count));
    body.insert("mood".to_owned(), json!(arguments.mood));
    body.insert("engine".to_owned(), json!(arguments.engine));
    body.insert("aspectRatio".to_owned(), json!(arguments.aspect_ratio));
    body.insert("waitMs".to_owned(), json!(arguments.wait_ms));
    if let Some(reference_premise) = &arguments.reference_premise {
        body.insert(
            "referencePremise".to_owned(),
            Value::String(reference_premise.clone()),
        );
    }
    if let Some(reference_image_path) = &arguments.reference_image_path {
        body.insert(
            "referenceImagePath".to_owned(),
            Value::String(reference_image_path.clone()),
        );
    }

    let request_timeout = config.run_timeout.max(Duration::from_millis(
        arguments.wait_ms.saturating_add(1_000),
    ));
    let response = http_json(
        &config.endpoint,
        "POST",
        "/api/run-batch",
        Some(&Value::Object(body)),
        request_timeout,
    )
    .map_err(|error| format!("Image Grid run submission failed: {error}"))?;
    if !(200..300).contains(&response.status) {
        return Err(response
            .data
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Image Grid HTTP {}", response.status)));
    }
    if !response.data.is_object() {
        return Err("Image Grid run submission returned an invalid JSON object".to_owned());
    }
    Ok(render_tool_result(
        &response.data,
        &server,
        &config.image_grid_url,
    ))
}

fn normalize_tool_arguments(arguments: &Value) -> NormalizedArguments {
    NormalizedArguments {
        prompts: arguments["prompts"]
            .as_array()
            .expect("validated prompts")
            .iter()
            .map(|value| value.as_str().expect("validated prompt").to_owned())
            .collect(),
        count: arguments.get("count").and_then(json_integer).unwrap_or(1) as usize,
        mood: arguments
            .get("mood")
            .and_then(Value::as_str)
            .unwrap_or("warm-mascot")
            .to_owned(),
        engine: arguments
            .get("engine")
            .and_then(Value::as_str)
            .unwrap_or("app-server-image")
            .to_owned(),
        aspect_ratio: arguments
            .get("aspectRatio")
            .and_then(Value::as_str)
            .unwrap_or("16:9")
            .to_owned(),
        reference_premise: arguments
            .get("referencePremise")
            .and_then(Value::as_str)
            .map(str::to_owned),
        reference_image_path: arguments
            .get("referenceImagePath")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        wait_ms: arguments.get("waitMs").and_then(json_integer).unwrap_or(0) as u64,
    }
}

fn start_or_join_native_server(config: &BridgeConfig) -> Result<ServerStartup, String> {
    let deadline = Instant::now() + config.launch_timeout;
    match check_health(config, deadline)? {
        Some(health) => {
            return Ok(ServerStartup {
                started: false,
                launch_plan: None,
                health,
            });
        }
        None => {}
    }

    let launch_plan = config.launch_plan.as_ref().ok_or_else(|| {
        "Image Grid Native is not running, and no native auto-launch target is configured. Set IMAGE_GRID_APP_DIR with IMAGE_GRID_START_COMMAND, or set IMAGE_GRID_NATIVE_SERVER_BIN."
            .to_owned()
    })?;
    let lock_path = startup_lock_path(&config.endpoint);
    let lock = StartupLock::acquire(&lock_path)?;
    if lock.is_none() {
        let health = wait_for_joined_health(config, deadline)?;
        return Ok(ServerStartup {
            started: false,
            launch_plan: None,
            health,
        });
    }
    let _lock = lock;

    if let Some(health) = check_health(config, deadline)? {
        return Ok(ServerStartup {
            started: false,
            launch_plan: None,
            health,
        });
    }

    let mut child = launch_native_server(launch_plan)?;
    let probe_deadline = deadline.min(Instant::now() + config.launch_probe);
    while Instant::now() < probe_deadline {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("could not inspect native launch process: {error}"))?
        {
            if !status.success() {
                return Err(format!(
                    "Image Grid Native launch command failed early with {status}: {}",
                    launch_plan.label
                ));
            }
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }

    loop {
        if let Some(health) = check_health(config, deadline)? {
            return Ok(ServerStartup {
                started: true,
                launch_plan: Some(launch_plan.label.clone()),
                health,
            });
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("could not inspect native launch process: {error}"))?
        {
            return Err(format!(
                "Image Grid Native launch command exited before health became ready ({status}): {}",
                launch_plan.label
            ));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "Image Grid Native auto-launch did not become healthy at {} within {} ms",
                config.image_grid_url,
                config.launch_timeout.as_millis()
            ));
        }
        thread::sleep(Duration::from_millis(100).min(remaining_duration(deadline)));
    }
}

fn wait_for_joined_health(config: &BridgeConfig, deadline: Instant) -> Result<Value, String> {
    loop {
        if let Some(health) = check_health(config, deadline)? {
            return Ok(health);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "Timed out waiting for the shared Image Grid Native startup at {}",
                config.image_grid_url
            ));
        }
        thread::sleep(Duration::from_millis(100).min(remaining_duration(deadline)));
    }
}

fn launch_native_server(plan: &LaunchPlan) -> Result<Child, String> {
    let mut command = Command::new(&plan.program);
    command
        .args(&plan.arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(cwd) = &plan.cwd {
        command.current_dir(cwd);
    }
    for (name, value) in &plan.environment {
        command.env(name, value);
    }
    command.spawn().map_err(|error| {
        format!(
            "could not launch Image Grid Native with {}: {error}",
            plan.label
        )
    })
}

fn assert_engine_ready(
    config: &BridgeConfig,
    mut health: Value,
    engine: &str,
) -> Result<Value, String> {
    if engine != "app-server-image" {
        return Ok(health);
    }
    let response = http_json(
        &config.endpoint,
        "POST",
        "/api/preflight/app-server-image",
        None,
        config.preflight_timeout,
    )
    .map_err(|error| {
        format!(
            "Image Grid server is running, but App Server image generation is not ready. Preflight request failed: {error}"
        )
    })?;
    let ready = response.status == 200
        && response.data.get("ok").and_then(Value::as_bool) == Some(true)
        && response.data.get("appServerImage").and_then(Value::as_bool) == Some(true);
    if !ready {
        return Err(app_server_diagnostic_message(&response.data));
    }
    if let Some(object) = health.as_object_mut() {
        object.insert("appServerImage".to_owned(), Value::Bool(true));
        object.insert("appServerImageReady".to_owned(), Value::Bool(true));
        if let Some(diagnostics) = response.data.get("diagnostics") {
            object.insert("appServerImageDiagnostics".to_owned(), diagnostics.clone());
        }
    }
    Ok(health)
}

fn app_server_diagnostic_message(data: &Value) -> String {
    let diagnostics = data
        .get("diagnostics")
        .or_else(|| data.get("appServerImageDiagnostics"))
        .unwrap_or(&Value::Null);
    let mut message =
        "Image Grid server is running, but App Server image generation is not ready.".to_owned();
    if let Some(command) = diagnostics.get("selectedCommand").and_then(Value::as_str) {
        let source = diagnostics
            .get("selectedSource")
            .and_then(Value::as_str)
            .unwrap_or("unknown source");
        message.push_str(&format!(" Selected command: {command} ({source})."));
    }
    if let Some(failure) = diagnostics
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
    {
        message.push_str(&format!(" Preflight failure: {failure}."));
    }
    let candidate_details = diagnostics
        .get("candidates")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|candidate| {
            let status = candidate.get("status").and_then(Value::as_str)?;
            if !matches!(status, "rejected" | "unavailable" | "skipped") {
                return None;
            }
            let source = candidate
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("unknown source");
            let command = candidate
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("(none)");
            let reason = candidate
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("no reason reported");
            Some(format!("{status} {source}={command}: {reason}"))
        })
        .collect::<Vec<_>>();
    if !candidate_details.is_empty() {
        message.push_str(&format!(
            " Candidate diagnostics: {}.",
            candidate_details.join("; ")
        ));
    }
    message
}

fn check_health(config: &BridgeConfig, deadline: Instant) -> Result<Option<Value>, String> {
    if Instant::now() >= deadline {
        return Ok(None);
    }
    let timeout = config.health_timeout.min(remaining_duration(deadline));
    let response = match http_json(&config.endpoint, "GET", "/api/health", None, timeout) {
        Ok(response) => response,
        Err(HttpRequestError::Unavailable(_)) | Err(HttpRequestError::Timeout(_)) => {
            return Ok(None);
        }
        Err(error) => return Err(format!("Image Grid health request failed: {error}")),
    };
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "Image Grid health endpoint returned HTTP {}; refusing to launch over a foreign listener",
            response.status
        ));
    }
    validate_health_payload(&response.data, config.app_dir.as_deref())?;
    Ok(Some(response.data))
}

fn validate_health_payload(health: &Value, expected_root: Option<&Path>) -> Result<(), String> {
    let object = health
        .as_object()
        .ok_or_else(|| "health response was not a JSON object".to_owned())?;
    if object.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err("health response did not report ok=true".to_owned());
    }
    let app = object
        .get("app")
        .and_then(Value::as_str)
        .or_else(|| {
            object
                .get("identity")
                .and_then(|identity| identity.get("app"))
                .and_then(Value::as_str)
        })
        .unwrap_or_default();
    if app != EXPECTED_APP_IDENTITY {
        return Err(format!(
            "health response app identity was {}; expected {:?}; refusing the foreign runtime",
            if app.is_empty() {
                "null".to_owned()
            } else {
                format!("{app:?}")
            },
            EXPECTED_APP_IDENTITY
        ));
    }
    let reported_root = object
        .get("serverRoot")
        .and_then(Value::as_str)
        .or_else(|| {
            object
                .get("identity")
                .and_then(|identity| identity.get("serverRoot"))
                .and_then(Value::as_str)
        })
        .filter(|root| !root.is_empty())
        .ok_or_else(|| "health response did not include serverRoot".to_owned())?;
    if let Some(expected_root) = expected_root {
        let actual_root = fs::canonicalize(reported_root).map_err(|error| {
            format!("health serverRoot is unavailable: {reported_root} ({error})")
        })?;
        if actual_root != expected_root {
            return Err(format!(
                "Image Grid Native serverRoot mismatch: reported {}; expected IMAGE_GRID_APP_DIR={}",
                actual_root.display(),
                expected_root.display()
            ));
        }
    }
    Ok(())
}

#[derive(Debug)]
enum HttpRequestError {
    Unavailable(String),
    Timeout(String),
    Protocol(String),
}

impl std::fmt::Display for HttpRequestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(message) | Self::Timeout(message) | Self::Protocol(message) => {
                formatter.write_str(message)
            }
        }
    }
}

fn http_json(
    endpoint: &HttpEndpoint,
    method: &str,
    path: &str,
    body: Option<&Value>,
    timeout: Duration,
) -> Result<HttpResponse, HttpRequestError> {
    let address = endpoint
        .connect_address()
        .map_err(HttpRequestError::Protocol)?;
    let mut stream = TcpStream::connect_timeout(&address, timeout).map_err(|error| {
        if error.kind() == io::ErrorKind::TimedOut {
            HttpRequestError::Timeout(format!(
                "request timed out after {} ms",
                timeout.as_millis()
            ))
        } else {
            HttpRequestError::Unavailable(error.to_string())
        }
    })?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| HttpRequestError::Protocol(error.to_string()))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| HttpRequestError::Protocol(error.to_string()))?;
    let body = body
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|error| HttpRequestError::Protocol(error.to_string()))?
        .unwrap_or_default();
    let request_path = endpoint.request_path(path);
    write!(
        stream,
        "{method} {request_path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        endpoint.host_header,
        body.len()
    )
    .and_then(|_| stream.write_all(&body))
    .and_then(|_| stream.flush())
    .map_err(|error| {
        if error.kind() == io::ErrorKind::TimedOut {
            HttpRequestError::Timeout(format!("request timed out after {} ms", timeout.as_millis()))
        } else {
            HttpRequestError::Unavailable(error.to_string())
        }
    })?;

    let mut bytes = Vec::new();
    stream
        .take(MAX_JSON_RESPONSE_BYTES + 64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            if error.kind() == io::ErrorKind::TimedOut {
                HttpRequestError::Timeout(format!(
                    "request timed out after {} ms",
                    timeout.as_millis()
                ))
            } else {
                HttpRequestError::Protocol(error.to_string())
            }
        })?;
    parse_http_json_response(&bytes)
}

fn parse_http_json_response(bytes: &[u8]) -> Result<HttpResponse, HttpRequestError> {
    let header_end = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| {
            HttpRequestError::Protocol("HTTP response had no header boundary".to_owned())
        })?;
    let header = std::str::from_utf8(&bytes[..header_end]).map_err(|error| {
        HttpRequestError::Protocol(format!("HTTP headers were not UTF-8: {error}"))
    })?;
    let mut lines = header.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| HttpRequestError::Protocol("HTTP response had no status line".to_owned()))?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| {
            HttpRequestError::Protocol("HTTP response had an invalid status".to_owned())
        })?;
    let mut content_length = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse::<usize>().ok();
        } else if name.eq_ignore_ascii_case("transfer-encoding")
            && value
                .split(',')
                .any(|encoding| encoding.trim().eq_ignore_ascii_case("chunked"))
        {
            chunked = true;
        }
    }
    let raw_body = &bytes[header_end + 4..];
    let body = if chunked {
        decode_chunked(raw_body)?
    } else if let Some(content_length) = content_length {
        if raw_body.len() < content_length {
            return Err(HttpRequestError::Protocol(
                "HTTP response body ended before Content-Length".to_owned(),
            ));
        }
        raw_body[..content_length].to_vec()
    } else {
        raw_body.to_vec()
    };
    if body.len() as u64 > MAX_JSON_RESPONSE_BYTES {
        return Err(HttpRequestError::Protocol(format!(
            "JSON response body exceeds {MAX_JSON_RESPONSE_BYTES} bytes"
        )));
    }
    let data = if body.iter().all(u8::is_ascii_whitespace) {
        Value::Null
    } else {
        serde_json::from_slice(&body).map_err(|error| {
            HttpRequestError::Protocol(format!("server returned invalid JSON: {error}"))
        })?
    };
    Ok(HttpResponse { status, data })
}

fn decode_chunked(bytes: &[u8]) -> Result<Vec<u8>, HttpRequestError> {
    let mut cursor = 0;
    let mut decoded = Vec::new();
    loop {
        let line_end = bytes[cursor..]
            .windows(2)
            .position(|window| window == b"\r\n")
            .map(|offset| cursor + offset)
            .ok_or_else(|| HttpRequestError::Protocol("invalid chunked response".to_owned()))?;
        let size_text = std::str::from_utf8(&bytes[cursor..line_end])
            .map_err(|_| HttpRequestError::Protocol("invalid chunk size".to_owned()))?
            .split(';')
            .next()
            .unwrap_or_default();
        let size = usize::from_str_radix(size_text.trim(), 16)
            .map_err(|_| HttpRequestError::Protocol("invalid chunk size".to_owned()))?;
        cursor = line_end + 2;
        if size == 0 {
            break;
        }
        let chunk_end = cursor
            .checked_add(size)
            .filter(|end| end + 2 <= bytes.len())
            .ok_or_else(|| HttpRequestError::Protocol("truncated chunked response".to_owned()))?;
        decoded.extend_from_slice(&bytes[cursor..chunk_end]);
        if decoded.len() as u64 > MAX_JSON_RESPONSE_BYTES {
            return Err(HttpRequestError::Protocol(format!(
                "JSON response body exceeds {MAX_JSON_RESPONSE_BYTES} bytes"
            )));
        }
        if &bytes[chunk_end..chunk_end + 2] != b"\r\n" {
            return Err(HttpRequestError::Protocol(
                "invalid chunk boundary".to_owned(),
            ));
        }
        cursor = chunk_end + 2;
    }
    Ok(decoded)
}

struct StartupLock {
    path: PathBuf,
}

impl StartupLock {
    fn acquire(path: &Path) -> Result<Option<Self>, String> {
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut file) => {
                writeln!(file, "pid={}", std::process::id())
                    .map_err(|error| format!("could not write native startup lock: {error}"))?;
                Ok(Some(Self {
                    path: path.to_path_buf(),
                }))
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(None),
            Err(error) => Err(format!("could not acquire native startup lock: {error}")),
        }
    }
}

impl Drop for StartupLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn startup_lock_path(endpoint: &HttpEndpoint) -> PathBuf {
    let host = endpoint
        .host
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    env::temp_dir().join(format!(
        "codex-image-grid-native-mcp-{host}-{}.lock",
        endpoint.port
    ))
}

fn render_tool_result(data: &Value, server: &ServerStartup, base_url: &str) -> Value {
    let outputs = data
        .get("outputs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let output_source = if outputs.is_empty() {
        data.get("jobs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    } else {
        outputs.clone()
    };
    let output_paths = output_source
        .iter()
        .filter_map(|output| output.get("outputPath").and_then(Value::as_str))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let image_urls = outputs
        .iter()
        .filter_map(|output| output_image_url(output, base_url))
        .collect::<Vec<_>>();
    let markdown_outputs = outputs
        .iter()
        .filter_map(|output| output_markdown(output, base_url))
        .collect::<Vec<_>>();
    let codex_markdown = markdown_outputs.join("\n");
    let diagnostics = data
        .get("diagnostics")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            outputs
                .iter()
                .filter(|output| {
                    output
                        .get("errorCode")
                        .is_some_and(|value| !value.is_null())
                        || output
                            .get("errorMessage")
                            .is_some_and(|value| !value.is_null())
                        || output
                            .get("diagnosticLog")
                            .is_some_and(|value| !value.is_null())
                })
                .cloned()
                .collect()
        });

    let run_id = string_field(data, "runId").unwrap_or_default();
    let status = string_field(data, "status").unwrap_or("queued");
    let status_url = absolute_url(base_url, data.get("statusUrl"));
    let manifest_path = string_field(data, "manifestPath").unwrap_or_default();
    let handoff_path = string_field(data, "handoffPath").unwrap_or_default();
    let mut summary = vec![
        format!("runId: {run_id}"),
        format!("status: {status}"),
        format!("serverStarted: {}", server.started),
    ];
    if let Some(launch_plan) = &server.launch_plan {
        summary.push(format!("launchPlan: {launch_plan}"));
    }
    summary.extend([
        format!("statusUrl: {}", status_url.as_deref().unwrap_or("")),
        format!("manifestPath: {manifest_path}"),
        format!("handoffPath: {handoff_path}"),
        "outputPaths:".to_owned(),
    ]);
    summary.extend(
        output_paths
            .iter()
            .map(|output_path| format!("- {output_path}")),
    );
    summary.push("imageUrls:".to_owned());
    if image_urls.is_empty() {
        summary.push("- none yet; check statusUrl or handoffPath after completion".to_owned());
    } else {
        summary.extend(image_urls.iter().map(|image_url| format!("- {image_url}")));
    }
    summary.push("diagnostics:".to_owned());
    if diagnostics.is_empty() {
        summary.push("- none".to_owned());
    } else {
        summary.extend(diagnostics.iter().map(|entry| {
            let code = string_field(entry, "errorCode")
                .or_else(|| string_field(entry, "upstreamStatus"))
                .unwrap_or("diagnostic");
            let message = string_field(entry, "errorMessage")
                .or_else(|| string_field(entry, "statusText"))
                .or_else(|| string_field(entry, "diagnosticLog"))
                .unwrap_or("see manifest");
            format!("- {} {code}: {message}", output_label(entry))
        }));
    }
    summary.push("codexMarkdown:".to_owned());
    summary.push(if codex_markdown.is_empty() {
        "none yet; image markdown is available after outputs finish".to_owned()
    } else {
        codex_markdown.clone()
    });

    let structured_outputs = outputs
        .iter()
        .map(|output| {
            let mut output = output.clone();
            if let Some(object) = output.as_object_mut() {
                object.insert(
                    "absoluteImageUrl".to_owned(),
                    output_image_url(&Value::Object(object.clone()), base_url)
                        .map(Value::String)
                        .unwrap_or(Value::Null),
                );
                object.insert(
                    "codexMarkdown".to_owned(),
                    output_markdown(&Value::Object(object.clone()), base_url)
                        .map(Value::String)
                        .unwrap_or(Value::Null),
                );
            }
            output
        })
        .collect::<Vec<_>>();
    let mut structured = serde_json::Map::new();
    for (name, value) in [
        ("runId", data.get("runId").cloned().unwrap_or(Value::Null)),
        ("status", data.get("status").cloned().unwrap_or(Value::Null)),
        (
            "completed",
            data.get("completed").cloned().unwrap_or(Value::Null),
        ),
        ("serverStarted", Value::Bool(server.started)),
        ("health", server.health.clone()),
        ("server", data.get("server").cloned().unwrap_or(Value::Null)),
        (
            "statusUrl",
            status_url.map(Value::String).unwrap_or(Value::Null),
        ),
        (
            "manifestUrl",
            absolute_url(base_url, data.get("manifestUrl"))
                .map(Value::String)
                .unwrap_or(Value::Null),
        ),
        (
            "manifestViewUrl",
            absolute_url(base_url, data.get("manifestViewUrl"))
                .map(Value::String)
                .unwrap_or(Value::Null),
        ),
        (
            "handoffUrl",
            absolute_url(base_url, data.get("handoffUrl"))
                .map(Value::String)
                .unwrap_or(Value::Null),
        ),
        (
            "handoffViewUrl",
            absolute_url(base_url, data.get("handoffViewUrl"))
                .map(Value::String)
                .unwrap_or(Value::Null),
        ),
        (
            "manifestPath",
            data.get("manifestPath").cloned().unwrap_or(Value::Null),
        ),
        (
            "handoffPath",
            data.get("handoffPath").cloned().unwrap_or(Value::Null),
        ),
        ("outputPaths", json!(output_paths)),
        ("imageUrls", json!(image_urls)),
        ("codexMarkdown", Value::String(codex_markdown)),
        ("outputs", Value::Array(structured_outputs)),
        ("diagnostics", Value::Array(diagnostics)),
        ("counts", data.get("counts").cloned().unwrap_or(Value::Null)),
    ] {
        structured.insert(name.to_owned(), value);
    }
    if let Some(launch_plan) = &server.launch_plan {
        structured.insert("launchPlan".to_owned(), Value::String(launch_plan.clone()));
    }
    json!({
        "content": [{
            "type": "text",
            "text": summary.join("\n")
        }],
        "structuredContent": Value::Object(structured),
        "isError": false
    })
}

fn output_label(output: &Value) -> String {
    format!(
        "prompt {}/{} variant {}/{}",
        integer_field(output, "promptIndex").unwrap_or(1),
        integer_field(output, "promptTotal").unwrap_or(1),
        integer_field(output, "variant").unwrap_or(1),
        integer_field(output, "total").unwrap_or(1)
    )
}

fn output_image_url(output: &Value, base_url: &str) -> Option<String> {
    absolute_url(base_url, output.get("imageUrl"))
}

fn output_markdown(output: &Value, base_url: &str) -> Option<String> {
    let image_url = output_image_url(output, base_url)?
        .replace('(', "%28")
        .replace(')', "%29");
    let alt = output_label(output)
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]");
    Some(format!("![{alt}]({image_url})"))
}

fn absolute_url(base_url: &str, value: Option<&Value>) -> Option<String> {
    let value = value?.as_str().filter(|value| !value.is_empty())?;
    if value.starts_with("http://") || value.starts_with("https://") {
        return Some(value.to_owned());
    }
    Some(format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        value.trim_start_matches('/')
    ))
}

fn string_field<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(Value::as_str)
}

fn integer_field(value: &Value, name: &str) -> Option<u64> {
    value.get(name).and_then(Value::as_u64)
}

fn first_nonempty_env(names: &[&str]) -> Option<String> {
    names
        .iter()
        .filter_map(|name| env::var(name).ok())
        .find(|value| !value.trim().is_empty())
}

fn bounded_env_duration(name: &str, fallback: u64, minimum: u64, maximum: u64) -> Duration {
    let value = env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
        .clamp(minimum, maximum);
    Duration::from_millis(value)
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, String> {
    let path = fs::canonicalize(path)
        .map_err(|error| format!("{label} is unavailable at {} ({error})", path.display()))?;
    if !path.is_dir() {
        return Err(format!("{label} must point to a directory"));
    }
    Ok(path)
}

fn resolve_executable(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("IMAGE_GRID_NATIVE_SERVER_BIN must be an absolute executable path".to_owned());
    }
    let path = fs::canonicalize(path).map_err(|error| {
        format!(
            "IMAGE_GRID_NATIVE_SERVER_BIN is unavailable at {} ({error})",
            path.display()
        )
    })?;
    if !path.is_file() {
        return Err("IMAGE_GRID_NATIVE_SERVER_BIN must point to a file".to_owned());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path)
            .map_err(|error| format!("could not inspect native server executable: {error}"))?
            .permissions()
            .mode();
        if mode & 0o111 == 0 {
            return Err("IMAGE_GRID_NATIVE_SERVER_BIN must be executable".to_owned());
        }
    }
    Ok(path)
}

fn remaining_duration(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

fn validate_tool_arguments(arguments: &Value) -> Result<(), String> {
    let prompts = arguments
        .get("prompts")
        .and_then(Value::as_array)
        .ok_or_else(|| "prompts must be an array".to_owned())?;
    if prompts.is_empty() {
        return Err("prompts array must contain at least one prompt".to_owned());
    }
    if prompts.len() > MAX_PROMPTS {
        return Err(format!("prompt batch is limited to {MAX_PROMPTS} prompts"));
    }
    for (index, prompt) in prompts.iter().enumerate() {
        let prompt = prompt
            .as_str()
            .ok_or_else(|| format!("prompt {} must be a string", index + 1))?;
        if prompt.trim().is_empty() {
            return Err(format!("prompt {} must not be empty", index + 1));
        }
    }

    let count = match arguments.get("count") {
        None => 1_i128,
        Some(value) => json_integer(value).ok_or_else(|| "count must be an integer".to_owned())?,
    };
    if !(1..=i128::from(MAX_VARIANTS_PER_PROMPT)).contains(&count) {
        return Err(format!(
            "count must be between 1 and {MAX_VARIANTS_PER_PROMPT}"
        ));
    }
    if prompts.len() * count as usize > MAX_RUN_JOBS {
        return Err(format!("a run is limited to {MAX_RUN_JOBS} total jobs"));
    }

    validate_enum(
        arguments,
        "mood",
        &[
            "warm-mascot",
            "clean-thumbnail",
            "editorial-soft",
            "cinematic",
            "minimal-product",
        ],
    )?;
    validate_enum(arguments, "engine", &["app-server-image", "codex-svg"])?;
    validate_enum(
        arguments,
        "aspectRatio",
        &["16:9", "4:3", "1:1", "3:4", "9:16"],
    )?;

    if let Some(wait_ms) = arguments.get("waitMs") {
        let valid = json_integer(wait_ms)
            .is_some_and(|value| (0..=i128::from(MAX_WAIT_MS)).contains(&value));
        if !valid {
            return Err(format!(
                "waitMs must be an integer between 0 and {MAX_WAIT_MS}"
            ));
        }
    }
    if arguments
        .get("referencePremise")
        .is_some_and(|value| !value.is_string())
    {
        return Err("referencePremise must be a string".to_owned());
    }

    if let Some(reference_path) = arguments.get("referenceImagePath") {
        let reference_path = reference_path
            .as_str()
            .ok_or_else(|| "referenceImagePath must be a string".to_owned())?;
        if !reference_path.is_empty() {
            validate_reference_image(Path::new(reference_path))
                .map_err(|error| error.to_string())?;
        }
    }

    Ok(())
}

fn validate_enum(arguments: &Value, name: &str, accepted: &[&str]) -> Result<(), String> {
    let Some(value) = arguments.get(name) else {
        return Ok(());
    };
    let value = value
        .as_str()
        .ok_or_else(|| format!("{name} must be one of: {}", accepted.join(", ")))?;
    if !accepted.contains(&value) {
        return Err(format!("{name} must be one of: {}", accepted.join(", ")));
    }
    Ok(())
}

pub fn tool_record() -> Value {
    json!({
        "name": TOOL_NAME,
        "title": "Generate Image Grid",
        "description": TOOL_DESCRIPTION,
        "inputSchema": {
            "type": "object",
            "description": "Prompt count multiplied by variants per prompt must not exceed 24 total jobs.",
            "properties": {
                "prompts": {
                    "type": "array",
                    "description": "Prompt Batch input. Pass project-specific visual directions.",
                    "minItems": 1,
                    "maxItems": 12,
                    "items": {
                        "type": "string",
                        "minLength": 1,
                        "pattern": "\\S"
                    }
                },
                "count": {
                    "type": "integer",
                    "description": "Variants per prompt. prompts.length × count must be at most 24.",
                    "minimum": 1,
                    "maximum": 6,
                    "default": 1
                },
                "mood": {
                    "type": "string",
                    "enum": [
                        "warm-mascot",
                        "clean-thumbnail",
                        "editorial-soft",
                        "cinematic",
                        "minimal-product"
                    ],
                    "default": "warm-mascot"
                },
                "engine": {
                    "type": "string",
                    "enum": ["app-server-image", "codex-svg"],
                    "default": "app-server-image"
                },
                "aspectRatio": {
                    "type": "string",
                    "enum": ["16:9", "4:3", "1:1", "3:4", "9:16"],
                    "default": "16:9"
                },
                "referencePremise": {
                    "type": "string",
                    "description": "Optional visual identity notes from the current product or reference image."
                },
                "referenceImagePath": {
                    "type": "string",
                    "description": "Optional absolute local PNG, JPEG, or WebP path to attach as the visual reference."
                },
                "waitMs": {
                    "type": "integer",
                    "description": "Optional short wait for completion before returning.",
                    "minimum": 0,
                    "maximum": 120000,
                    "default": 0
                }
            },
            "required": ["prompts"],
            "allOf": [
                conditional_prompt_limit(1, 12),
                conditional_prompt_limit(2, 12),
                conditional_prompt_limit(3, 8),
                conditional_prompt_limit(4, 6),
                conditional_prompt_limit(5, 4),
                conditional_prompt_limit(6, 4)
            ],
            "x-image-grid-total-job-constraint": {
                "formula": "prompts.length * count",
                "maximum": 24
            }
        },
        "annotations": {
            "title": "Generate Image Grid",
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": false,
            "openWorldHint": false
        }
    })
}

fn conditional_prompt_limit(count: u8, max_items: u8) -> Value {
    json!({
        "if": {
            "required": ["count"],
            "properties": {
                "count": {
                    "const": count
                }
            }
        },
        "then": {
            "properties": {
                "prompts": {
                    "maxItems": max_items
                }
            }
        }
    })
}

fn tool_error(message: impl Into<String>) -> Value {
    json!({
        "content": [
            {
                "type": "text",
                "text": message.into()
            }
        ],
        "isError": true
    })
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn rpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into()
        }
    })
}

fn javascript_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|number| number != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn json_integer(value: &Value) -> Option<i128> {
    if let Some(value) = value.as_i64() {
        return Some(i128::from(value));
    }
    if let Some(value) = value.as_u64() {
        return Some(i128::from(value));
    }
    let value = value.as_f64()?;
    if value.is_finite() && value.fract() == 0.0 {
        Some(value as i128)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    #[derive(Debug)]
    struct TestRequest {
        method: String,
        path: String,
        body: Value,
    }

    struct FakeServer {
        url: String,
        handle: Option<thread::JoinHandle<Vec<TestRequest>>>,
    }

    impl FakeServer {
        fn running(root: PathBuf, expected_requests: usize) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
            let address = listener.local_addr().expect("fake address");
            let handle =
                thread::spawn(move || serve_fake_native(listener, root, expected_requests));
            Self {
                url: format!("http://{address}"),
                handle: Some(handle),
            }
        }

        fn finish(mut self) -> Vec<TestRequest> {
            self.handle
                .take()
                .expect("fake handle")
                .join()
                .expect("fake server thread")
        }
    }

    impl Drop for FakeServer {
        fn drop(&mut self) {
            if let Some(handle) = self.handle.take() {
                drop(handle);
            }
        }
    }

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!(
                "image-grid-mcp-{label}-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create test directory");
            Self { path }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn provider_free_transcript_covers_initialize_list_and_call() {
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":",
            "{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{},",
            "\"clientInfo\":{\"name\":\"native-smoke\",\"version\":\"0.1.0\"}}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\",\"params\":{}}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":",
            "{\"name\":\"generate_image_grid\",\"arguments\":{\"prompts\":[]}}}\n"
        );
        let mut output = Vec::new();

        serve(Cursor::new(input), &mut output).expect("MCP transcript");

        let responses: Vec<Value> = String::from_utf8(output)
            .expect("UTF-8 output")
            .lines()
            .map(|line| serde_json::from_str(line).expect("JSON response"))
            .collect();
        assert_eq!(responses.len(), 3);
        assert_eq!(responses[0]["id"], 1);
        assert_eq!(
            responses[0]["result"]["protocolVersion"],
            MCP_PROTOCOL_VERSION
        );
        assert_eq!(
            responses[0]["result"]["serverInfo"]["name"],
            "codex-image-grid-native"
        );
        assert_eq!(responses[1]["id"], 2);
        assert_eq!(
            responses[1]["result"]["tools"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(
            responses[1]["result"]["tools"][0]["name"],
            "generate_image_grid"
        );
        assert_eq!(
            responses[2],
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "result": {
                    "content": [{
                        "type": "text",
                        "text": "prompts array must contain at least one prompt"
                    }],
                    "isError": true
                }
            })
        );
    }

    #[test]
    fn tool_schema_preserves_the_frozen_public_limits() {
        let tool = tool_record();
        assert_eq!(tool["inputSchema"]["required"], json!(["prompts"]));
        assert_eq!(
            tool["inputSchema"]["properties"]["prompts"]["maxItems"],
            MAX_PROMPTS
        );
        assert_eq!(
            tool["inputSchema"]["properties"]["count"]["maximum"],
            MAX_VARIANTS_PER_PROMPT
        );
        assert_eq!(
            tool["inputSchema"]["x-image-grid-total-job-constraint"]["maximum"],
            MAX_RUN_JOBS
        );
        assert_eq!(
            tool["inputSchema"]["properties"]["waitMs"]["maximum"],
            MAX_WAIT_MS
        );
        assert!(tool["inputSchema"].get("additionalProperties").is_none());
    }

    #[test]
    fn strict_argument_errors_match_the_frozen_messages() {
        assert_eq!(
            validate_tool_arguments(&json!({"prompts": ["valid"], "count": 1.5})),
            Err("count must be an integer".to_owned())
        );
        assert_eq!(
            validate_tool_arguments(&json!({"prompts": ["valid"], "waitMs": -1})),
            Err("waitMs must be an integer between 0 and 120000".to_owned())
        );
        assert_eq!(
            validate_tool_arguments(&json!({
                "prompts": ["valid"],
                "referenceImagePath": "relative.png"
            })),
            Err("referenceImagePath must be an absolute local file path".to_owned())
        );
    }

    #[test]
    fn preflight_error_includes_nonselected_candidate_diagnostics() {
        let message = app_server_diagnostic_message(&json!({
            "diagnostics": {
                "selectedCommand": "/selected/codex",
                "selectedSource": "configured",
                "error": {"message": "initialize failed"},
                "candidates": [
                    {
                        "source": "IMAGE_GRID_CODEX_BIN",
                        "command": "/rejected/codex",
                        "status": "rejected",
                        "reason": "command is not executable"
                    },
                    {
                        "source": "chatgpt-bundled",
                        "command": "/missing/codex",
                        "status": "unavailable",
                        "reason": "file does not exist"
                    },
                    {
                        "source": "PATH",
                        "command": null,
                        "status": "skipped",
                        "reason": "PATH is unavailable"
                    },
                    {
                        "source": "selected",
                        "command": "/selected/codex",
                        "status": "selected",
                        "reason": null
                    }
                ]
            }
        }));

        assert_eq!(
            message,
            "Image Grid server is running, but App Server image generation is not ready. \
Selected command: /selected/codex (configured). Preflight failure: initialize failed. \
Candidate diagnostics: rejected IMAGE_GRID_CODEX_BIN=/rejected/codex: command is not executable; \
unavailable chatgpt-bundled=/missing/codex: file does not exist; \
skipped PATH=(none): PATH is unavailable."
        );
    }

    #[test]
    fn valid_call_uses_native_health_preflight_and_path_staging_contract() {
        let directory = TestDirectory::new("running");
        let root = fs::canonicalize(&directory.path).expect("canonical root");
        let reference_path = root.join("reference.png");
        fs::write(&reference_path, b"provider-free-reference").expect("reference fixture");
        let fake = FakeServer::running(root.clone(), 3);
        let config = test_config(&fake.url, Some(root.clone()), None);
        let input = json!({
            "prompts": ["project visual"],
            "count": 1,
            "mood": "clean-thumbnail",
            "engine": "app-server-image",
            "aspectRatio": "4:3",
            "referencePremise": "preserve the mascot",
            "referenceImagePath": reference_path,
            "waitMs": 250
        });
        validate_tool_arguments(&input).expect("valid input");

        let result =
            call_generate_image_grid_with_config(&normalize_tool_arguments(&input), &config)
                .expect("native generation response");
        let requests = fake.finish();

        assert_eq!(
            requests
                .iter()
                .map(|request| format!("{} {}", request.method, request.path))
                .collect::<Vec<_>>(),
            vec![
                "GET /api/health",
                "POST /api/preflight/app-server-image",
                "POST /api/run-batch"
            ]
        );
        let run_body = &requests[2].body;
        assert_eq!(
            run_body["referenceImagePath"],
            reference_path.to_string_lossy().as_ref()
        );
        assert!(run_body.get("referenceImage").is_none());
        assert_eq!(run_body["count"], 1);
        assert_eq!(run_body["waitMs"], 250);

        assert_eq!(result["isError"], false);
        assert_eq!(
            result["structuredContent"]["statusUrl"],
            format!("{}/api/runs/abc12345", config.image_grid_url)
        );
        assert_eq!(
            result["structuredContent"]["manifestUrl"],
            format!("{}/generated/abc12345/manifest.json", config.image_grid_url)
        );
        assert_eq!(
            result["structuredContent"]["imageUrls"],
            json!([format!(
                "{}/generated/abc12345/variant-01.png",
                config.image_grid_url
            )])
        );
        assert_eq!(
            result["structuredContent"]["codexMarkdown"],
            format!(
                "![prompt 1/1 variant 1/1]({}/generated/abc12345/variant-01.png)",
                config.image_grid_url
            )
        );
        assert_eq!(
            result["structuredContent"]["health"]["appServerImageReady"],
            true
        );
        let summary = result["content"][0]["text"].as_str().expect("summary");
        assert!(summary.contains("runId: abc12345"));
        assert!(summary.contains("serverStarted: false"));
        assert!(summary.contains("diagnostics:\n- none"));
    }

    #[test]
    fn configured_native_process_is_launched_once_and_verified() {
        let directory = TestDirectory::new("launch");
        let root = fs::canonicalize(&directory.path).expect("canonical root");
        let listener = TcpListener::bind("127.0.0.1:0").expect("reserve port");
        let address = listener.local_addr().expect("reserved address");
        drop(listener);
        let url = format!("http://{address}");
        let executable = env::current_exe().expect("test executable");
        let launch = LaunchPlan {
            label: "test native executable".to_owned(),
            program: executable,
            arguments: vec![
                "--ignored".to_owned(),
                "--exact".to_owned(),
                "tests::native_server_child_fixture".to_owned(),
                "--test-threads=1".to_owned(),
            ],
            cwd: Some(root.clone()),
            environment: vec![
                (
                    "IMAGE_GRID_MCP_FAKE_CHILD_ADDRESS".to_owned(),
                    address.to_string(),
                ),
                (
                    "IMAGE_GRID_MCP_FAKE_CHILD_ROOT".to_owned(),
                    root.to_string_lossy().into_owned(),
                ),
            ],
        };
        let mut config = test_config(&url, Some(root), Some(launch));
        config.launch_timeout = Duration::from_secs(5);
        config.launch_probe = Duration::from_millis(50);
        let input = json!({
            "prompts": ["vector mark"],
            "engine": "codex-svg",
            "waitMs": 0
        });
        validate_tool_arguments(&input).expect("valid launch input");

        let result =
            call_generate_image_grid_with_config(&normalize_tool_arguments(&input), &config)
                .expect("auto-launched generation response");

        assert_eq!(result["isError"], false);
        assert_eq!(result["structuredContent"]["serverStarted"], true);
        assert_eq!(
            result["structuredContent"]["launchPlan"],
            "test native executable"
        );
    }

    #[test]
    #[ignore = "spawned by configured_native_process_is_launched_once_and_verified"]
    fn native_server_child_fixture() {
        let Ok(address) = env::var("IMAGE_GRID_MCP_FAKE_CHILD_ADDRESS") else {
            return;
        };
        let root =
            PathBuf::from(env::var("IMAGE_GRID_MCP_FAKE_CHILD_ROOT").expect("child fixture root"));
        let listener = TcpListener::bind(&address).expect("child fixture bind");
        let requests = serve_fake_native(listener, root, 2);
        assert_eq!(
            requests
                .iter()
                .map(|request| request.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/api/health", "/api/run-batch"]
        );
    }

    fn test_config(
        url: &str,
        app_dir: Option<PathBuf>,
        launch_plan: Option<LaunchPlan>,
    ) -> BridgeConfig {
        BridgeConfig {
            endpoint: HttpEndpoint::parse(url).expect("test endpoint"),
            image_grid_url: url.to_owned(),
            app_dir,
            launch_plan,
            launch_timeout: Duration::from_secs(2),
            health_timeout: Duration::from_millis(500),
            preflight_timeout: Duration::from_millis(500),
            run_timeout: Duration::from_secs(2),
            launch_probe: Duration::from_millis(25),
        }
    }

    fn serve_fake_native(
        listener: TcpListener,
        root: PathBuf,
        expected_requests: usize,
    ) -> Vec<TestRequest> {
        listener
            .set_nonblocking(true)
            .expect("nonblocking fake listener");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut requests = Vec::new();
        while requests.len() < expected_requests && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .expect("fake read timeout");
                    let request = read_test_request(&mut stream);
                    let response = fake_response(&request, &root);
                    let bytes = serde_json::to_vec(&response).expect("fake response JSON");
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    )
                    .and_then(|_| stream.write_all(&bytes))
                    .and_then(|_| stream.flush())
                    .expect("write fake response");
                    requests.push(request);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("fake accept failed: {error}"),
            }
        }
        assert_eq!(
            requests.len(),
            expected_requests,
            "fake server request count"
        );
        requests
    }

    fn read_test_request(stream: &mut TcpStream) -> TestRequest {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4096];
        let header_end = loop {
            let count = stream.read(&mut buffer).expect("read fake request");
            assert!(count > 0, "request ended before headers");
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break position;
            }
        };
        let header = std::str::from_utf8(&bytes[..header_end]).expect("request headers");
        let mut lines = header.split("\r\n");
        let request_line = lines.next().expect("request line");
        let mut request_parts = request_line.split_whitespace();
        let method = request_parts.next().expect("method").to_owned();
        let path = request_parts.next().expect("path").to_owned();
        let content_length = lines
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("content length"))
            })
            .unwrap_or(0);
        let body_start = header_end + 4;
        while bytes.len() < body_start + content_length {
            let count = stream.read(&mut buffer).expect("read fake body");
            assert!(count > 0, "request ended before body");
            bytes.extend_from_slice(&buffer[..count]);
        }
        let body = if content_length == 0 {
            Value::Null
        } else {
            serde_json::from_slice(&bytes[body_start..body_start + content_length])
                .expect("request JSON")
        };
        TestRequest { method, path, body }
    }

    fn fake_response(request: &TestRequest, root: &Path) -> Value {
        match request.path.as_str() {
            "/api/health" => json!({
                "ok": true,
                "app": EXPECTED_APP_IDENTITY,
                "serverRoot": root,
                "identity": {
                    "app": EXPECTED_APP_IDENTITY,
                    "serverRoot": root
                },
                "appServerImage": false,
                "appServerImageReady": false
            }),
            "/api/preflight/app-server-image" => json!({
                "ok": true,
                "appServerImage": true,
                "appServerImageReady": true,
                "diagnostics": {
                    "ready": true,
                    "selectedCommand": "/provider-free/codex",
                    "selectedSource": "fixture"
                }
            }),
            "/api/run-batch" => {
                let run_root = root.join("generated").join("abc12345");
                json!({
                    "runId": "abc12345",
                    "status": "done",
                    "completed": true,
                    "counts": {"total": 1, "queued": 0, "starting": 0, "running": 0, "done": 1, "error": 0},
                    "statusUrl": "/api/runs/abc12345",
                    "manifestPath": run_root.join("manifest.json"),
                    "manifestUrl": "/generated/abc12345/manifest.json",
                    "manifestViewUrl": "/artifacts/abc12345/manifest",
                    "handoffPath": run_root.join("handoff.md"),
                    "handoffUrl": "/generated/abc12345/handoff.md",
                    "handoffViewUrl": "/artifacts/abc12345/handoff",
                    "server": {"app": EXPECTED_APP_IDENTITY},
                    "diagnostics": [],
                    "outputs": [{
                        "promptIndex": 1,
                        "promptTotal": 1,
                        "variant": 1,
                        "total": 1,
                        "status": "done",
                        "outputPath": run_root.join("variant-01.png"),
                        "imageUrl": "/generated/abc12345/variant-01.png"
                    }]
                })
            }
            path => panic!("unexpected fake request: {} {path}", request.method),
        }
    }
}
