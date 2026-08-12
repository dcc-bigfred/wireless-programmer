//! Minimal HTTP/1.1 server for [`FakeDevice`](crate::FakeDevice) mocks.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

use crate::device::{FakeDevice, FakeRequest};

/// Namespace for the fake Soft-AP HTTP server.
pub struct FakeHttpServer;

impl FakeHttpServer {
    /// Accept connections and serve `device` until the listener fails.
    pub async fn serve(
        listener: TcpListener,
        device: Arc<Mutex<dyn FakeDevice>>,
    ) -> io::Result<()> {
        loop {
            let (stream, _) = listener.accept().await?;
            let device = Arc::clone(&device);
            tokio::spawn(async move {
                if let Err(e) = handle_connection(stream, device).await {
                    log::debug!("wp-fake connection error: {e}");
                }
            });
        }
    }
}

/// Bind `addr` (port `0` allowed), log the local address, spawn the accept
/// loop, and return the bound address.
pub async fn bind_and_serve(
    addr: SocketAddr,
    device: Arc<Mutex<dyn FakeDevice>>,
) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    log::info!("wp-fake listening on {local}");
    let device_clone = Arc::clone(&device);
    tokio::spawn(async move {
        if let Err(e) = FakeHttpServer::serve(listener, device_clone).await {
            log::error!("wp-fake server stopped: {e}");
        }
    });
    Ok(local)
}

async fn handle_connection(
    mut stream: TcpStream,
    device: Arc<Mutex<dyn FakeDevice>>,
) -> io::Result<()> {
    let mut buf = Vec::with_capacity(1024);
    let header_end = loop {
        let mut chunk = [0u8; 512];
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
        if buf.len() > 64 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP headers too large",
            ));
        }
    };

    let header = std::str::from_utf8(&buf[..header_end])
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let (method, path, content_length) = parse_request_line_and_headers(header)?;

    let body_start = header_end + 4;
    while buf.len() < body_start + content_length {
        let mut chunk = [0u8; 512];
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }

    let body = if content_length > 0 {
        Some(&buf[body_start..body_start + content_length.min(buf.len() - body_start)])
    } else {
        None
    };

    let response = {
        let mut guard = device.lock().await;
        guard.handle(FakeRequest {
            method: &method,
            path: &path,
            body,
        })
    };

    write_response(&mut stream, &response).await
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_request_line_and_headers(header: &str) -> io::Result<(String, String, usize)> {
    let mut lines = header.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "empty request"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing method"))?
        .to_string();
    let path = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing path"))?
        .to_string();
    // HTTP version ignored.

    let mut content_length = 0usize;
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            content_length = rest.trim().parse().unwrap_or(0);
        }
    }
    Ok((method, path, content_length))
}

async fn write_response(
    stream: &mut TcpStream,
    response: &crate::device::FakeResponse,
) -> io::Result<()> {
    let reason = match response.status {
        200 => "OK",
        404 => "Not Found",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        reason,
        response.content_type,
        response.body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&response.body).await?;
    stream.flush().await
}
