use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{bail, Context, Result};
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
};

use crate::{
    k8s::{CHECKPOINT_PORT, OPENCODE_PORT, SUPERVISOR_PORT},
    state::WorkspaceState,
    AppState,
};

const MAX_HEADERS: usize = 32 * 1024;
const MAX_CONNECTIONS: usize = 1024;

#[derive(Debug, PartialEq, Eq)]
pub struct PreviewTarget {
    pub preview_key: String,
    pub port: u16,
}

pub fn parse_host(host: &str, base_domain: &str) -> Option<PreviewTarget> {
    let host = host.trim().trim_end_matches('.');
    let host = if host.starts_with('[') {
        return None;
    } else {
        host.rsplit_once(':')
            .filter(|(_, suffix)| suffix.parse::<u16>().is_ok())
            .map(|(name, _)| name)
            .unwrap_or(host)
    };
    let suffix = format!(".{base_domain}");
    let label = host.strip_suffix(&suffix)?;
    if label.contains('.') {
        return None;
    }
    let (key, port) = label.rsplit_once('-')?;
    let port = port.parse().ok()?;
    if key.is_empty()
        || port == 0
        || [OPENCODE_PORT, CHECKPOINT_PORT, SUPERVISOR_PORT].contains(&port)
    {
        return None;
    }
    Some(PreviewTarget {
        preview_key: key.into(),
        port,
    })
}

pub async fn serve(listener: TcpListener, state: AppState) -> Result<()> {
    let limit = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let (stream, peer) = listener.accept().await?;
        let permit = limit.clone().acquire_owned().await?;
        let state = state.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(error) = proxy(stream, peer, state).await {
                tracing::debug!(operation="preview", %peer, %error, "preview connection closed");
            }
        });
    }
}

async fn proxy(mut client: TcpStream, _peer: SocketAddr, state: AppState) -> Result<()> {
    let request = read_headers(&mut client).await?;
    let headers = parse_headers(&request)?;
    let host = headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .context("Host header is required")?;
    let Some(target) = parse_host(host, &state.config.base_domain) else {
        client
            .write_all(response(400, "invalid or reserved preview hostname").as_bytes())
            .await?;
        bail!("invalid preview hostname");
    };
    let owner = match state.auth.identity(&headers) {
        Ok(owner) => owner,
        Err(message) => {
            client.write_all(response(401, message).as_bytes()).await?;
            bail!(message);
        }
    };
    let workspace = state
        .store
        .workspace_by_preview_key(&target.preview_key)?
        .or(state.store.workspace(&target.preview_key)?);
    let Some(workspace) = workspace
        .filter(|workspace| workspace.owner == owner && workspace.state == WorkspaceState::Running)
    else {
        client
            .write_all(response(404, "preview workspace not found").as_bytes())
            .await?;
        bail!("preview authorization or state rejected");
    };
    let Some(ip) = state.k8s.ready_pod_ip(&workspace.preview_key).await else {
        client
            .write_all(response(503, "workspace is not ready").as_bytes())
            .await?;
        bail!("workspace has no ready Pod");
    };
    let address = format!("{ip}:{}", target.port);
    let mut backend =
        match tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(&address)).await {
            Ok(Ok(stream)) => stream,
            _ => {
                client
                    .write_all(
                        response(502, "nothing is listening on this preview port").as_bytes(),
                    )
                    .await?;
                bail!("preview backend unavailable");
            }
        };
    backend.write_all(&request).await?;
    tokio::io::copy_bidirectional(&mut client, &mut backend).await?;
    Ok(())
}

async fn read_headers(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(4096);
    let mut buffer = [0_u8; 1024];
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let count = stream.read(&mut buffer).await?;
            if count == 0 {
                bail!("connection closed before request headers");
            }
            bytes.extend_from_slice(&buffer[..count]);
            if bytes.len() > MAX_HEADERS {
                bail!("request headers exceed limit");
            }
            if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                return Ok(());
            }
        }
    })
    .await
    .context("request header timeout")??;
    Ok(bytes)
}

fn parse_headers(request: &[u8]) -> Result<HeaderMap> {
    let text = std::str::from_utf8(request).context("request headers are not UTF-8")?;
    let header_end = text
        .find("\r\n\r\n")
        .context("incomplete request headers")?;
    let mut lines = text[..header_end].split("\r\n");
    lines.next().context("request line is missing")?;
    let mut result = HeaderMap::new();
    for line in lines {
        let (name, value) = line.split_once(':').context("malformed request header")?;
        result.append(
            HeaderName::from_bytes(name.trim().as_bytes())?,
            HeaderValue::from_str(value.trim())?,
        );
    }
    Ok(result)
}

fn response(status: u16, message: &str) -> String {
    let reason = match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        502 => "Bad Gateway",
        _ => "Service Unavailable",
    };
    format!("HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{message}", message.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_workspace_then_port_hostname() {
        assert_eq!(
            parse_host("wrk-abc-18080.test.invalid", "test.invalid"),
            Some(PreviewTarget {
                preview_key: "wrk-abc".into(),
                port: 18080
            })
        );
    }

    #[test]
    fn rejects_wrong_domain_and_control_ports() {
        assert!(parse_host("wrk-4096.test.invalid", "test.invalid").is_none());
        assert!(parse_host("wrk-8080.test.invalid.evil", "test.invalid").is_none());
    }
}
