#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use hyper::body::{Frame, Incoming};
use hyper::header::{self, HeaderName, HeaderValue};
use hyper::http::{HeaderMap, Method, Response, StatusCode};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use pulse_bus::CancellationToken;
use std::convert::Infallible;
use std::io;
use std::net::{SocketAddr, TcpListener};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use url::Url;

pub struct Request {
    pub method: String,
    pub url: Url,
    path: String,
    pub header: HeaderMap,
    pub body: Vec<u8>,
    pub remote_addr: SocketAddr,
    pub ctx: CancellationToken,
    pub force_query: bool,
}

impl Request {
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn raw_query(&self) -> &str {
        self.url.query().unwrap_or("")
    }
    pub fn force_query(&self) -> bool {
        self.force_query
    }
    pub fn query(&self, key: &str) -> String {
        self.query_all(key).into_iter().next().unwrap_or_default()
    }
    pub fn query_all(&self, key: &str) -> Vec<String> {
        self.url
            .query_pairs()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
            .collect()
    }
    pub fn header(&self, name: &str) -> String {
        self.header
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    }
    pub fn host(&self) -> String {
        self.header(header::HOST.as_str())
    }
}

struct Head {
    status: u16,
    headers: HeaderMap,
    rx: mpsc::UnboundedReceiver<Bytes>,
    ctx: CancellationToken,
}

struct ResponseInner {
    status: u16,
    wrote_header: bool,
    headers: HeaderMap,
    buffer: Vec<u8>,
    streaming: bool,
    head_tx: Option<oneshot::Sender<Head>>,
    body_tx: Option<mpsc::UnboundedSender<Bytes>>,
    body_rx: Option<mpsc::UnboundedReceiver<Bytes>>,
    ctx: CancellationToken,
}

pub struct ResponseWriter {
    inner: Arc<Mutex<ResponseInner>>,
}

impl ResponseWriter {
    fn new(ctx: CancellationToken, head_tx: oneshot::Sender<Head>) -> Self {
        let (body_tx, body_rx) = mpsc::unbounded_channel();
        Self {
            inner: Arc::new(Mutex::new(ResponseInner {
                status: 200,
                wrote_header: false,
                headers: HeaderMap::new(),
                buffer: Vec::new(),
                streaming: false,
                head_tx: Some(head_tx),
                body_tx: Some(body_tx),
                body_rx: Some(body_rx),
                ctx,
            })),
        }
    }

    pub fn header_mut(&mut self) -> impl std::ops::DerefMut<Target = HeaderMap> + '_ {
        parking_lot_style_header_guard(self.inner.lock().expect("response writer poisoned"))
    }

    pub fn write_header(&mut self, status: u16) {
        let mut inner = self.inner.lock().expect("response writer poisoned");
        if !inner.wrote_header {
            inner.status = status;
            inner.wrote_header = true;
        }
    }

    pub fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut inner = self.inner.lock().expect("response writer poisoned");
        if !inner.wrote_header {
            inner.status = 200;
            inner.wrote_header = true;
        }
        if inner.streaming {
            if let Some(tx) = &inner.body_tx {
                tx.send(Bytes::copy_from_slice(data)).map_err(|_| {
                    inner.ctx.cancel();
                    io::Error::new(io::ErrorKind::BrokenPipe, "client disconnected")
                })?;
            }
        } else {
            inner.buffer.extend_from_slice(data);
        }
        Ok(data.len())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        let mut inner = self.inner.lock().expect("response writer poisoned");
        if !inner.wrote_header {
            inner.status = 200;
            inner.wrote_header = true;
        }
        if !inner.streaming {
            inner.streaming = true;
            let Some(tx) = inner.head_tx.take() else {
                return Ok(());
            };
            let Some(rx) = inner.body_rx.take() else {
                return Ok(());
            };
            let head = Head {
                status: inner.status,
                headers: inner.headers.clone(),
                rx,
                ctx: inner.ctx.clone(),
            };
            let _ = tx.send(head);
        }
        if !inner.buffer.is_empty() {
            let bytes = Bytes::from(std::mem::take(&mut inner.buffer));
            if let Some(body_tx) = &inner.body_tx {
                body_tx.send(bytes).map_err(|_| {
                    inner.ctx.cancel();
                    io::Error::new(io::ErrorKind::BrokenPipe, "client disconnected")
                })?;
            }
        }
        Ok(())
    }

    fn finish(&mut self) {
        let mut inner = self.inner.lock().expect("response writer poisoned");
        if !inner.streaming {
            let Some(tx) = inner.head_tx.take() else {
                return;
            };
            let Some(rx) = inner.body_rx.take() else {
                return;
            };
            inner.streaming = true;
            let head = Head {
                status: if inner.wrote_header {
                    inner.status
                } else {
                    200
                },
                headers: inner.headers.clone(),
                rx,
                ctx: inner.ctx.clone(),
            };
            let body = Bytes::from(std::mem::take(&mut inner.buffer));
            if !body.is_empty()
                && let Some(body_tx) = &inner.body_tx
            {
                let _ = body_tx.send(body);
            }
            let _ = tx.send(head);
        }
        inner.body_tx.take();
    }
}

struct HeaderGuard<'a>(std::sync::MutexGuard<'a, ResponseInner>);
fn parking_lot_style_header_guard(g: std::sync::MutexGuard<'_, ResponseInner>) -> HeaderGuard<'_> {
    HeaderGuard(g)
}
impl std::ops::Deref for HeaderGuard<'_> {
    type Target = HeaderMap;
    fn deref(&self) -> &Self::Target {
        &self.0.headers
    }
}
impl std::ops::DerefMut for HeaderGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0.headers
    }
}

pub type HandlerFn = Arc<dyn Fn(&mut ResponseWriter, &Request) + Send + Sync>;

pub fn http_redirect(w: &mut ResponseWriter, r: &Request, target: &str, code: u16) {
    if let Ok(v) = HeaderValue::from_str(target) {
        w.header_mut().insert(header::LOCATION, v);
    }
    if r.method == "GET" {
        w.header_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        w.write_header(code);
        let escaped = target
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&#34;")
            .replace('\'', "&#39;");
        let _ = w.write(format!("<a href=\"{}\">{}</a>.\n", escaped, status_text(code)).as_bytes());
    } else {
        w.write_header(code);
    }
}

pub fn http_error(w: &mut ResponseWriter, msg: &str, code: u16) {
    w.header_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    w.header_mut().insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    w.write_header(code);
    let _ = w.write(format!("{}\n", msg).as_bytes());
}

pub fn status_text(code: u16) -> &'static str {
    match code {
        100 => "Continue",
        101 => "Switching Protocols",
        102 => "Processing",
        103 => "Early Hints",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        203 => "Non-Authoritative Information",
        204 => "No Content",
        205 => "Reset Content",
        206 => "Partial Content",
        207 => "Multi-Status",
        208 => "Already Reported",
        226 => "IM Used",
        300 => "Multiple Choices",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        305 => "Use Proxy",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Request Entity Too Large",
        414 => "Request URI Too Long",
        415 => "Unsupported Media Type",
        416 => "Requested Range Not Satisfiable",
        417 => "Expectation Failed",
        418 => "I'm a teapot",
        421 => "Misdirected Request",
        422 => "Unprocessable Entity",
        423 => "Locked",
        424 => "Failed Dependency",
        425 => "Too Early",
        426 => "Upgrade Required",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        451 => "Unavailable For Legal Reasons",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        506 => "Variant Also Negotiates",
        507 => "Insufficient Storage",
        508 => "Loop Detected",
        510 => "Not Extended",
        511 => "Network Authentication Required",
        _ => "",
    }
}

#[allow(clippy::collapsible_if)]
pub fn serve_content(w: &mut ResponseWriter, r: &Request, _name: &str, data: &[u8]) {
    let n = data.len() as u64;
    w.header_mut()
        .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    let mut start = 0u64;
    let mut end = n.saturating_sub(1);
    let mut status = 200u16;
    if let Some(range) = r.header.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        if let Some(spec) = range.strip_prefix("bytes=") {
            if !spec.contains(',') {
                let ok = if let Some((a, b)) = spec.split_once('-') {
                    if a.is_empty() {
                        b.parse::<u64>()
                            .ok()
                            .filter(|&suffix| suffix > 0)
                            .map(|suffix| {
                                start = n.saturating_sub(suffix);
                                end = n.saturating_sub(1);
                            })
                            .is_some()
                    } else if let Ok(s) = a.parse::<u64>() {
                        start = s;
                        end = if b.is_empty() {
                            n.saturating_sub(1)
                        } else {
                            b.parse::<u64>().unwrap_or(n)
                        };
                        start < n && start <= end && end < n
                    } else {
                        false
                    }
                } else {
                    false
                };
                if ok {
                    status = 206;
                    w.header_mut().insert(
                        header::CONTENT_RANGE,
                        HeaderValue::from_str(&format!("bytes {}-{}/{}", start, end, n)).unwrap(),
                    );
                } else {
                    w.header_mut().insert(
                        header::CONTENT_RANGE,
                        HeaderValue::from_str(&format!("bytes */{}", n)).unwrap(),
                    );
                    w.write_header(416);
                    return;
                }
            } else {
                w.header_mut().insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes */{}", n)).unwrap(),
                );
                w.write_header(416);
                return;
            }
        }
    }
    let len = if n == 0 { 0 } else { end - start + 1 };
    w.header_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&len.to_string()).unwrap(),
    );
    w.write_header(status);
    if r.method != "HEAD" && len > 0 {
        let _ = w.write(&data[start as usize..=end as usize]);
    }
}

pub fn mime_type_by_extension(ext: &str) -> String {
    let e = ext.to_ascii_lowercase();
    let exact = match e.as_str() {
        ".avif" => "image/avif",
        ".css" => "text/css; charset=utf-8",
        ".gif" => "image/gif",
        ".htm" | ".html" => "text/html; charset=utf-8",
        ".jpeg" | ".jpg" => "image/jpeg",
        ".js" | ".mjs" => "text/javascript; charset=utf-8",
        ".json" => "application/json",
        ".pdf" => "application/pdf",
        ".png" => "image/png",
        ".svg" => "image/svg+xml",
        ".wasm" => "application/wasm",
        ".webp" => "image/webp",
        ".xml" => "text/xml; charset=utf-8",
        _ => "",
    };
    if !exact.is_empty() {
        return exact.to_string();
    }
    let guessed = mime_guess::from_ext(e.trim_start_matches('.'))
        .first_raw()
        .unwrap_or("");
    if guessed.is_empty() {
        return String::new();
    }
    if guessed.starts_with("text/") && !guessed.contains("charset=") {
        format!("{}; charset=utf-8", guessed)
    } else {
        guessed.to_string()
    }
}

struct CancelOnDropStream {
    rx: mpsc::UnboundedReceiver<Bytes>,
    ctx: CancellationToken,
}
impl Stream for CancelOnDropStream {
    type Item = Result<Frame<Bytes>, Infallible>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.rx)
            .poll_recv(cx)
            .map(|v| v.map(|b| Ok(Frame::data(b))))
    }
}
impl Drop for CancelOnDropStream {
    fn drop(&mut self) {
        self.ctx.cancel();
    }
}

fn boxed_body(head: Head) -> BoxBody<Bytes, Infallible> {
    let stream = CancelOnDropStream {
        rx: head.rx,
        ctx: head.ctx,
    };
    BodyExt::boxed(StreamBody::new(stream))
}

async fn handle_request(
    req: hyper::Request<Incoming>,
    remote_addr: SocketAddr,
    root_ctx: CancellationToken,
    handler: HandlerFn,
) -> Result<Response<BoxBody<Bytes, Infallible>>, Infallible> {
    let req_ctx = root_ctx.child();
    let (parts, body) = req.into_parts();
    let body_bytes = match tokio::time::timeout(Duration::from_secs(30), body.collect()).await {
        Ok(Ok(collected)) => collected.to_bytes().to_vec(),
        _ => Vec::new(),
    };
    let target = parts.uri.to_string();
    let force_query = target.ends_with('?');
    let url = Url::parse(&format!(
        "http://hub{}",
        if target.starts_with('/') {
            target
        } else {
            format!("/{}", target)
        }
    ))
    .unwrap_or_else(|_| Url::parse("http://hub/").unwrap());
    let path = percent_decode_path(url.path());
    let request = Request {
        method: parts.method.as_str().to_string(),
        path,
        url,
        header: parts.headers,
        body: body_bytes,
        remote_addr,
        ctx: req_ctx.clone(),
        force_query,
    };
    let (head_tx, head_rx) = oneshot::channel();
    let mut writer = ResponseWriter::new(req_ctx.clone(), head_tx);
    let h = handler.clone();
    tokio::task::spawn_blocking(move || {
        (h)(&mut writer, &request);
        writer.finish();
    });
    let head = match head_rx.await {
        Ok(h) => h,
        Err(_) => Head {
            status: 500,
            headers: HeaderMap::new(),
            rx: mpsc::unbounded_channel().1,
            ctx: req_ctx,
        },
    };
    let mut builder = Response::builder()
        .status(StatusCode::from_u16(head.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR));
    for (k, v) in head.headers.iter() {
        builder = builder.header(k, v);
    }

    fn percent_decode_path(path: &str) -> String {
        let bytes = path.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%'
                && i + 2 < bytes.len()
                && let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
            {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
            out.push(bytes[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).to_string()
    }

    fn hex(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    Ok(builder.body(boxed_body(head)).unwrap())
}

pub fn serve(ctx: &CancellationToken, listener: TcpListener, handler: HandlerFn) -> Result<()> {
    let addr = listener.local_addr()?;
    if addr.ip().to_string() != "127.0.0.1" {
        return Err(anyhow!(
            "hub refuses non-loopback listeners; use --host 127.0.0.1 (remote serving is not supported)"
        ));
    }
    listener.set_nonblocking(true)?;
    let root = ctx.clone();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            if root.is_cancelled() { break; }
            tokio::select! {
                accepted = listener.accept() => {
                    let Ok((stream, remote)) = accepted else { continue; };
                    let h = handler.clone();
                    let c = root.clone();
                    tasks.spawn(async move {
                        let service = service_fn(move |req| handle_request(req, remote, c.clone(), h.clone()));
                        let mut builder = http1::Builder::new();
                        builder.timer(TokioTimer::new()).header_read_timeout(Duration::from_secs(5)).keep_alive(true).max_buf_size(1 << 20);
                        let _ = builder.serve_connection(TokioIo::new(stream), service).await;
                    });
                }
                // Reap closed connections as they finish; otherwise the set
                // keeps every finished task until shutdown.
                Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while !tasks.is_empty() && Instant::now() < deadline {
            tokio::select! { _ = tasks.join_next() => {}, _ = tokio::time::sleep(Duration::from_millis(10)) => {} }
        }
        Ok::<(), anyhow::Error>(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    fn start(handler: HandlerFn) -> (String, CancellationToken, thread::JoinHandle<Result<()>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let ctx = CancellationToken::new();
        let server_ctx = ctx.clone();
        let join = thread::spawn(move || serve(&server_ctx, listener, handler));
        (url, ctx, join)
    }

    fn client() -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
    }

    #[test]
    fn json_get_post_head_range_redirect() {
        let _serial = crate::test_helpers::serial();
        let handler: HandlerFn = Arc::new(|w, r| match r.path() {
            "/json" => {
                w.header_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
                let _ = w.write(br#"{"ok":true}"#);
            }
            "/echo" => {
                let _ = w.write(&r.body);
            }
            "/head" => serve_content(w, r, "x.txt", b"abcdef"),
            "/range" => serve_content(w, r, "x.txt", b"abcdef"),
            "/redirect" => http_redirect(w, r, "/json", 302),
            _ => http_error(w, "missing", 404),
        });
        let (url, ctx, join) = start(handler);
        let c = client();
        let resp = c.get(format!("{url}/json")).send().unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().unwrap(), r#"{"ok":true}"#);
        assert_eq!(
            c.post(format!("{url}/echo"))
                .body("hello")
                .send()
                .unwrap()
                .text()
                .unwrap(),
            "hello"
        );
        let resp = c.head(format!("{url}/head")).send().unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.headers().get("content-length").unwrap(), "6");
        let resp = c
            .get(format!("{url}/range"))
            .header("Range", "bytes=1-3")
            .send()
            .unwrap();
        assert_eq!(resp.status(), 206);
        assert_eq!(resp.headers().get("content-range").unwrap(), "bytes 1-3/6");
        assert_eq!(resp.text().unwrap(), "bcd");
        let resp = c.get(format!("{url}/redirect")).send().unwrap();
        assert_eq!(resp.status(), 302);
        assert_eq!(resp.headers().get("location").unwrap(), "/json");
        ctx.cancel();
        join.join().unwrap().unwrap();
    }

    #[test]
    fn sse_flush_and_disconnect_cancels_context() {
        let _serial = crate::test_helpers::serial();
        let seen_cancel = Arc::new(AtomicBool::new(false));
        let seen = seen_cancel.clone();
        let handler: HandlerFn = Arc::new(move |w, r| {
            w.header_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/event-stream"),
            );
            let _ = w.write(b"data: one\n\n");
            let _ = w.flush();
            std::thread::sleep(Duration::from_millis(100));
            let _ = w.write(b"data: two\n\n");
            let _ = w.flush();
            for _ in 0..50 {
                if r.ctx.is_cancelled() {
                    seen.store(true, Ordering::SeqCst);
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let (url, ctx, join) = start(handler);
        let resp = client().get(format!("{url}/sse")).send().unwrap();
        let mut reader = BufReader::new(resp);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, "data: one\n");
        drop(reader);
        std::thread::sleep(Duration::from_millis(500));
        assert!(seen_cancel.load(Ordering::SeqCst));
        ctx.cancel();
        join.join().unwrap().unwrap();
    }

    #[test]
    fn sse_flush_sends_first_chunk_before_second_write() {
        let _serial = crate::test_helpers::serial();
        let second_written = Arc::new(AtomicBool::new(false));
        let (first_seen_tx, first_seen_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let first_seen = Arc::new(Mutex::new(Some(first_seen_rx)));
        let flag = second_written.clone();
        let wait_for_first = first_seen.clone();
        let handler: HandlerFn = Arc::new(move |w, _r| {
            w.header_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/event-stream"),
            );
            let _ = w.write(b"data: 1\n\n");
            let _ = w.flush();
            let rx = wait_for_first.lock().expect("first seen lock").take();
            if let Some(rx) = rx {
                let _ = rx.recv_timeout(Duration::from_secs(30));
            }
            flag.store(true, Ordering::SeqCst);
            let _ = w.write(b"data: 2\n\n");
            let _ = w.flush();
        });
        let (url, ctx, join) = start(handler);
        let addr = url.trim_start_matches("http://");
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        stream
            .write_all(
                format!("GET /sse HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .unwrap();

        let mut buf = [0u8; 1024];
        let mut got = Vec::new();
        // Generous bound: under a loaded scheduler (full-suite pre-commit runs) the
        // handler thread may take well over a second to be scheduled; the test only
        // needs the first chunk to arrive before the second write, not quickly.
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline && !String::from_utf8_lossy(&got).contains("data: 1\n\n") {
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue;
                }
                Err(e) => panic!("socket read: {e}"),
            }
        }
        first_seen_tx.send(()).unwrap();
        let text = String::from_utf8_lossy(&got);
        assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");
        assert!(text.contains("data: 1\n\n"), "{text}");
        assert!(
            !second_written.load(Ordering::SeqCst),
            "first chunk was not observed before the second write"
        );

        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline && !String::from_utf8_lossy(&got).contains("data: 2\n\n") {
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue;
                }
                Err(e) => panic!("socket read: {e}"),
            }
        }
        let text = String::from_utf8_lossy(&got);
        assert!(text.contains("data: 2\n\n"), "{text}");
        ctx.cancel();
        join.join().unwrap().unwrap();
    }

    #[test]
    fn serve_refuses_non_loopback_listener() {
        let _serial = crate::test_helpers::serial();
        let listener = TcpListener::bind("0.0.0.0:0").unwrap();
        let ctx = CancellationToken::new();
        let handler: HandlerFn = Arc::new(|_, _| {});
        let err = serve(&ctx, listener, handler).unwrap_err();
        assert_eq!(
            err.to_string(),
            "hub refuses non-loopback listeners; use --host 127.0.0.1 (remote serving is not supported)"
        );
        ctx.cancel();
    }
}
