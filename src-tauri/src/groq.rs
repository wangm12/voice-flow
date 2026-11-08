use std::sync::OnceLock;
use std::time::Duration;

pub async fn validate_key(key: &str) -> String {
    validate_key_at_endpoint(key, "https://api.groq.com/openai/v1/models")
        .await
        .unwrap_or_else(str::to_owned)
}

async fn validate_key_at_endpoint(key: &str, endpoint: &str) -> Result<String, &'static str> {
    if crate::network_policy::ensure_cloud_allowed().is_err() {
        return Ok("blocked_offline".into());
    }
    let cloud_cancellation = crate::network_policy::cloud_request_token();
    let key = key.trim();
    if key.is_empty() {
        return Ok("invalid".into());
    }

    let client = match http_client() {
        Ok(client) => client,
        Err(error) => {
            log::warn!("Groq key validation client error: {error}");
            return Err("network_error");
        }
    };

    let request = client.get(endpoint).bearer_auth(key).send();
    let response = match tokio::select! {
        biased;
        _ = cloud_cancellation.cancelled() => return Ok("blocked_offline".into()),
        result = request => result,
    } {
        Ok(response) => response,
        Err(error) => {
            log::warn!("Groq key validation network error: {error}");
            return Err("network_error");
        }
    };

    let status = response.status();
    if status == reqwest::StatusCode::OK {
        return Ok("valid".into());
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Ok("invalid".into());
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Ok("rate_limited".into());
    }
    if status.is_server_error() {
        log::warn!("Groq key validation server error: {status}");
        return Ok("server_error".into());
    }

    // Provider error bodies can contain account/request metadata. Keep them
    // out of logs; the UI only needs the stable status category.
    log::warn!("Groq key validation returned HTTP status {status}");
    Err("network_error")
}

fn http_client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    match CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
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
        let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
        assert_eq!(validate_key("  ").await, "invalid");
    }

    #[tokio::test]
    async fn validation_rejects_redirect_without_second_request() {
        let _network_policy_guard = crate::network_policy::strict_offline_test_guard().await;
        let redirect_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redirect_address = redirect_listener.local_addr().unwrap();
        let (shutdown_redirect, redirect_shutdown) = tokio::sync::oneshot::channel();
        let redirect_server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            tokio::select! {
                _ = redirect_shutdown => 0,
                accepted = redirect_listener.accept() => {
                    let (mut stream, _) = accepted.expect("accept redirect target request");
                    let mut request = Vec::new();
                    let mut chunk = [0; 1024];
                    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                        let count = stream.read(&mut chunk).await.expect("read redirect request");
                        assert!(count > 0, "redirect request ended before its headers");
                        request.extend_from_slice(&chunk[..count]);
                        assert!(request.len() <= 16 * 1024, "redirect request headers are bounded");
                    }
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await
                        .expect("respond to redirect target");
                    1
                }
            }
        });

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let redirect_location = format!("http://{redirect_address}/redirect-target");
        let server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut stream, _) = listener.accept().await.expect("accept validation request");
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = stream
                    .read(&mut chunk)
                    .await
                    .expect("read validation request");
                assert!(count > 0, "validation request ended before its headers");
                request.extend_from_slice(&chunk[..count]);
                assert!(
                    request.len() <= 16 * 1024,
                    "validation request headers are bounded"
                );
            }
            let request = String::from_utf8_lossy(&request);
            assert!(request.starts_with("GET /models "));
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: {redirect_location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("respond with redirect");
            1
        });

        let endpoint = format!("http://{address}/models");
        let status = validate_key_at_endpoint("synthetic-test-key", &endpoint).await;
        let _ = shutdown_redirect.send(());
        let request_count = server.await.expect("validation server task joins");
        let redirect_request_count = redirect_server.await.expect("redirect server task joins");
        assert!(matches!(status, Err("network_error")));
        assert_eq!(request_count, 1);
        assert_eq!(redirect_request_count, 0);
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
