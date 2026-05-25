//! Local HTTP ingest for host agents, sandbox samplers, and sidecars.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use serde_json::json;

use crate::collectors::adapters::local_push::accept_local_push_json;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::PluginOutput;

struct LocalHttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

pub fn start_local_report_server(addr: &str) -> Result<Receiver<PluginOutput>> {
    let listener = TcpListener::bind(addr)?;
    let (sender, receiver) = mpsc::channel();
    let addr = addr.to_string();

    thread::spawn(move || {
        println!(
            "{}",
            json!({
                "level": "info",
                "message": "local_report_server_started",
                "addr": addr,
                "endpoint": "/api/local/ingest",
            })
        );

        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let sender = sender.clone();
                    thread::spawn(move || handle_local_report_connection(stream, sender));
                }
                Err(error) => eprintln!(
                    "{}",
                    json!({
                        "level": "error",
                        "message": "local_report_accept_failed",
                        "error": error.to_string(),
                    })
                ),
            }
        }
    });

    Ok(receiver)
}

fn handle_local_report_connection(mut stream: TcpStream, sender: Sender<PluginOutput>) {
    match read_local_http_request(&mut stream) {
        Ok(request) => {
            if request.method == "GET" && request.path == "/health" {
                let _ = write_http_response(&mut stream, 200, "OK", r#"{"status":"ok"}"#);
                return;
            }

            if request.path != "/api/local/ingest" {
                let _ =
                    write_http_response(&mut stream, 404, "Not Found", r#"{"error":"not_found"}"#);
                return;
            }

            if request.method != "POST" {
                let _ = write_http_response(
                    &mut stream,
                    405,
                    "Method Not Allowed",
                    r#"{"error":"method_not_allowed"}"#,
                );
                return;
            }

            let ack = accept_local_push_json(&request.body, &sender);
            let _ = write_http_response(&mut stream, ack.status, ack.reason, &ack.body);
        }
        Err(error) => {
            let _ = write_http_response(
                &mut stream,
                400,
                "Bad Request",
                &json!({ "error": error.to_string() }).to_string(),
            );
        }
    }
}

fn read_local_http_request(stream: &mut TcpStream) -> Result<LocalHttpRequest> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let size = stream.read(&mut chunk)?;
        if size == 0 {
            return Err(CollectorError::Config(
                "connection closed before HTTP headers".to_string(),
            ));
        }
        buffer.extend_from_slice(&chunk[..size]);

        if buffer.len() > 1024 * 1024 {
            return Err(CollectorError::Config(
                "local report request is too large".to_string(),
            ));
        }

        if let Some(index) = find_header_end(&buffer) {
            break index;
        }
    };

    let header_bytes = &buffer[..header_end];
    let headers = String::from_utf8_lossy(header_bytes);
    let mut lines = headers.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| CollectorError::Config("missing HTTP request line".to_string()))?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| CollectorError::Config("missing HTTP method".to_string()))?
        .to_string();
    let path = parts
        .next()
        .ok_or_else(|| CollectorError::Config("missing HTTP path".to_string()))?
        .to_string();
    let content_length = lines
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);

    if content_length > 1024 * 1024 {
        return Err(CollectorError::Config(
            "local report body is too large".to_string(),
        ));
    }

    let body_start = header_end + 4;
    while buffer.len() < body_start + content_length {
        let size = stream.read(&mut chunk)?;
        if size == 0 {
            return Err(CollectorError::Config(
                "connection closed before HTTP body".to_string(),
            ));
        }
        buffer.extend_from_slice(&chunk[..size]);
    }

    Ok(LocalHttpRequest {
        method,
        path,
        body: buffer[body_start..body_start + content_length].to_vec(),
    })
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    body: &str,
) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes())
}
