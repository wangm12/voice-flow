//! Minimal local HTTP responder used by provider tests.
//! It deliberately lives behind `cfg(test)` and adds no runtime dependency.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

pub async fn spawn_response(
    status: u16,
    content_type: &str,
    body: impl Into<Vec<u8>>,
    extra_headers: &[(&str, &str)],
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local provider test server");
    let address = listener
        .local_addr()
        .expect("read local provider test address");
    let body = body.into();
    let content_type = content_type.to_owned();
    let headers = extra_headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect::<String>();

    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let _ = read_request_body(&mut stream).await;
        let reason = match status {
            200 => "OK",
            401 => "Unauthorized",
            403 => "Forbidden",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            _ => "Test Response",
        };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.write_all(&body).await;
    });

    format!("http://{address}")
}

pub async fn spawn_response_with_request_capture(
    status: u16,
    content_type: &str,
    body: impl Into<Vec<u8>>,
    extra_headers: &[(&str, &str)],
) -> (String, oneshot::Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local provider test server");
    let address = listener
        .local_addr()
        .expect("read local provider test address");
    let body = body.into();
    let content_type = content_type.to_owned();
    let headers = extra_headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect::<String>();
    let (sender, receiver) = oneshot::channel();

    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let request_body = read_request_body(&mut stream).await;
        let _ = sender.send(request_body);
        let reason = match status {
            200 => "OK",
            401 => "Unauthorized",
            403 => "Forbidden",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            _ => "Test Response",
        };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.write_all(&body).await;
    });

    (format!("http://{address}"), receiver)
}

/// Capture the full request, including authorization headers, for synthetic
/// local-provider routing tests. Callers must use synthetic credentials only.
pub async fn spawn_response_with_full_request_capture(
    status: u16,
    content_type: &str,
    body: impl Into<Vec<u8>>,
    extra_headers: &[(&str, &str)],
) -> (String, oneshot::Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local provider test server");
    let address = listener
        .local_addr()
        .expect("read local provider test address");
    let body = body.into();
    let content_type = content_type.to_owned();
    let headers = extra_headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect::<String>();
    let (sender, receiver) = oneshot::channel();

    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let request = read_full_request(&mut stream).await;
        let _ = sender.send(request);
        let reason = match status {
            200 => "OK",
            401 => "Unauthorized",
            403 => "Forbidden",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            _ => "Test Response",
        };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.write_all(&body).await;
    });

    (format!("http://{address}"), receiver)
}

/// Capture a bounded sequence of full requests on one loopback origin. This
/// models separate API routes on a shared Custom connection without involving
/// a live provider or credential store.
pub async fn spawn_responses_with_full_request_capture(
    responses: Vec<(u16, &'static str, Vec<u8>)>,
) -> (String, oneshot::Receiver<Vec<Vec<u8>>>) {
    assert!(!responses.is_empty() && responses.len() <= 8);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local provider test server");
    let address = listener
        .local_addr()
        .expect("read local provider test address");
    let (sender, receiver) = oneshot::channel();

    tokio::spawn(async move {
        let mut requests = Vec::with_capacity(responses.len());
        for (status, content_type, body) in responses {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            requests.push(read_full_request(&mut stream).await);
            let reason = match status {
                200 => "OK",
                401 => "Unauthorized",
                403 => "Forbidden",
                429 => "Too Many Requests",
                500 => "Internal Server Error",
                _ => "Test Response",
            };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.write_all(&body).await;
        }
        let _ = sender.send(requests);
    });

    (format!("http://{address}"), receiver)
}

async fn read_full_request(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 4096];
    let mut expected_body_len = None;
    loop {
        let Ok(read) = stream.read(&mut chunk).await else {
            return Vec::new();
        };
        if read == 0 {
            return request;
        }
        request.extend_from_slice(&chunk[..read]);
        if expected_body_len.is_none() {
            let Some(header_end) = request.windows(4).position(|value| value == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            expected_body_len = Some(
                headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0),
            );
        }
        let header_end = request
            .windows(4)
            .position(|value| value == b"\r\n\r\n")
            .unwrap_or_default();
        if request.len() >= header_end + 4 + expected_body_len.unwrap_or(0) {
            return request;
        }
    }
}

async fn read_request_body(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 4096];
    let mut expected_body_len = None;
    loop {
        let Ok(read) = stream.read(&mut chunk).await else {
            return Vec::new();
        };
        if read == 0 {
            return Vec::new();
        }
        request.extend_from_slice(&chunk[..read]);
        if expected_body_len.is_none() {
            let Some(header_end) = request.windows(4).position(|value| value == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            expected_body_len = Some(
                headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0),
            );
        }
        let header_end = request
            .windows(4)
            .position(|value| value == b"\r\n\r\n")
            .unwrap_or_default();
        if request.len() >= header_end + 4 + expected_body_len.unwrap_or(0) {
            return request[header_end + 4..header_end + 4 + expected_body_len.unwrap_or(0)]
                .to_vec();
        }
    }
}

/// Bounded scripted responses with request receipts independent of adapter completion.
/// The owned task is aborted on drop, including delayed-response cancellation tests.
pub struct ResponseStep {
    pub status: u16,
    pub body: String,
    pub delay: std::time::Duration,
}
pub struct ScriptedServer {
    pub url: String,
    pub receipts: tokio::sync::mpsc::Receiver<usize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for ScriptedServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub async fn spawn_scripted(steps: Vec<ResponseStep>) -> ScriptedServer {
    assert!(!steps.is_empty() && steps.len() <= 8);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("local script listener");
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (sender, receipts) = tokio::sync::mpsc::channel(8);
    let task = tokio::spawn(async move {
        for (index, step) in steps.into_iter().enumerate() {
            let Ok(Ok((mut stream, _))) =
                tokio::time::timeout(std::time::Duration::from_secs(10), listener.accept()).await
            else {
                return;
            };
            let Ok(body) = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                read_request_body(&mut stream),
            )
            .await
            else {
                return;
            };
            if body.is_empty() {
                return;
            }
            if sender.send(index + 1).await.is_err() {
                return;
            }
            tokio::time::sleep(step.delay).await;
            let response = format!("HTTP/1.1 {} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nRetry-After: 0\r\n\r\n{}", step.status, step.body.len(), step.body);
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    ScriptedServer {
        url,
        receipts,
        task,
    }
}
