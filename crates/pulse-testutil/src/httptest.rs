//! A tiny HTTP/1.1 test server for exercising HTTP clients in tests.
//!
//! ```ignore
//! let srv = httptest::Server::new(|req| {
//!     assert_eq!(req.method, "POST");
//!     Response::json(200, &serde_json::json!({"ok": true}))
//! });
//! let url = srv.url(); // "http://127.0.0.1:<port>"
//! ```
//!
//! Every response carries `Connection: close`, so clients open one connection per
//! request and the handler sees requests in the order the client sent them.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// One parsed request as the handler sees it.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub method: String,
    /// Path plus query string exactly as sent (`/v1/models?x=1`).
    pub uri: String,
    /// Header names lower-cased.
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    /// Path without the query string.
    pub fn path(&self) -> &str {
        self.uri.split('?').next().unwrap_or("")
    }

    /// Query string without the `?`, or `""`.
    pub fn query(&self) -> &str {
        self.uri.split_once('?').map(|(_, q)| q).unwrap_or("")
    }

    /// Header value (case-insensitive name).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    pub fn body_str(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> serde_json::Result<serde_json::Value> {
        serde_json::from_slice(&self.body)
    }
}

/// Response the handler returns.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16) -> Self {
        Response {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn text(status: u16, body: impl Into<String>) -> Self {
        Response::new(status)
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(body.into().into_bytes())
    }

    pub fn json<T: serde::Serialize + ?Sized>(status: u16, value: &T) -> Self {
        let mut body = serde_json::to_vec(value).expect("serialize test response");
        body.push(b'\n');
        Response::new(status)
            .header("Content-Type", "application/json")
            .body(body)
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }
}

type Handler = dyn Fn(&Request) -> Response + Send + Sync + 'static;

/// A listening test server; dropping it stops the listener.
pub struct Server {
    addr: String,
    closed: Arc<AtomicBool>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl Server {
    /// Starts a server on an ephemeral loopback port.
    pub fn new<F>(handler: F) -> Server
    where
        F: Fn(&Request) -> Response + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let addr = format!("http://{}", listener.local_addr().expect("local addr"));
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let closed = Arc::new(AtomicBool::new(false));
        let handler: Arc<Handler> = Arc::new(handler);
        let flag = closed.clone();
        let handle = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        // BSD/macOS accepted sockets inherit the listener's
                        // O_NONBLOCK; the per-connection reader must block.
                        let _ = stream.set_nonblocking(false);
                        let h = handler.clone();
                        std::thread::spawn(move || serve(stream, &*h));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        Server {
            addr,
            closed,
            handle: Mutex::new(Some(handle)),
        }
    }

    /// Base URL without a trailing slash.
    pub fn url(&self) -> String {
        self.addr.clone()
    }

    /// Stops accepting connections and joins the accept thread; `Drop` calls it too.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.lock().expect("server handle").take() {
            let _ = h.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.close();
    }
}

fn serve(mut stream: TcpStream, handler: &Handler) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let req = match read_request(&mut stream) {
        Some(r) => r,
        None => return,
    };
    let resp = handler(&req);
    let mut out = Vec::new();
    let _ = write!(out, "HTTP/1.1 {} {}\r\n", resp.status, reason(resp.status));
    let mut has_type = false;
    for (k, v) in &resp.headers {
        if k.eq_ignore_ascii_case("content-type") {
            has_type = true;
        }
        let _ = write!(out, "{k}: {v}\r\n");
    }
    if !has_type && !resp.body.is_empty() {
        let _ = write!(out, "Content-Type: text/plain; charset=utf-8\r\n");
    }
    let _ = write!(out, "Content-Length: {}\r\n", resp.body.len());
    let _ = write!(out, "Connection: close\r\n\r\n");
    out.extend_from_slice(&resp.body);
    let _ = stream.write_all(&out);
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.trim_end().splitn(3, ' ');
    let method = parts.next()?.to_string();
    let uri = parts.next()?.to_string();
    let mut headers = BTreeMap::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 {
            break;
        }
        let t = line.trim_end_matches(['\r', '\n']);
        if t.is_empty() {
            break;
        }
        if let Some((k, v)) = t.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let mut body = Vec::new();
    if let Some(n) = headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
    {
        body.resize(n, 0);
        reader.read_exact(&mut body).ok()?;
    } else if headers
        .get("transfer-encoding")
        .is_some_and(|v| v.eq_ignore_ascii_case("chunked"))
    {
        loop {
            line.clear();
            reader.read_line(&mut line).ok()?;
            let size = usize::from_str_radix(line.trim().split(';').next()?.trim(), 16).ok()?;
            if size == 0 {
                line.clear();
                let _ = reader.read_line(&mut line);
                break;
            }
            let mut chunk = vec![0; size];
            reader.read_exact(&mut chunk).ok()?;
            body.extend_from_slice(&chunk);
            line.clear();
            reader.read_line(&mut line).ok()?;
        }
    }
    Some(Request {
        method,
        uri,
        headers,
        body,
    })
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(url: &str, req: &str) -> String {
        let addr = url.trim_start_matches("http://");
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(req.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn serves_json_and_echoes_request() {
        let seen = Arc::new(Mutex::new(Vec::<Request>::new()));
        let log = seen.clone();
        let srv = Server::new(move |req| {
            log.lock().unwrap().push(req.clone());
            if req.path() == "/v1/models" {
                Response::json(200, &serde_json::json!({"data": [{"id": "m"}]}))
            } else {
                Response::text(404, "no")
            }
        });
        let got = raw(
            &srv.url(),
            "POST /v1/models?x=1 HTTP/1.1\r\nHost: t\r\nAuthorization: Bearer tok\r\nContent-Length: 7\r\n\r\n{\"a\":1}",
        );
        assert!(got.starts_with("HTTP/1.1 200 OK\r\n"), "{got}");
        assert!(got.contains("Content-Type: application/json"));
        assert!(got.ends_with("{\"data\":[{\"id\":\"m\"}]}\n"));
        let miss = raw(&srv.url(), "GET /nope HTTP/1.1\r\nHost: t\r\n\r\n");
        assert!(miss.starts_with("HTTP/1.1 404 Not Found\r\n"));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].method, "POST");
        assert_eq!(seen[0].query(), "x=1");
        assert_eq!(seen[0].header("authorization"), Some("Bearer tok"));
        assert_eq!(seen[0].json().unwrap()["a"], 1);
        srv.close();
    }
}
