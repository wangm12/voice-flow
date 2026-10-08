use std::sync::OnceLock;
use std::time::Duration;

pub async fn validate_key(key: &str) -> String {
    if crate::network_policy::ensure_cloud_allowed().is_err() {
        return "blocked_offline".into();
    }
    let cloud_cancellation = crate::network_policy::cloud_request_token();
    let key = key.trim();
    if key.is_empty() {
        return "invalid".into();
    }

    let client = match http_client() {
        Ok(client) => client,
        Err(error) => {
            log::warn!("Groq key validation client error: {error}");
            return "network_error".into();
        }
    };

    let request = client
        .get("https://api.groq.com/openai/v1/models")
        .bearer_auth(key)
        .send();
    let response = match tokio::select! {
        biased;
        _ = cloud_cancellation.cancelled() => return "blocked_offline".into(),
        result = request => result,
    } {
        Ok(response) => response,
        Err(error) => {
            log::warn!("Groq key validation network error: {error}");
            return "network_error".into();
        }
    };

    let status = response.status();
    if status == reqwest::StatusCode::OK {
        return "valid".into();
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return "invalid".into();
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return "rate_limited".into();
    }
    if status.is_server_error() {
        log::warn!("Groq key validation server error: {status}");
        return "server_error".into();
    }

    // Provider error bodies can contain account/request metadata. Keep them
    // out of logs; the UI only needs the stable status category.
    log::warn!("Groq key validation returned HTTP status {status}");
    "network_error".into()
}

fn http_client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    match CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent("VoiceFlow/0.1")
            .build()
            .map_err(|error| error.to_string())
    }) {
        Ok(client) => Ok(client),
        Err(error) => Err(error.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_key_is_invalid() {
        assert_eq!(validate_key("  ").await, "invalid");
    }

    #[tokio::test]
    #[ignore = "opt-in live validation of a configured Groq credential; prints status only"]
    async fn live_validate_configured_groq_key_file() {
        let path = std::env::var_os("VOICEFLOW_GROQ_VALIDATION_KEY_FILE")
            .expect("set VOICEFLOW_GROQ_VALIDATION_KEY_FILE to a readable key file");
        let key = std::fs::read_to_string(path).expect("read configured Groq key in memory");
        let status = validate_key(&key).await;
        assert!(
            matches!(
                status.as_str(),
                "valid" | "invalid" | "rate_limited" | "server_error" | "network_error"
            ),
            "unexpected sanitized validation status"
        );
        eprintln!("Groq key validation status: {status}");
    }
}
