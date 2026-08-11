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
