use std::sync::OnceLock;
use std::time::Duration;

pub async fn validate_key(key: &str) -> String {
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

    let response = match client
        .get("https://api.groq.com/openai/v1/models")
        .bearer_auth(key)
        .send()
        .await
    {
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
}
