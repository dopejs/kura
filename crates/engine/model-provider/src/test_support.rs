//! Helpers shared by the wire clients' tests.

/// Serves one scripted `(status, body)` per connection, in order, and
/// records each raw request.
pub(crate) fn spawn_scripted_endpoint(
    script: Vec<(u16, &'static str)>,
) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = std::sync::Arc::clone(&seen);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        for (stream, (status, body)) in listener.incoming().zip(script) {
            let Ok(mut stream) = stream else { continue };
            // Read the whole request: the body can arrive in a later
            // segment than the head, and the test asserts on the body.
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let read = stream.read(&mut buffer).unwrap_or(0);
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                let text = String::from_utf8_lossy(&request);
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let length = text[..head_end]
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())?
                        })
                        .unwrap_or(0);
                    if request.len() >= head_end + 4 + length {
                        break;
                    }
                }
            }
            recorded
                .lock()
                .expect("lock")
                .push(String::from_utf8_lossy(&request).to_string());
            let reason = if status == 200 { "OK" } else { "Bad Request" };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (format!("http://{addr}"), seen)
}
