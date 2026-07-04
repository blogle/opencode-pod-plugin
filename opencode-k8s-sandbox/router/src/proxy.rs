use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::config::Config;
use crate::health::SharedMetrics;
use crate::index::SandboxIndex;

// Hostname pattern: {port}-{sandbox-id}. where sandbox-id is exactly 8 hex chars
// We require port to be 1-5 digits (matching spec 4.4 step 4)
static HOST_RE: once_cell::sync::Lazy<regex::Regex> =
    once_cell::sync::Lazy::new(|| regex::Regex::new(r"^(\d{1,5})-([0-9a-f]{8})\.").unwrap());

#[derive(Debug, PartialEq)]
pub struct ParsedHost {
    pub port: u16,
    pub sandbox_id: String,
}

/// Parse the hostname component into port and sandbox ID.
/// Returns None if the pattern doesn't match.
pub fn parse_host(host: &str) -> Option<ParsedHost> {
    let caps = HOST_RE.captures(host)?;
    let port: u16 = caps.get(1)?.as_str().parse().ok()?;
    let sandbox_id = caps.get(2)?.as_str().to_string();
    Some(ParsedHost { port, sandbox_id })
}

/// Handle a single proxied connection.
pub async fn handle_connection(
    mut stream: TcpStream,
    index: SandboxIndex,
    config: Config,
    metrics: SharedMetrics,
) {
    metrics.total_connections.inc();
    metrics.active_connections.inc();

    let result = handle_connection_inner(&mut stream, &index, &config, &metrics).await;

    metrics.active_connections.dec();

    if let Err(e) = result {
        tracing::debug!("connection error: {}", e);
    }
}

async fn handle_connection_inner(
    stream: &mut TcpStream,
    index: &SandboxIndex,
    config: &Config,
    metrics: &Metrics,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio::io::AsyncReadExt;

    // Read until we find \r\n\r\n (headers complete) or exceed max header size
    // Use a timeout to prevent slowloris-style attacks
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 1024];

    let header_timeout = std::time::Duration::from_millis(config.header_timeout_ms);
    let read_result = tokio::time::timeout(header_timeout, async {
        loop {
            let n = stream.read(&mut tmp).await?;
            if n == 0 {
                return Err::<usize, Box<dyn std::error::Error + Send + Sync>>("connection closed before headers complete".into());
            }
            buf.extend_from_slice(&tmp[..n]);

            if buf.len() > config.max_header_bytes {
                stream
                    .write_all(b"HTTP/1.1 400 Header Too Large\r\nContent-Length: 0\r\n\r\n")
                    .await?;
                return Err("header too large".into());
            }

            if let Some(pos) = find_header_end(&buf) {
                return Ok(pos);
            }
        }
    }).await;

    let header_end = match read_result {
        Ok(Ok(pos)) => pos,
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            stream
                .write_all(b"HTTP/1.1 408 Request Timeout\r\nContent-Length: 0\r\n\r\n")
                .await?;
            return Err("header read timeout".into());
        }
    };

    // Parse headers with httparse
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut req = httparse::Request::new(&mut headers);
    let status = req
        .parse(&buf)
        .map_err(|e| format!("httparse error: {}", e))?;

    if status.is_partial() {
        stream
            .write_all(b"HTTP/1.1 400 Partial Request\r\nContent-Length: 0\r\n\r\n")
            .await?;
        return Err("partial request".into());
    }

    // Extract Host header
    let host = match req
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("Host"))
    {
        Some(h) => h,
        None => {
            stream
                .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")
                .await?;
            return Err("missing Host header".into());
        }
    };
    let host_str = std::str::from_utf8(host.value)?;

    // Validate base domain if configured
    if let Some(ref base_domain) = config.base_domain {
        if !host_str.ends_with(base_domain) {
            stream
                .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")
                .await?;
            return Err("hostname does not match base domain".into());
        }
    }

    // Parse the hostname
    let parsed = match parse_host(host_str) {
        Some(p) => p,
        None => {
            stream
                .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")
                .await?;
            return Err("invalid hostname format".into());
        }
    };

    // Look up the sandbox in the index
    let pod_info = {
        let idx = index.read().await;
        idx.get(&parsed.sandbox_id).cloned()
    };

    let pod_info = match pod_info {
        Some(info) => info,
        None => {
            metrics.lookup_misses.inc();
            stream
                .write_all(b"HTTP/1.1 502 Sandbox Not Found\r\nContent-Length: 22\r\n\r\nsandbox not found or not ready")
                .await?;
            return Err("sandbox not found".into());
        }
    };

    // Dial the backend pod
    let backend_addr = format!("{}:{}", pod_info.pod_ip, parsed.port);
    let connect_timeout = std::time::Duration::from_millis(config.connect_timeout_ms);

    let mut backend = match timeout(connect_timeout, TcpStream::connect(&backend_addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            tracing::debug!("dial failed for {}: {}", backend_addr, e);
            stream
                .write_all(b"HTTP/1.1 502 Backend Unavailable\r\nContent-Length: 0\r\n\r\n")
                .await?;
            return Err("dial failed".into());
        }
        Err(_) => {
            tracing::debug!("dial timeout for {}", backend_addr);
            stream
                .write_all(b"HTTP/1.1 502 Backend Timeout\r\nContent-Length: 0\r\n\r\n")
                .await?;
            return Err("dial timeout".into());
        }
    };

    // Write the buffered header bytes AND any body bytes already read to the backend
    // header_end is the index of the last \n in \r\n\r\n, so we need header_end + 1
    // to include all bytes up to and including the header terminator
    backend.write_all(&buf[..=header_end]).await?;

    // If we read past the header end, those bytes are body data that must be forwarded
    if buf.len() > header_end + 1 {
        backend.write_all(&buf[header_end + 1..]).await?;
    }

    // Bidirectional copy for the rest of the connection
    let (bytes_copied, _) = tokio::io::copy_bidirectional(stream, &mut backend).await?;

    metrics.bytes_proxied.inc_by(bytes_copied);

    Ok(())
}

/// Find the position of \r\n\r\n in the buffer
fn find_header_end(buf: &[u8]) -> Option<usize> {
    for i in 0..buf.len().saturating_sub(3) {
        if buf[i] == b'\r' && buf[i + 1] == b'\n' && buf[i + 2] == b'\r' && buf[i + 3] == b'\n' {
            return Some(i + 3);
        }
    }
    None
}

use crate::health::Metrics;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_host_valid() {
        let result = parse_host("5173-a1b2c3d4.sandbox.test").unwrap();
        assert_eq!(result.port, 5173);
        assert_eq!(result.sandbox_id, "a1b2c3d4");
    }

    #[test]
    fn test_parse_host_port_8080() {
        let result = parse_host("8080-abc12345.example.com").unwrap();
        assert_eq!(result.port, 8080);
        assert_eq!(result.sandbox_id, "abc12345");
    }

    #[test]
    fn test_parse_host_invalid_no_port() {
        assert!(parse_host("abc12345.example.com").is_none());
    }

    #[test]
    fn test_parse_host_invalid_short_id() {
        assert!(parse_host("8080-abcd.example.com").is_none());
    }

    #[test]
    fn test_parse_host_invalid_long_id() {
        assert!(parse_host("8080-abcdefgh1.example.com").is_none());
    }

    #[test]
    fn test_parse_host_invalid_hex_chars() {
        assert!(parse_host("8080-xyzw1234.example.com").is_none());
    }

    #[test]
    fn test_parse_host_max_port_digits() {
        let result = parse_host("65535-a1b2c3d4.test").unwrap();
        assert_eq!(result.port, 65535);
    }

    #[test]
    fn test_parse_host_port_too_large() {
        // 7 digits exceeds the 1-5 digit constraint
        assert!(parse_host("1234567-a1b2c3d4.test").is_none());
    }

    #[test]
    fn test_find_header_end() {
        let buf = b"GET / HTTP/1.1\r\nHost: test\r\n\r\n";
        assert_eq!(find_header_end(buf), Some(29));
    }

    #[test]
    fn test_find_header_end_not_found() {
        let buf = b"GET / HTTP/1.1\r\nHost: test";
        assert!(find_header_end(buf).is_none());
    }
}
