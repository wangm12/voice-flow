//! Guarded, native Ollama access for the explicitly selected local cleanup model.
//!
//! This module deliberately does not reuse the cloud-capable cleanup HTTP
//! client. Every request is restricted to a literal loopback host, and the
//! client disables both proxy discovery and redirects.

use reqwest::{redirect::Policy, Client, Method, StatusCode, Url};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;

pub const MODEL: &str = "qwen3.5:4b";

const MODEL_WITH_LOCAL_SUFFIX: &str = "qwen3.5:4b:local";
const MAX_STATUS_BODY_BYTES: usize = 512 * 1024;
const MAX_CHAT_BODY_BYTES: usize = 2 * 1024 * 1024;
// A conservative UTF-8 byte ceiling, not a token-count estimate. With at
// most 64 messages, this leaves half of the 8k context for the model's chat
// template, message framing, and separators under byte-fallback tokenization.
const MAX_INPUT_BYTES: usize = 4096;
const MAX_MESSAGES: usize = 64;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const METADATA_TIMEOUT: Duration = Duration::from_secs(8);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
const NUM_CTX: u32 = 8192;
const NUM_PREDICT: u32 = 2048;

#[derive(Debug, Clone, Serialize)]
pub struct LocalCleanupStatus {
    pub available: bool,
    pub status: String,
    pub message: String,
    pub model: String,
}

#[derive(Debug, Clone, Copy)]
enum FailureKind {
    InvalidEndpoint,
    OllamaUnreachable,
    ModelMissing,
    ModelRemote,
    UnsupportedServer,
    Failed,
}

impl FailureKind {
    fn status(self) -> &'static str {
        match self {
            Self::InvalidEndpoint => "invalid_endpoint",
            Self::OllamaUnreachable => "ollama_unreachable",
            Self::ModelMissing => "model_missing",
            Self::ModelRemote => "model_remote",
            Self::UnsupportedServer => "unsupported_server",
            Self::Failed => "failed",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::InvalidEndpoint => {
                "Use a local Ollama address on localhost, 127.0.0.1, or ::1."
            }
            Self::OllamaUnreachable => {
                "Ollama could not be reached at the saved local address. Start Ollama and check its address."
            }
            Self::ModelMissing => {
                "The local qwen3.5:4b model is not installed in Ollama. Install it locally, then check again."
            }
            Self::ModelRemote => {
                "This Ollama model may route to a remote host. Use a locally installed model and disable cloud routing on older Ollama versions."
            }
            Self::UnsupportedServer => {
                "This Ollama server does not provide the local API required for cleanup. Update Ollama and try again."
            }
            Self::Failed => "Local Ollama cleanup could not be completed. Check Ollama and try again.",
        }
    }

    fn status_result(self) -> LocalCleanupStatus {
        LocalCleanupStatus {
            available: false,
            status: self.status().to_owned(),
            message: self.message().to_owned(),
            model: MODEL.to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum CallError {
    Request,
    Http,
    NotFound,
    BodyTooLarge,
    InvalidJson,
}

#[derive(Debug)]
struct Preflight {
    model_for_chat: &'static str,
}

fn client() -> Result<&'static Client, FailureKind> {
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
        Err(()) => Err(FailureKind::Failed),
    }
}

fn allowed_loopback_host(url: &Url) -> bool {
    matches!(
        url.host_str(),
        Some(host)
            if host.eq_ignore_ascii_case("localhost")
                || host == "127.0.0.1"
                || host == "::1"
                || host == "[::1]"
    )
}

fn has_no_credentials_or_suffix(url: &Url) -> bool {
    url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

fn normalized_base(base_url: &str) -> Option<Url> {
    let mut url = Url::parse(base_url.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !allowed_loopback_host(&url)
        || !has_no_credentials_or_suffix(&url)
        || !matches!(url.path(), "" | "/" | "/v1")
    {
        return None;
    }

    // Native Ollama APIs live at the origin root even when a saved compatible
    // base URL includes the conventional `/v1` suffix.
    url.set_path("/");
    Some(url)
}

fn validate_endpoint(url: &Url, expected_path: &str) -> bool {
    matches!(url.scheme(), "http" | "https")
        && allowed_loopback_host(url)
        && has_no_credentials_or_suffix(url)
        && url.path() == expected_path
}

fn endpoint_from_base(base: &Url, path: &'static str) -> Option<Url> {
    let mut url = base.clone();
    url.set_path(path);
    validate_endpoint(&url, path).then_some(url)
}

fn endpoint(base_url: &str, path: &'static str) -> Option<Url> {
    let base = normalized_base(base_url)?;
    endpoint_from_base(&base, path)
}

fn parse_chat_endpoint(endpoint: &str) -> Option<Url> {
    let url = Url::parse(endpoint.trim()).ok()?;
    validate_endpoint(&url, "/api/chat").then_some(url)
}

fn base_from_chat_endpoint(endpoint: &Url) -> Url {
    let mut base = endpoint.clone();
    base.set_path("/");
    base
}

/// Build the native Ollama chat URL. An empty string means the saved address
/// did not pass loopback and path validation.
pub fn chat_endpoint(base_url: &str) -> String {
    endpoint(base_url, "/api/chat")
        .map(|url| url.to_string())
        .unwrap_or_default()
}

/// Validate an already-built native `/api/chat` URL before wiring it into the
/// cleanup route.
pub fn is_chat_endpoint(endpoint: &str) -> bool {
    parse_chat_endpoint(endpoint).is_some()
}

fn requested_model_is_supported(requested_model: &str) -> bool {
    requested_model == MODEL
}

fn local_status(available: bool, kind: FailureKind) -> LocalCleanupStatus {
    if available {
        LocalCleanupStatus {
            available: true,
            status: "available".to_owned(),
            message: "Local qwen3.5:4b cleanup is available.".to_owned(),
            model: MODEL.to_owned(),
        }
    } else {
        kind.status_result()
    }
}

/// Check only loopback Ollama metadata. No transcript or prompt is sent by
/// this operation.
pub async fn status(base_url: &str, requested_model: &str) -> LocalCleanupStatus {
    if !requested_model_is_supported(requested_model) {
        return local_status(false, FailureKind::Failed);
    }

    let Some(base) = normalized_base(base_url) else {
        return local_status(false, FailureKind::InvalidEndpoint);
    };

    match preflight(&base).await {
        Ok(_) => local_status(true, FailureKind::Failed),
        Err(kind) => local_status(false, kind),
    }
}

async fn preflight(base: &Url) -> Result<Preflight, FailureKind> {
    let version_url =
        endpoint_from_base(base, "/api/version").ok_or(FailureKind::InvalidEndpoint)?;
    let version_response = get_json(&version_url, MAX_STATUS_BODY_BYTES)
        .await
        .map_err(map_probe_error)?;
    let version = version_response
        .get("version")
        .and_then(Value::as_str)
        .filter(|version| !version.trim().is_empty())
        .ok_or(FailureKind::UnsupportedServer)?;
    let use_local_suffix = version_at_least_0344(version);

    let cloud_disabled = if use_local_suffix {
        None
    } else {
        let status_url =
            endpoint_from_base(base, "/api/status").ok_or(FailureKind::InvalidEndpoint)?;
        let server_status = get_json(&status_url, MAX_STATUS_BODY_BYTES)
            .await
            .map_err(map_probe_error)?;
        server_status
            .get("cloud")
            .and_then(|cloud| cloud.get("disabled"))
            .and_then(Value::as_bool)
    };

    let tags_url = endpoint_from_base(base, "/api/tags").ok_or(FailureKind::InvalidEndpoint)?;
    let tags = get_json(&tags_url, MAX_STATUS_BODY_BYTES)
        .await
        .map_err(map_probe_error)?;
    let models = tags
        .get("models")
        .and_then(Value::as_array)
        .ok_or(FailureKind::UnsupportedServer)?;

    // Fail closed if Ollama advertises any remote mapping. This check happens
    // before `/api/show` or any request containing user text.
    if models.iter().any(has_remote_mapping) {
        return Err(FailureKind::ModelRemote);
    }

    let installed = models
        .iter()
        .any(|model| model.get("name").and_then(Value::as_str) == Some(MODEL));
    if !installed {
        return Err(FailureKind::ModelMissing);
    }

    if !use_local_suffix && cloud_disabled != Some(true) {
        return Err(FailureKind::ModelRemote);
    }

    let show_url = endpoint_from_base(base, "/api/show").ok_or(FailureKind::InvalidEndpoint)?;
    let show_response = post_json(&show_url, &json!({ "model": MODEL }), MAX_STATUS_BODY_BYTES)
        .await
        .map_err(|error| match error {
            CallError::NotFound => FailureKind::ModelMissing,
            other => map_probe_error(other),
        })?;
    if has_remote_mapping(&show_response) {
        return Err(FailureKind::ModelRemote);
    }

    Ok(Preflight {
        model_for_chat: if use_local_suffix {
            MODEL_WITH_LOCAL_SUFFIX
        } else {
            MODEL
        },
    })
}

fn has_remote_mapping(value: &Value) -> bool {
    ["remote_model", "remote_host"]
        .iter()
        .any(|key| match value.get(*key) {
            Some(Value::String(value)) => !value.trim().is_empty(),
            Some(Value::Null) | None => false,
            Some(_) => true,
        })
}

fn version_at_least_0344(version: &str) -> bool {
    let version = version.trim().strip_prefix('v').unwrap_or(version.trim());
    let version = version.split('+').next().unwrap_or(version);
    let (core, prerelease) = version
        .split_once('-')
        .map_or((version, false), |(core, _)| (core, true));
    let mut components = core.split('.');
    let Some(major) = components.next().and_then(|part| part.parse::<u64>().ok()) else {
        return false;
    };
    let Some(minor) = components.next().and_then(|part| part.parse::<u64>().ok()) else {
        return false;
    };
    let Some(patch) = components.next().and_then(|part| part.parse::<u64>().ok()) else {
        return false;
    };
    if components.next().is_some() {
        return false;
    }

    (major, minor, patch) > (0, 34, 4) || ((major, minor, patch) == (0, 34, 4) && !prerelease)
}

fn map_probe_error(error: CallError) -> FailureKind {
    match error {
        CallError::Request => FailureKind::OllamaUnreachable,
        CallError::Http
        | CallError::NotFound
        | CallError::BodyTooLarge
        | CallError::InvalidJson => FailureKind::UnsupportedServer,
    }
}

async fn get_json(url: &Url, max_bytes: usize) -> Result<Value, CallError> {
    request_json(
        client().map_err(|_| CallError::Request)?,
        Method::GET,
        url,
        None,
        max_bytes,
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
    )
    .await
}

async fn request_json(
    client: &Client,
    method: Method,
    url: &Url,
    body: Option<&Value>,
    max_bytes: usize,
) -> Result<Value, CallError> {
    let expected_path = match url.path() {
        "/api/version" => "/api/version",
        "/api/status" => "/api/status",
        "/api/tags" => "/api/tags",
        "/api/show" => "/api/show",
        "/api/chat" => "/api/chat",
        _ => return Err(CallError::Request),
    };
    if !validate_endpoint(url, expected_path) {
        return Err(CallError::Request);
    }

    let timeout = if expected_path == "/api/chat" {
        REQUEST_TIMEOUT
    } else {
        METADATA_TIMEOUT
    };
    let mut request = client
        .request(method, url.clone())
        .header(reqwest::header::ACCEPT, "application/json")
        .timeout(timeout);
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request.send().await.map_err(|_| CallError::Request)?;
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
    while let Some(chunk) = response.chunk().await.map_err(|_| CallError::Request)? {
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(CallError::BodyTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Complete one local cleanup request using the already-validated native chat
/// endpoint supplied by the caller.
pub async fn complete_at_endpoint(
    endpoint: &str,
    requested_model: &str,
    messages: Vec<(String, String)>,
) -> Result<String, String> {
    if !requested_model_is_supported(requested_model) {
        return Err("This local cleanup model is not supported.".to_owned());
    }
    let chat_url = parse_chat_endpoint(endpoint)
        .ok_or_else(|| FailureKind::InvalidEndpoint.message().to_owned())?;
    validate_messages(&messages)?;
    let base = base_from_chat_endpoint(&chat_url);
    let preflight = preflight(&base)
        .await
        .map_err(|kind| kind.message().to_owned())?;

    let message_values: Vec<Value> = messages
        .into_iter()
        .map(|(role, content)| json!({ "role": role, "content": content }))
        .collect();
    let body = json!({
        "model": preflight.model_for_chat,
        "messages": message_values,
        "think": false,
        "stream": false,
        "options": {
            "num_ctx": NUM_CTX,
            "num_predict": NUM_PREDICT,
        },
        "keep_alive": "5m",
    });
    let response = post_json(&chat_url, &body, MAX_CHAT_BODY_BYTES)
        .await
        .map_err(map_chat_error)?;

    if has_remote_mapping(&response) || response.get("message").is_some_and(has_remote_mapping) {
        return Err(FailureKind::ModelRemote.message().to_owned());
    }
    if !is_normal_completion(&response) {
        return Err("Ollama did not finish a normal local cleanup response. Try again.".to_owned());
    }

    let content = response
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .filter(|content| !content.trim().is_empty())
        .ok_or_else(|| "Ollama returned no cleanup text. Try again.".to_owned())?;
    Ok(content.to_owned())
}

fn validate_messages(messages: &[(String, String)]) -> Result<(), String> {
    if messages.is_empty() || messages.len() > MAX_MESSAGES {
        return Err("The local cleanup request has an invalid number of messages.".to_owned());
    }
    let mut total_bytes = 0usize;
    let mut has_user_content = false;
    for (role, content) in messages {
        if !matches!(role.as_str(), "system" | "user" | "assistant") {
            return Err(
                "The local cleanup request contains an unsupported message role.".to_owned(),
            );
        }
        total_bytes = total_bytes.saturating_add(content.len());
        if total_bytes > MAX_INPUT_BYTES {
            return Err("The local cleanup request is too large.".to_owned());
        }
        if role == "user" && !content.trim().is_empty() {
            has_user_content = true;
        }
    }
    if !has_user_content {
        return Err("The local cleanup request contains no user text.".to_owned());
    }
    Ok(())
}

fn is_normal_completion(response: &Value) -> bool {
    if response.get("done").and_then(Value::as_bool) != Some(true)
        || response.get("done_reason").and_then(Value::as_str) != Some("stop")
        || response.get("error").is_some_and(|error| !error.is_null())
        || has_tool_data(response)
    {
        return false;
    }

    if let Some(eval_count) = response.get("eval_count") {
        let Some(eval_count) = eval_count.as_u64() else {
            return false;
        };
        if eval_count >= u64::from(NUM_PREDICT) {
            return false;
        }
    }

    response
        .get("message")
        .and_then(Value::as_object)
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
        == Some("assistant")
}

fn has_tool_data(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return true;
    };
    for key in ["tool_calls", "tool_call_id", "tool_name"] {
        match object.get(key) {
            None | Some(Value::Null) => {}
            Some(Value::Array(calls)) if calls.is_empty() => {}
            Some(_) => return true,
        }
    }
    if let Some(message) = object.get("message") {
        let Some(message) = message.as_object() else {
            return true;
        };
        for key in ["tool_calls", "tool_call_id", "tool_name"] {
            match message.get(key) {
                None | Some(Value::Null) => {}
                Some(Value::Array(calls)) if calls.is_empty() => {}
                Some(_) => return true,
            }
        }
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            return true;
        }
    } else {
        return true;
    }
    false
}

fn map_chat_error(error: CallError) -> String {
    match error {
        CallError::Request => FailureKind::OllamaUnreachable.message().to_owned(),
        CallError::NotFound => FailureKind::ModelMissing.message().to_owned(),
        CallError::Http => FailureKind::UnsupportedServer.message().to_owned(),
        CallError::BodyTooLarge => "Ollama returned too much cleanup data. Try again.".to_owned(),
        CallError::InvalidJson => FailureKind::UnsupportedServer.message().to_owned(),
    }
}
