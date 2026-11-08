//! Native, explicitly selected LM Studio cleanup.
//!
//! This adapter is intentionally separate from the OpenAI-compatible Custom
//! provider. It talks only to the fixed local LM Studio origin, refreshes the
//! selected model/instance facts before each request, and accepts only the
//! native API's single text-message response shape.

use reqwest::{redirect::Policy, Client, Method, StatusCode, Url};
use serde::Serialize;
use serde_json::{json, Value};
use std::future::Future;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::io::AsyncReadExt;

pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:1234";
pub const MODEL_KEY: &str = "qwen/qwen3.8-27b";
pub const MODEL_VARIANT: &str = "qwen/qwen3.8-27b@6bit";
pub const MODEL_INSTANCE_ID: &str = "voiceflow-cleanup";
pub(crate) const MAX_OUTPUT_TOKENS: u64 = 2048;

const MODEL_PUBLISHER: &str = "qwen";
const MODEL_ARCHITECTURE: &str = "qwen3_5";
const MODEL_FORMAT: &str = "mlx";
const MODEL_SIZE_BYTES: u64 = 22_805_016_658;
const CONTEXT_LENGTH: u64 = 8192;
const MAX_STATUS_BODY_BYTES: usize = 512 * 1024;
const MAX_CLI_STATUS_BYTES: usize = 256 * 1024;
const MAX_CHAT_BODY_BYTES: usize = 256 * 1024;
// Bound the complete UTF-8 message content conservatively below the 8k
// context, leaving room for native chat framing and the 2048-token output cap.
pub(crate) const MAX_INPUT_BYTES: usize = 4096;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const METADATA_TIMEOUT: Duration = Duration::from_secs(8);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
const STATUS_PROBE_TTL: Duration = Duration::from_secs(5);

struct ProbeCache<T> {
    value: tokio::sync::Mutex<Option<(String, Instant, T)>>,
}

impl<T> Default for ProbeCache<T> {
    fn default() -> Self {
        Self {
            value: tokio::sync::Mutex::new(None),
        }
    }
}

static CLI_STATUS_CACHE: OnceLock<ProbeCache<()>> = OnceLock::new();
static API_MODELS_CACHE: OnceLock<ProbeCache<Value>> = OnceLock::new();

async fn cached_cli_status(
    cache: &ProbeCache<()>,
    key: String,
    now: impl Fn() -> Instant,
    probe: impl Future<Output = Result<Value, LocalError>>,
    trace: Option<&crate::metrics::TraceHandle>,
) -> Result<bool, LocalError> {
    let (mut cached, wait_span) = match cache.value.try_lock() {
        Ok(cached) => (cached, None),
        Err(_) => {
            let span = trace.map(|trace| trace.begin(crate::metrics::Stage::LmStudioStatusProbe));
            (cache.value.lock().await, span)
        }
    };
    let started = now();
    if let Some((cached_key, inserted, _)) = cached.as_ref() {
        if cached_key == &key && started.saturating_duration_since(*inserted) < STATUS_PROBE_TTL {
            return Ok(true);
        }
    }
    drop(wait_span);
    let _span = trace.map(|trace| trace.begin(crate::metrics::Stage::LmStudioStatusProbe));
    let body = probe.await?;
    validate_cli_instance(&body)?;
    *cached = Some((key, now(), ()));
    Ok(false)
}

async fn cached_api_models(
    cache: &ProbeCache<Value>,
    url: &Url,
    now: impl Fn() -> Instant,
    probe: impl Future<Output = Result<Value, CallError>>,
    trace: Option<&crate::metrics::TraceHandle>,
) -> Result<(Value, bool), LocalError> {
    let key = format!(
        "{}|{}|{}|{}",
        url.as_str(),
        MODEL_VARIANT,
        MODEL_INSTANCE_ID,
        CONTEXT_LENGTH
    );
    let (mut cached, wait_span) = match cache.value.try_lock() {
        Ok(cached) => (cached, None),
        Err(_) => {
            let span = trace.map(|trace| trace.begin(crate::metrics::Stage::LmStudioStatusProbe));
            (cache.value.lock().await, span)
        }
    };
    let started = now();
    if let Some((cached_key, inserted, value)) = cached.as_ref() {
        if cached_key == &key && started.saturating_duration_since(*inserted) < STATUS_PROBE_TTL {
            return Ok((value.clone(), true));
        }
    }
    drop(wait_span);
    let _span = trace.map(|trace| trace.begin(crate::metrics::Stage::LmStudioStatusProbe));
    let body = probe.await.map_err(map_call_error)?;
    validate_model_metadata(&body)?;
    *cached = Some((key, now(), body.clone()));
    Ok((body, false))
}

#[derive(Debug, Clone, Serialize)]
pub struct CleanupStatus {
    pub available: bool,
    pub status: String,
    pub message: String,
    pub model: String,
    pub instance_id: String,
    /// `None` means the complete API/CLI check did not establish a usable
    /// source fact. Model identity and source are separate.
    pub local_source_confirmed: Option<bool>,
}

#[derive(Debug, Error, Clone, Copy)]
pub enum LocalError {
    #[error("LM Studio could not be reached at its local address.")]
    Unreachable,
    #[error("The LM Studio request timed out.")]
    Timeout,
    #[error("The LM Studio local endpoint is invalid.")]
    InvalidEndpoint,
    #[error("The selected Qwen 3.8 model is not listed in LM Studio.")]
    ModelMissing,
    #[error("LM Studio model metadata does not match the supported Qwen 3.8 MLX variant.")]
    ModelMismatch,
    #[error("LM Studio must expose exactly one matching loaded instance with the supported context length.")]
    InstanceNotReady,
    #[error("LM Studio does not advertise reasoning-off support for the selected model.")]
    ReasoningOffUnavailable,
    #[error("The selected LM Studio instance is remote; local cleanup is not allowed.")]
    RemoteInstance,
    #[error("LM Studio did not provide enough instance-source data to confirm local execution.")]
    InstanceSourceUnknown,
    #[error("The LM Studio CLI is unavailable at ~/.lmstudio/bin/lms.")]
    CliUnavailable,
    #[error("The LM Studio CLI status command timed out.")]
    CliTimeout,
    #[error("The LM Studio CLI could not read the selected server's loaded model.")]
    CliFailed,
    #[error("LM Studio returned an unsupported or invalid response.")]
    InvalidResponse,
    #[error("LM Studio returned an HTTP error.")]
    Http,
    #[error("The LM Studio response exceeded the local size limit.")]
    BodyTooLarge,
    #[error("The native LM Studio request has an unsupported message shape.")]
    InvalidInput,
    #[error("The native LM Studio input is {input_bytes} UTF-8 bytes; the limit is {max_bytes}.")]
    InputTooLarge {
        input_bytes: usize,
        max_bytes: usize,
    },
}

#[derive(Debug, Clone, Copy)]
enum CallError {
    Request,
    Timeout,
    Http,
    NotFound,
    BodyTooLarge,
    InvalidJson,
}

#[derive(Debug)]
struct ValidatedInstance {
    local_source_confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCompletion {
    pub text: String,
    pub model_instance_id: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: u64,
}

pub fn chat_endpoint(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    format!("{base}/api/v1/chat")
}

fn client() -> Result<&'static Client, LocalError> {
    static CLIENT: OnceLock<Result<Client, ()>> = OnceLock::new();
    match CLIENT.get_or_init(|| {
        Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| ())
    }) {
        Ok(client) => Ok(client),
        Err(()) => Err(LocalError::Unreachable),
    }
}

fn normalized_base(base_url: &str) -> Option<Url> {
    let url = Url::parse(base_url.trim()).ok()?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port_or_known_default() != Some(1234)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return None;
    }
    Some(url)
}

fn endpoint(base: &Url, path: &'static str) -> Url {
    let mut url = base.clone();
    url.set_path(path);
    url
}

fn requested_model_is_supported(model: &str) -> bool {
    model.trim() == MODEL_VARIANT
}

fn model_item(body: &Value) -> Result<&Value, LocalError> {
    let models = body
        .get("models")
        .and_then(Value::as_array)
        .ok_or(LocalError::InvalidResponse)?;
    let mut matching_models = models
        .iter()
        .filter(|model| model.get("key").and_then(Value::as_str) == Some(MODEL_KEY));
    let Some(model) = matching_models.next() else {
        return Err(LocalError::ModelMissing);
    };
    if matching_models.next().is_some() {
        return Err(LocalError::ModelMismatch);
    }

    if model.get("publisher").and_then(Value::as_str) != Some(MODEL_PUBLISHER)
        || model.get("architecture").and_then(Value::as_str) != Some(MODEL_ARCHITECTURE)
        || model.get("format").and_then(Value::as_str) != Some(MODEL_FORMAT)
        || model.get("selected_variant").and_then(Value::as_str) != Some(MODEL_VARIANT)
        || model.get("size_bytes").and_then(Value::as_u64) != Some(MODEL_SIZE_BYTES)
        || model
            .get("quantization")
            .and_then(|quantization| quantization.get("name"))
            .and_then(Value::as_str)
            != Some("6bit")
        || model
            .get("quantization")
            .and_then(|quantization| quantization.get("bits_per_weight"))
            .and_then(Value::as_u64)
            != Some(6)
    {
        return Err(LocalError::ModelMismatch);
    }

    let reasoning_off_supported = model
        .get("capabilities")
        .and_then(|capabilities| capabilities.get("reasoning"))
        .and_then(|reasoning| reasoning.get("allowed_options"))
        .and_then(Value::as_array)
        .is_some_and(|options| options.iter().any(|option| option.as_str() == Some("off")));
    if !reasoning_off_supported {
        return Err(LocalError::ReasoningOffUnavailable);
    }

    let instances = model
        .get("loaded_instances")
        .and_then(Value::as_array)
        .ok_or(LocalError::InstanceNotReady)?;
    if instances.len() != 1 {
        return Err(LocalError::InstanceNotReady);
    }
    let instance = instances.first().ok_or(LocalError::InstanceNotReady)?;
    if instance.get("id").and_then(Value::as_str) != Some(MODEL_INSTANCE_ID)
        || instance
            .get("config")
            .and_then(|config| config.get("context_length"))
            .and_then(Value::as_u64)
            != Some(CONTEXT_LENGTH)
    {
        return Err(LocalError::InstanceNotReady);
    }
    Ok(model)
}

fn validate_model_metadata(body: &Value) -> Result<(), LocalError> {
    let _model = model_item(body)?;
    Ok(())
}

/// Report the current model and loaded-instance contract without sending any
/// transcript. The LM Studio API does not disclose the model's source path.
pub async fn status(base_url: &str, model: &str) -> CleanupStatus {
    let mut status = CleanupStatus {
        available: false,
        status: "unknown".to_owned(),
        message: "The model source is confirmed separately through the LM Studio CLI; model identity is checked through its API.".to_owned(),
        model: model.to_owned(),
        instance_id: MODEL_INSTANCE_ID.to_owned(),
        local_source_confirmed: None,
    };
    if !requested_model_is_supported(model) {
        status.status = "unsupported_model".to_owned();
        status.message = LocalError::ModelMismatch.to_string();
        return status;
    }
    let Some(base) = normalized_base(base_url) else {
        status.status = "invalid_endpoint".to_owned();
        status.message = LocalError::InvalidEndpoint.to_string();
        return status;
    };
    match preflight(&base, None).await {
        Ok(instance) => {
            status.available = true;
            status.local_source_confirmed = Some(instance.local_source_confirmed);
            status.status = "ready_local_instance".to_owned();
            status.message = "LM Studio CLI confirms the selected instance is local on 127.0.0.1:1234, and native API metadata matches the supported Qwen 3.8 MLX variant. The API does not expose the physical model path.".to_owned();
        }
        Err(error) => {
            status.status = status_code(&error).to_owned();
            status.message = error.to_string();
            status.local_source_confirmed = match error {
                LocalError::RemoteInstance => Some(false),
                _ => None,
            };
        }
    }
    status
}

pub(crate) fn status_code(error: &LocalError) -> &'static str {
    match error {
        LocalError::Unreachable => "unreachable",
        LocalError::Timeout => "timeout",
        LocalError::InvalidEndpoint => "invalid_endpoint",
        LocalError::ModelMissing => "model_missing",
        LocalError::ModelMismatch => "model_mismatch",
        LocalError::InstanceNotReady => "instance_not_ready",
        LocalError::ReasoningOffUnavailable => "reasoning_off_unavailable",
        LocalError::RemoteInstance => "remote_instance",
        LocalError::InstanceSourceUnknown => "instance_source_unknown",
        LocalError::CliUnavailable => "cli_unavailable",
        LocalError::CliTimeout => "cli_timeout",
        LocalError::CliFailed => "cli_failed",
        LocalError::InvalidResponse => "invalid_response",
        LocalError::Http => "http_error",
        LocalError::BodyTooLarge => "body_too_large",
        LocalError::InvalidInput => "invalid_input",
        LocalError::InputTooLarge { .. } => "input_too_large",
    }
}

async fn preflight(
    base: &Url,
    trace: Option<&crate::metrics::TraceHandle>,
) -> Result<ValidatedInstance, LocalError> {
    let executable = lms_binary().ok_or(LocalError::CliUnavailable)?;
    if !executable.is_file() {
        return Err(LocalError::CliUnavailable);
    }
    let cli_key = format!(
        "{}|ps --json --host 127.0.0.1 --port 1234|{}|{}",
        executable.display(),
        MODEL_VARIANT,
        MODEL_INSTANCE_ID
    );
    let url = endpoint(base, "/api/v1/models");
    preflight_with_trace(
        base,
        CLI_STATUS_CACHE.get_or_init(ProbeCache::default),
        API_MODELS_CACHE.get_or_init(ProbeCache::default),
        cli_key,
        Instant::now,
        (
            run_cli_status_probe(executable),
            get_json(&url, MAX_STATUS_BODY_BYTES),
        ),
        trace,
    )
    .await
}

async fn preflight_with_trace(
    base: &Url,
    cli_cache: &ProbeCache<()>,
    api_cache: &ProbeCache<Value>,
    cli_key: String,
    now: impl Fn() -> Instant,
    probes: (
        impl Future<Output = Result<Value, LocalError>>,
        impl Future<Output = Result<Value, CallError>>,
    ),
    trace: Option<&crate::metrics::TraceHandle>,
) -> Result<ValidatedInstance, LocalError> {
    let (cli_probe, api_probe) = probes;
    cached_cli_status(cli_cache, cli_key, &now, cli_probe, trace).await?;
    let url = endpoint(base, "/api/v1/models");
    let _body = cached_api_models(api_cache, &url, now, api_probe, trace).await?;
    Ok(ValidatedInstance {
        local_source_confirmed: true,
    })
}

fn lms_binary() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".lmstudio/bin/lms"))
}

async fn run_cli_status_probe(executable: PathBuf) -> Result<Value, LocalError> {
    let mut child = tokio::process::Command::new(executable)
        .args(["ps", "--json", "--host", "127.0.0.1", "--port", "1234"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| LocalError::CliUnavailable)?;
    let stdout = child.stdout.take().ok_or(LocalError::CliFailed)?;
    let output = tokio::time::timeout(METADATA_TIMEOUT, async {
        let mut bytes = Vec::new();
        stdout
            .take((MAX_CLI_STATUS_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| LocalError::CliFailed)?;
        if bytes.len() > MAX_CLI_STATUS_BYTES {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(LocalError::CliFailed);
        }
        let exit = child.wait().await.map_err(|_| LocalError::CliFailed)?;
        if !exit.success() {
            return Err(LocalError::CliFailed);
        }
        Ok(bytes)
    })
    .await;
    let output = match output {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(error);
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(LocalError::CliTimeout);
        }
    };
    serde_json::from_slice(&output).map_err(|_| LocalError::CliFailed)
}

fn validate_cli_instance(body: &Value) -> Result<(), LocalError> {
    let instances = body.as_array().ok_or(LocalError::CliFailed)?;
    let mut matching = instances
        .iter()
        .filter(|instance| instance.get("modelKey").and_then(Value::as_str) == Some(MODEL_KEY));
    let instance = matching.next().ok_or(LocalError::InstanceNotReady)?;
    if matching.next().is_some() {
        return Err(LocalError::InstanceNotReady);
    }
    if instance.get("identifier").and_then(Value::as_str) != Some(MODEL_INSTANCE_ID)
        || instance.get("format").and_then(Value::as_str) != Some("safetensors")
        || instance.get("selectedVariant").and_then(Value::as_str) != Some(MODEL_VARIANT)
        || instance.get("architecture").and_then(Value::as_str) != Some(MODEL_ARCHITECTURE)
        || instance.get("sizeBytes").and_then(Value::as_u64) != Some(MODEL_SIZE_BYTES)
        || instance
            .get("quantization")
            .and_then(|quantization| quantization.get("name"))
            .and_then(Value::as_str)
            != Some("6bit")
        || instance
            .get("quantization")
            .and_then(|quantization| quantization.get("bits"))
            .and_then(Value::as_u64)
            != Some(6)
        || instance.get("contextLength").and_then(Value::as_u64) != Some(CONTEXT_LENGTH)
    {
        return Err(LocalError::ModelMismatch);
    }
    match instance.get("deviceIdentifier") {
        Some(Value::Null) => Ok(()),
        Some(Value::String(_)) => Err(LocalError::RemoteInstance),
        Some(_) | None => Err(LocalError::InstanceSourceUnknown),
    }
}

fn map_call_error(error: CallError) -> LocalError {
    match error {
        CallError::Request => LocalError::Unreachable,
        CallError::Timeout => LocalError::Timeout,
        CallError::Http | CallError::NotFound => LocalError::Http,
        CallError::BodyTooLarge => LocalError::BodyTooLarge,
        CallError::InvalidJson => LocalError::InvalidResponse,
    }
}

async fn get_json(url: &Url, max_bytes: usize) -> Result<Value, CallError> {
    request_json(
        client().map_err(|_| CallError::Request)?,
        Method::GET,
        url,
        None,
        max_bytes,
        METADATA_TIMEOUT,
    )
    .await
}

async fn post_json(url: &Url, body: &Value, max_bytes: usize) -> Result<Value, CallError> {
    request_json(
        client().map_err(|_| CallError::Request)?,
        Method::POST,
        url,
        Some(body),
        max_bytes,
        REQUEST_TIMEOUT,
    )
    .await
}

async fn request_json(
    client: &Client,
    method: Method,
    url: &Url,
    body: Option<&Value>,
    max_bytes: usize,
    timeout: Duration,
) -> Result<Value, CallError> {
    let base = normalized_base(DEFAULT_BASE_URL).ok_or(CallError::Request)?;
    let expected_path = match url.path() {
        "/api/v1/models" => "/api/v1/models",
        "/api/v1/chat" => "/api/v1/chat",
        _ => return Err(CallError::Request),
    };
    if url.scheme() != "http"
        || url.host_str() != base.host_str()
        || url.port_or_known_default() != base.port_or_known_default()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != expected_path
    {
        return Err(CallError::Request);
    }

    let mut request = client
        .request(method, url.clone())
        .header(reqwest::header::ACCEPT, "application/json")
        .timeout(timeout);
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request.send().await.map_err(|error| {
        if error.is_timeout() {
            CallError::Timeout
        } else {
            CallError::Request
        }
    })?;
    match response.status() {
        StatusCode::NOT_FOUND => return Err(CallError::NotFound),
        status if !status.is_success() => return Err(CallError::Http),
        _ => {}
    }
    let bytes = read_limited(response, max_bytes).await?;
    serde_json::from_slice(&bytes).map_err(|_| CallError::InvalidJson)
}

async fn read_limited(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, CallError> {
    if response
        .content_length()
        .is_some_and(|content_length| content_length > max_bytes as u64)
    {
        return Err(CallError::BodyTooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        if error.is_timeout() {
            CallError::Timeout
        } else {
            CallError::Request
        }
    })? {
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(CallError::BodyTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Complete a text-only cleanup using the native `/api/v1/chat` contract.
pub async fn complete_at_endpoint(
    endpoint: &str,
    requested_model: &str,
    messages: Vec<(String, String)>,
    trace: Option<&crate::metrics::TraceHandle>,
) -> Result<NativeCompletion, LocalError> {
    if !requested_model_is_supported(requested_model) {
        return Err(LocalError::ModelMismatch);
    }
    let url = Url::parse(endpoint).map_err(|_| LocalError::InvalidEndpoint)?;
    let base = normalized_base(DEFAULT_BASE_URL).ok_or(LocalError::InvalidEndpoint)?;
    if url.scheme() != "http"
        || url.host_str() != base.host_str()
        || url.port_or_known_default() != base.port_or_known_default()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/api/v1/chat"
    {
        return Err(LocalError::InvalidEndpoint);
    }
    let (system_prompt, input) = validate_messages(&messages)?;
    preflight(&base, trace).await?;

    let request = json!({
        "model": MODEL_INSTANCE_ID,
        "system_prompt": system_prompt,
        "input": input,
        "stream": false,
        "reasoning": "off",
        "context_length": CONTEXT_LENGTH,
        "max_output_tokens": MAX_OUTPUT_TOKENS,
        "temperature": 0,
        "store": false,
        "integrations": [],
    });
    let response = post_json(&url, &request, MAX_CHAT_BODY_BYTES)
        .await
        .map_err(map_call_error)?;
    parse_response(&response)
}

pub(crate) fn validate_messages(messages: &[(String, String)]) -> Result<(&str, &str), LocalError> {
    if messages.len() != 2
        || messages[0].0 != "system"
        || messages[1].0 != "user"
        || messages[0].1.trim().is_empty()
        || messages[1].1.trim().is_empty()
    {
        return Err(LocalError::InvalidInput);
    }
    let input_bytes = messages[0].1.len().saturating_add(messages[1].1.len());
    if input_bytes > MAX_INPUT_BYTES {
        return Err(LocalError::InputTooLarge {
            input_bytes,
            max_bytes: MAX_INPUT_BYTES,
        });
    }
    Ok((&messages[0].1, &messages[1].1))
}

fn parse_response(response: &Value) -> Result<NativeCompletion, LocalError> {
    if response.get("model_instance_id").and_then(Value::as_str) != Some(MODEL_INSTANCE_ID)
        || response
            .get("response_id")
            .is_some_and(|response_id| !response_id.is_null())
    {
        return Err(LocalError::InvalidResponse);
    }
    let output = response
        .get("output")
        .and_then(Value::as_array)
        .filter(|output| output.len() == 1)
        .ok_or(LocalError::InvalidResponse)?;
    let message = &output[0];
    if message.as_object().is_none_or(|fields| fields.len() != 2)
        || message.get("type").and_then(Value::as_str) != Some("message")
    {
        return Err(LocalError::InvalidResponse);
    }
    let content = message
        .get("content")
        .and_then(Value::as_str)
        .filter(|content| !content.trim().is_empty())
        .ok_or(LocalError::InvalidResponse)?;
    let stats = response.get("stats").ok_or(LocalError::InvalidResponse)?;
    // Without a native finish reason, reaching the configured cap cannot prove
    // completion, so it is conservatively treated as unconfirmed.
    let output_tokens = stats
        .get("total_output_tokens")
        .and_then(Value::as_u64)
        .filter(|tokens| *tokens > 0 && *tokens < MAX_OUTPUT_TOKENS)
        .ok_or(LocalError::InvalidResponse)?;
    let reasoning_tokens = stats
        .get("reasoning_output_tokens")
        .and_then(Value::as_u64)
        .ok_or(LocalError::InvalidResponse)?;
    if reasoning_tokens != 0 {
        return Err(LocalError::InvalidResponse);
    }
    Ok(NativeCompletion {
        text: content.to_owned(),
        model_instance_id: MODEL_INSTANCE_ID.to_owned(),
        input_tokens: stats.get("input_tokens").and_then(Value::as_u64),
        output_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_model(device: Option<Value>) -> Value {
        let mut instance = json!({
            "id": MODEL_INSTANCE_ID,
            "config": {"context_length": CONTEXT_LENGTH, "parallel": 4}
        });
        if let Some(device) = device {
            instance["deviceIdentifier"] = device;
        }
        json!({"models": [{
            "key": MODEL_KEY,
            "publisher": MODEL_PUBLISHER,
            "architecture": MODEL_ARCHITECTURE,
            "format": MODEL_FORMAT,
            "size_bytes": MODEL_SIZE_BYTES,
            "selected_variant": MODEL_VARIANT,
            "quantization": {"name": "6bit", "bits_per_weight": 6},
            "capabilities": {"reasoning": {"allowed_options": ["off", "low", "xhigh"]}},
            "loaded_instances": [instance]
        }]})
    }

    #[test]
    fn native_model_metadata_checks_exact_variant_instance_and_reasoning_off() {
        assert!(validate_model_metadata(&api_model(None)).is_ok());
        let mut body = api_model(None);
        body["models"][0]["capabilities"]["reasoning"]["allowed_options"] = json!(["low"]);
        assert!(matches!(
            validate_model_metadata(&body),
            Err(LocalError::ReasoningOffUnavailable)
        ));
        let mut body = api_model(None);
        body["models"][0]["selected_variant"] = json!("qwen/qwen3.8-27b@4bit");
        assert!(matches!(
            validate_model_metadata(&body),
            Err(LocalError::ModelMismatch)
        ));
        let mut body = api_model(None);
        body["models"][0]["loaded_instances"]
            .as_array_mut()
            .unwrap()
            .push(
                json!({"id": "other-qwen-instance", "config": {"context_length": CONTEXT_LENGTH}}),
            );
        assert!(matches!(
            validate_model_metadata(&body),
            Err(LocalError::InstanceNotReady)
        ));
    }

    #[test]
    fn cli_source_requires_explicit_local_null_and_exact_loaded_identity() {
        let local = json!([{
            "identifier": MODEL_INSTANCE_ID,
            "modelKey": MODEL_KEY,
            "format": "safetensors",
            "selectedVariant": MODEL_VARIANT,
            "architecture": MODEL_ARCHITECTURE,
            "sizeBytes": MODEL_SIZE_BYTES,
            "quantization": {"name": "6bit", "bits": 6},
            "contextLength": CONTEXT_LENGTH,
            "deviceIdentifier": null
        }]);
        assert!(validate_cli_instance(&local).is_ok());
        let mut missing = local.clone();
        missing[0]
            .as_object_mut()
            .unwrap()
            .remove("deviceIdentifier");
        assert!(matches!(
            validate_cli_instance(&missing),
            Err(LocalError::InstanceSourceUnknown)
        ));
        let mut wrong_type = local.clone();
        wrong_type[0]["deviceIdentifier"] = json!(123);
        assert!(matches!(
            validate_cli_instance(&wrong_type),
            Err(LocalError::InstanceSourceUnknown)
        ));
        let mut remote = local;
        remote[0]["deviceIdentifier"] = json!("remote-device");
        assert!(matches!(
            validate_cli_instance(&remote),
            Err(LocalError::RemoteInstance)
        ));
        let mut duplicate = json!([{
            "identifier": MODEL_INSTANCE_ID,
            "modelKey": MODEL_KEY,
            "format": "safetensors",
            "selectedVariant": MODEL_VARIANT,
            "architecture": MODEL_ARCHITECTURE,
            "sizeBytes": MODEL_SIZE_BYTES,
            "quantization": {"name": "6bit", "bits": 6},
            "contextLength": CONTEXT_LENGTH,
            "deviceIdentifier": null
        }]);
        duplicate.as_array_mut().unwrap().push(json!({
            "identifier": "other-qwen-instance",
            "modelKey": MODEL_KEY,
            "format": "safetensors",
            "selectedVariant": MODEL_VARIANT,
            "architecture": MODEL_ARCHITECTURE,
            "sizeBytes": MODEL_SIZE_BYTES,
            "quantization": {"name": "6bit", "bits": 6},
            "contextLength": CONTEXT_LENGTH,
            "deviceIdentifier": null
        }));
        assert!(matches!(
            validate_cli_instance(&duplicate),
            Err(LocalError::InstanceNotReady)
        ));
    }

    #[test]
    fn native_reply_accepts_only_single_text_message_without_reasoning() {
        let response = json!({
            "model_instance_id": MODEL_INSTANCE_ID,
            "output": [{"type": "message", "content": "A cleaned sentence."}],
            "stats": {"input_tokens": 20, "total_output_tokens": 5, "reasoning_output_tokens": 0}
        });
        let reply = parse_response(&response).unwrap();
        assert_eq!(reply.text, "A cleaned sentence.");
        assert_eq!(reply.input_tokens, Some(20));
        assert_eq!(reply.output_tokens, 5);

        let mut missing_tokens = response.clone();
        missing_tokens["stats"]
            .as_object_mut()
            .unwrap()
            .remove("total_output_tokens");
        assert!(matches!(
            parse_response(&missing_tokens),
            Err(LocalError::InvalidResponse)
        ));
        let mut malformed_tokens = response.clone();
        malformed_tokens["stats"]["total_output_tokens"] = json!("5");
        assert!(matches!(
            parse_response(&malformed_tokens),
            Err(LocalError::InvalidResponse)
        ));
        let mut just_below_cap = response.clone();
        just_below_cap["stats"]["total_output_tokens"] = json!(MAX_OUTPUT_TOKENS - 1);
        assert_eq!(
            parse_response(&just_below_cap).unwrap().output_tokens,
            MAX_OUTPUT_TOKENS - 1
        );
        for token_count in [0, MAX_OUTPUT_TOKENS, MAX_OUTPUT_TOKENS + 1] {
            let mut capped = response.clone();
            capped["stats"]["total_output_tokens"] = json!(token_count);
            assert!(matches!(
                parse_response(&capped),
                Err(LocalError::InvalidResponse)
            ));
        }

        let mut reasoning = response.clone();
        reasoning["output"] = json!([{"type": "reasoning", "content": "hidden"}]);
        assert!(matches!(
            parse_response(&reasoning),
            Err(LocalError::InvalidResponse)
        ));
        let mut stateful = response;
        stateful["response_id"] = json!("stored-response");
        assert!(matches!(
            parse_response(&stateful),
            Err(LocalError::InvalidResponse)
        ));
    }

    #[test]
    fn native_request_bounds_system_and_user_message_bytes_together() {
        let within_limit = vec![
            ("system".to_owned(), "s".repeat(2048)),
            ("user".to_owned(), "u".repeat(2048)),
        ];
        assert!(validate_messages(&within_limit).is_ok());

        let over_limit = vec![
            ("system".to_owned(), "s".repeat(2048)),
            ("user".to_owned(), "u".repeat(2049)),
        ];
        assert!(matches!(
            validate_messages(&over_limit),
            Err(LocalError::InputTooLarge {
                input_bytes: 4097,
                max_bytes: MAX_INPUT_BYTES
            })
        ));
        assert_eq!(
            status_code(&LocalError::InputTooLarge {
                input_bytes: 4097,
                max_bytes: MAX_INPUT_BYTES,
            }),
            "input_too_large"
        );
    }
    #[tokio::test]
    async fn status_probe_cached_within_ttl() {
        use std::cell::Cell;
        use std::fs;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let unique = format!(
            "voiceflow-lmstudio-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        fs::create_dir(&root).unwrap();
        let cli_count = root.join("cli-count");
        let cli = root.join("lms");
        let cli_fixture = json!([{
            "identifier": MODEL_INSTANCE_ID,
            "modelKey": MODEL_KEY,
            "format": "safetensors",
            "selectedVariant": MODEL_VARIANT,
            "architecture": MODEL_ARCHITECTURE,
            "sizeBytes": MODEL_SIZE_BYTES,
            "quantization": {"name": "6bit", "bits": 6},
            "contextLength": CONTEXT_LENGTH,
            "deviceIdentifier": null
        }]);
        fs::write(
            &cli,
            format!(
                "#!/bin/sh\necho x >> {}\nprintf '%s' '{}'\n",
                cli_count.display(),
                cli_fixture
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&cli, fs::Permissions::from_mode(0o700)).unwrap();
        }

        let now = Cell::new(Instant::now());
        let calls = AtomicUsize::new(0);
        let cli_cache = ProbeCache::default();
        let api_cache = ProbeCache::default();
        let cli_key = format!(
            "{}|ps --json|{}|{}",
            cli.display(),
            MODEL_VARIANT,
            MODEL_INSTANCE_ID
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = vec![0; 4096];
                let _ = stream.read(&mut request).await.unwrap();
                let body = serde_json::to_vec(&api_model(None)).unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.write_all(&body).await.unwrap();
            }
        });
        let test_client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .build()
            .unwrap();
        let base = Url::parse(&format!("http://{address}")).unwrap();
        let metrics = crate::metrics::Metrics::default();
        let mut timer = metrics.stop_to_insert(
            crate::metrics::MetricGroup::unknown("dictation"),
            tokio_util::sync::CancellationToken::new(),
        );
        let trace = timer.bind_trace(1);
        let api_probe = || async {
            let url = endpoint(&base, "/api/v1/models");
            let response = test_client
                .get(url)
                .send()
                .await
                .map_err(|_| CallError::Request)?;
            let bytes = response.bytes().await.map_err(|_| CallError::Request)?;
            serde_json::from_slice(&bytes).map_err(|_| CallError::InvalidJson)
        };
        let first = preflight_with_trace(
            &base,
            &cli_cache,
            &api_cache,
            cli_key.clone(),
            || now.get(),
            (run_cli_status_probe(cli.clone()), api_probe()),
            Some(&trace),
        )
        .await
        .unwrap();
        assert!(first.local_source_confirmed);
        assert_eq!(fs::read_to_string(&cli_count).unwrap().lines().count(), 1);
        assert_eq!(trace.snapshot().stages.len(), 2);

        let second = preflight_with_trace(
            &base,
            &cli_cache,
            &api_cache,
            cli_key.clone(),
            || now.get(),
            (run_cli_status_probe(cli.clone()), async {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(CallError::Request)
            }),
            Some(&trace),
        )
        .await
        .unwrap();
        assert!(second.local_source_confirmed);
        assert_eq!(fs::read_to_string(&cli_count).unwrap().lines().count(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(trace.snapshot().stages.len(), 2);

        now.set(now.get() + Duration::from_secs(5));
        let refreshed = preflight_with_trace(
            &base,
            &cli_cache,
            &api_cache,
            cli_key.clone(),
            || now.get(),
            (run_cli_status_probe(cli.clone()), api_probe()),
            Some(&trace),
        )
        .await
        .unwrap();
        assert!(refreshed.local_source_confirmed);
        assert_eq!(fs::read_to_string(&cli_count).unwrap().lines().count(), 2);
        assert_eq!(trace.snapshot().stages.len(), 4);

        let other_base = Url::parse("http://127.0.0.1:1235").unwrap();
        let other_endpoint = preflight_with_trace(
            &other_base,
            &cli_cache,
            &api_cache,
            cli_key.clone(),
            || now.get(),
            (run_cli_status_probe(cli.clone()), async {
                Ok::<_, CallError>(api_model(None))
            }),
            Some(&trace),
        )
        .await
        .unwrap();
        assert!(other_endpoint.local_source_confirmed);
        assert_eq!(fs::read_to_string(&cli_count).unwrap().lines().count(), 2);
        assert_eq!(trace.snapshot().stages.len(), 5);
        server.await.unwrap();

        let failure_cli_cache = ProbeCache::default();
        let failure_api_cache = ProbeCache::default();
        let failure = preflight_with_trace(
            &base,
            &failure_cli_cache,
            &failure_api_cache,
            cli_key.clone(),
            || now.get(),
            (run_cli_status_probe(cli.clone()), async {
                Err(CallError::Request)
            }),
            Some(&trace),
        )
        .await;
        assert!(matches!(failure, Err(LocalError::Unreachable)));
        assert_eq!(trace.snapshot().stages.len(), 7);
        let recovered = preflight_with_trace(
            &base,
            &failure_cli_cache,
            &failure_api_cache,
            cli_key,
            || now.get(),
            (run_cli_status_probe(cli.clone()), async {
                Ok::<_, CallError>(api_model(None))
            }),
            None,
        )
        .await
        .unwrap();
        assert!(recovered.local_source_confirmed);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn status_probe_records_real_cache_lock_wait_without_probe_on_hit() {
        use std::task::Poll;

        let cache = ProbeCache::default();
        let url = Url::parse("http://127.0.0.1:1234/api/v1/models").unwrap();
        let key = format!(
            "{}|{}|{}|{}",
            url.as_str(),
            MODEL_VARIANT,
            MODEL_INSTANCE_ID,
            CONTEXT_LENGTH
        );
        *cache.value.lock().await = Some((key, Instant::now(), api_model(None)));

        let metrics = crate::metrics::Metrics::default();
        let mut timer = metrics.stop_to_insert(
            crate::metrics::MetricGroup::unknown("dictation"),
            tokio_util::sync::CancellationToken::new(),
        );
        let trace = timer.bind_trace(77);
        let held = cache.value.lock().await;
        let future = cached_api_models(
            &cache,
            &url,
            Instant::now,
            async { panic!("valid cache hit must not execute the API probe") },
            Some(&trace),
        );
        tokio::pin!(future);
        std::future::poll_fn(|cx| {
            assert!(matches!(future.as_mut().poll(cx), Poll::Pending));
            Poll::Ready(())
        })
        .await;
        drop(held);
        let result = future.await.unwrap();
        assert!(result.1);
        let snapshot = trace.snapshot();
        assert_eq!(snapshot.stages.len(), 1);
        assert_eq!(
            snapshot.stages[0].0,
            crate::metrics::Stage::LmStudioStatusProbe
        );
    }
}
