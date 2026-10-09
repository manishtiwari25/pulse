use crate::httpx::{Request, ResponseWriter};
use anyhow::Result;
use hyper::header::{self, HeaderValue};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Component, Path};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) type JsonMap = BTreeMap<String, Value>;
pub type LoadHubFn = Box<dyn Fn() -> Result<()> + Send + Sync>;
pub type HandoverFn = Arc<dyn Fn(LoadHubFn) + Send + Sync>;
pub(crate) type ProviderCache = Arc<Mutex<BTreeMap<String, Arc<dyn pulse_provider::Provider>>>>;

/// Go: type Handler struct {
pub struct Handler {
    pub store: Option<Arc<pulse_store::Store>>,
    pub source: Option<Arc<dyn crate::board::TallySource>>,
    pub project: String,
    pub ui: Arc<dyn UiFs>,
    pub service: bool,
    pub port: i64,
    pub handover: Option<HandoverFn>,
    pub(crate) provider_cache: ProviderCache,
    /// Board snapshots and organization responses shared between requests.
    pub(crate) views: Arc<crate::view_cache::ViewCache>,
    /// How long a shared snapshot or organization response may be served
    /// after it was built; zero disables sharing (every request rebuilds).
    pub(crate) org_cache_ttl: Duration,
}

/// Go fs.FS-compatible UI filesystem.
pub trait UiFs: Send + Sync {
    fn read_file(&self, name: &str) -> std::io::Result<Vec<u8>>;
    fn is_dir(&self, name: &str) -> bool;
}

#[derive(rust_embed::RustEmbed)]
#[folder = "../../web/hub/out"]
pub struct Dist;

impl UiFs for Dist {
    fn read_file(&self, name: &str) -> std::io::Result<Vec<u8>> {
        Dist::get(name)
            .map(|f| f.data.into_owned())
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, name.to_string()))
    }
    fn is_dir(&self, name: &str) -> bool {
        let clean = name.trim_matches('/');
        let prefix = if clean.is_empty() {
            String::new()
        } else {
            format!("{clean}/")
        };
        Dist::iter().any(|p| p.starts_with(&prefix) && p.len() > prefix.len())
    }
}

impl Handler {
    /// Go: func (h *Handler) ServeHTTP(w http.ResponseWriter, r *http.Request)
    pub fn serve_http(&self, w: &mut ResponseWriter, r: &Request) {
        w.header_mut().insert(
            "x-content-type-options",
            HeaderValue::from_static("nosniff"),
        );
        w.header_mut()
            .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
        w.header_mut()
            .insert("x-frame-options", HeaderValue::from_static("DENY"));
        w.header_mut().insert(
            "content-security-policy",
            HeaderValue::from_static(
                "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'",
            ),
        );

        let p = r.path();
        if let Some(route) = p.strip_prefix("/api/v1/") {
            if route == "running" {
                self.addition_running(w, r);
                return;
            }
            if self.additions(w, r, route.to_string()) {
                return;
            }
            if let Some(rest) = route.strip_prefix("projects/")
                && let Some((slug, tail)) = rest.split_once('/')
            {
                self.project_api(w, r, slug.to_string(), tail.to_string());
                return;
            }
            if r.method == "POST" && (route == "decide" || route == "settings") {
                self.write_api(w, r, route.to_string());
                return;
            }
            if !read_method(w, r) {
                return;
            }
            if !self.project.is_empty() {
                self.project_api(w, r, self.project.clone(), route.to_string());
            } else {
                self.org_api(w, r, route.to_string());
            }
            return;
        }

        let mut legacy = p.trim_start_matches('/').to_string();
        if let Some(rest) = legacy.strip_prefix("p/") {
            legacy = rest.to_string();
        }
        if let Some((slug, route)) = legacy.split_once("/api/v1/") {
            self.project_api(w, r, slug.to_string(), route.to_string());
            return;
        }
        if p.contains("/api/") || p.starts_with("/api/") {
            write_error(
                w,
                404,
                "not found".into(),
                "No such API route.".into(),
                "Use /api/v1/.".into(),
            );
            return;
        }
        if !read_method(w, r) {
            return;
        }
        if p.ends_with('/')
            && let Some(store) = &self.store
        {
            let slug = legacy.trim_matches('/');
            if !slug.contains('/') && !slug.is_empty() {
                match store.project(slug) {
                    Ok(project) if project.is_some() || slug == pulse_store::COMMONS_SLUG => {
                        crate::httpx::http_redirect(
                            w,
                            r,
                            &format!(
                                "/project/?slug={}",
                                url::form_urlencoded::byte_serialize(slug.as_bytes())
                                    .collect::<String>()
                            ),
                            302,
                        );
                        return;
                    }
                    Ok(_) => {}
                    Err(err) => {
                        write_error(
                            w,
                            503,
                            "The registry is unavailable".into(),
                            err.to_string(),
                            "Restart pulse hub.".into(),
                        );
                        return;
                    }
                }
            }
        }
        self.static_(w, r);
    }

    /// Go: func (h *Handler) static(w http.ResponseWriter, r *http.Request)
    pub(crate) fn static_(&self, w: &mut ResponseWriter, r: &Request) {
        let mut name = clean_url_path(r.path());
        if name == "." || name.is_empty() {
            name = "index.html".into();
        } else if r.path().ends_with('/') {
            name.push_str("/index.html");
        }

        let data = match self.ui.read_file(&name) {
            Ok(data) => data,
            Err(err) => {
                if self.ui.is_dir(&name) {
                    match self.ui.read_file(&format!("{name}/index.html")) {
                        Ok(_) => {
                            let mut target = format!("/{}/", escape_url_path(&name));
                            if !r.raw_query().is_empty() || r.force_query() {
                                target.push('?');
                                target.push_str(r.raw_query());
                            }
                            crate::httpx::http_redirect(w, r, &target, 308);
                            return;
                        }
                        Err(index_err) if index_err.kind() != std::io::ErrorKind::NotFound => {
                            write_error(
                                w,
                                503,
                                "The UI could not be read".into(),
                                index_err.to_string(),
                                "Rebuild PULSE with the static UI export.".into(),
                            );
                            return;
                        }
                        Err(_) => {}
                    }
                }
                if err.kind() != std::io::ErrorKind::NotFound {
                    write_error(
                        w,
                        503,
                        "The UI could not be read".into(),
                        err.to_string(),
                        "Rebuild PULSE with the static UI export.".into(),
                    );
                    return;
                }
                match self
                    .ui
                    .read_file("404.html")
                    .or_else(|_| self.ui.read_file("index.html"))
                {
                    Ok(fallback) => {
                        name = if self.ui.read_file("404.html").is_ok() {
                            "404.html".into()
                        } else {
                            "index.html".into()
                        };
                        fallback
                    }
                    Err(err) => {
                        write_error(
                            w,
                            503,
                            "The hub UI is not built".into(),
                            err.to_string(),
                            "Run npm run build in web/hub, then rebuild pulse.".into(),
                        );
                        return;
                    }
                }
            }
        };
        if name.starts_with("_next/") {
            w.header_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            );
        } else {
            w.header_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        }
        let content_type = crate::httpx::mime_type_by_extension(
            Path::new(&name)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| format!(".{e}"))
                .unwrap_or_default()
                .as_str(),
        );
        if let Ok(value) = HeaderValue::from_str(&content_type) {
            w.header_mut().insert(header::CONTENT_TYPE, value);
        }
        crate::httpx::serve_content(w, r, &name, &data);
    }
}

/// Go: func New(s *store.Store, source TallySource) *Handler
pub fn new_handler(
    store: Option<Arc<pulse_store::Store>>,
    source: Option<Arc<dyn crate::board::TallySource>>,
) -> Handler {
    // Go: if source == nil && s != nil { source = nativeTally{s} }
    let source = source.or_else(|| {
        store.clone().map(|store| {
            Arc::new(crate::board::NativeTally { store }) as Arc<dyn crate::board::TallySource>
        })
    });
    Handler {
        store,
        source,
        project: String::new(),
        ui: Arc::new(Dist),
        service: false,
        port: 0,
        handover: None,
        provider_cache: Arc::new(Mutex::new(BTreeMap::new())),
        views: Arc::new(crate::view_cache::ViewCache::default()),
        org_cache_ttl: Duration::from_secs(60),
    }
}

/// Go: func writeJSON(w http.ResponseWriter, status int, payload any)
pub(crate) fn write_json(w: &mut ResponseWriter, status: i64, payload: Value) {
    w.header_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    w.header_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    w.write_header(status as u16);
    if let Ok(mut data) = pulse_store::gojson::to_vec(&payload) {
        data.push(b'\n');
        let _ = w.write(&data);
    }
}

/// Go: func writeError(w http.ResponseWriter, status int, what, why, fix string)
pub(crate) fn write_error(
    w: &mut ResponseWriter,
    status: i64,
    what: String,
    why: String,
    fix: String,
) {
    let mut payload = JsonMap::new();
    payload.insert("error".into(), Value::String(what));
    payload.insert("why".into(), Value::String(why));
    payload.insert("fix".into(), Value::String(fix));
    write_json(w, status, json!(payload));
}

/// Go: func readPayload(w http.ResponseWriter, r *http.Request) (map[string]any, bool)
pub(crate) fn read_payload(w: &mut ResponseWriter, r: &Request) -> (JsonMap, bool) {
    let origin = r.header("Origin");
    if !origin.is_empty() {
        let ok = url::Url::parse(&origin)
            .map(|u| {
                u.host_str()
                    .zip(u.port_or_known_default())
                    .map(|(host, port)| format!("{host}:{port}"))
                    .is_some_and(|h| h == r.host())
                    && matches!(u.scheme(), "http" | "https")
            })
            .unwrap_or(false);
        if !ok {
            write_error(
                w,
                403,
                "Cross-origin writes are not allowed".into(),
                "The request did not originate at this hub.".into(),
                "Open the hub on 127.0.0.1.".into(),
            );
            return (JsonMap::new(), false);
        }
    }
    if r.header("Sec-Fetch-Site") == "cross-site" {
        write_error(
            w,
            403,
            "Cross-origin writes are not allowed".into(),
            "The browser reports a cross-site request.".into(),
            "Open the hub on 127.0.0.1.".into(),
        );
        return (JsonMap::new(), false);
    }
    let content_type = r.header("Content-Type");
    let media = content_type.split(';').next().unwrap_or("").trim();
    if media != "application/json" {
        write_error(
            w,
            415,
            "Content-Type must be application/json".into(),
            "Only JSON writes are accepted.".into(),
            "Send Content-Type: application/json.".into(),
        );
        return (JsonMap::new(), false);
    }
    let data = &r.body;
    if data.is_empty() || data.len() > 16_384 {
        write_error(
            w,
            400,
            "JSON body must be 1..16384 bytes".into(),
            "The request body is outside the allowed size.".into(),
            "Send a smaller JSON object.".into(),
        );
        return (JsonMap::new(), false);
    }
    if std::str::from_utf8(data).is_err() {
        write_error(
            w,
            400,
            "body must be JSON".into(),
            "The request is not valid UTF-8.".into(),
            "Send a UTF-8 JSON object.".into(),
        );
        return (JsonMap::new(), false);
    }
    let mut de = serde_json::Deserializer::from_slice(data);
    let decoded = match Value::deserialize(&mut de) {
        Ok(v) => v,
        Err(_) => {
            write_error(
                w,
                400,
                "body must be JSON".into(),
                "The request body is malformed JSON.".into(),
                "Send a JSON object.".into(),
            );
            return (JsonMap::new(), false);
        }
    };
    let Value::Object(map) = decoded else {
        write_error(
            w,
            400,
            "body must be a JSON object".into(),
            "The body is not a JSON object.".into(),
            "Send an object with the documented fields.".into(),
        );
        return (JsonMap::new(), false);
    };
    if Value::deserialize(&mut de).is_ok() {
        write_error(
            w,
            400,
            "body must be JSON".into(),
            "The body contains data after the JSON object.".into(),
            "Send one JSON object.".into(),
        );
        return (JsonMap::new(), false);
    }
    (map.into_iter().collect(), true)
}

/// Go: func readMethod(w http.ResponseWriter, r *http.Request) bool
pub(crate) fn read_method(w: &mut ResponseWriter, r: &Request) -> bool {
    if r.method == "GET" || r.method == "HEAD" {
        return true;
    }
    w.header_mut()
        .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
    write_error(
        w,
        405,
        "Method not allowed".into(),
        "This route is read-only.".into(),
        "Use GET.".into(),
    );
    false
}

/// Go: func Serve(ctx context.Context, listener net.Listener, h http.Handler) error
pub fn serve(
    ctx: &pulse_bus::CancellationToken,
    listener: std::net::TcpListener,
    handler: crate::httpx::HandlerFn,
) -> Result<()> {
    crate::httpx::serve(ctx, listener, handler)
}

/// Go: func openBrowser(address string) error
pub(crate) fn open_browser(address: String) -> Result<()> {
    let command = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let status = std::process::Command::new(command).arg(address).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!("{command} exited {status}"))
    }
}

fn clean_url_path(p: &str) -> String {
    let joined = format!("/{p}");
    let mut parts = Vec::new();
    for component in Path::new(&joined).components() {
        match component {
            Component::Normal(s) => parts.push(s.to_string_lossy().to_string()),
            Component::ParentDir => {
                parts.pop();
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    }
}

fn escape_url_path(path: &str) -> String {
    let mut out = String::new();
    for &b in path.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl Handler {
    #[cfg(test)]
    pub(crate) fn clone_for_test(&self) -> Self {
        Self {
            store: self.store.clone(),
            source: self.source.clone(),
            project: self.project.clone(),
            ui: self.ui.clone(),
            service: self.service,
            port: self.port,
            handover: self.handover.clone(),
            provider_cache: self.provider_cache.clone(),
            views: self.views.clone(),
            org_cache_ttl: self.org_cache_ttl,
        }
    }
}

#[cfg(test)]
impl Clone for Handler {
    fn clone(&self) -> Self {
        self.clone_for_test()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::httpx::HandlerFn;
    use crate::test_helpers::MapFs;
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::thread;
    use tempfile::tempdir;

    fn handler_response(
        h: Handler,
        method: &str,
        path: &str,
        body: &str,
    ) -> (u16, String, hyper::HeaderMap) {
        let h = Arc::new(h);
        let server =
            crate::test_helpers::TestServer::start(Arc::new(move |w, r| h.serve_http(w, r)));
        let client = crate::test_helpers::client();
        let url = format!("{}{}", server.url, path);
        let req = match method {
            "HEAD" => client.head(&url),
            "POST" => client
                .post(&url)
                .header("Content-Type", "application/json")
                .body(body.to_string()),
            _ => client.get(&url),
        };
        let response = req.send().expect("send handler request");
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let text = if method == "HEAD" {
            String::new()
        } else {
            response.text().expect("read handler response")
        };
        (status, text, headers)
    }

    fn confirmed_request(
        h: Handler,
        route: &str,
        body: &str,
    ) -> (u16, serde_json::Map<String, Value>) {
        let h = Arc::new(h);
        let server =
            crate::test_helpers::TestServer::start(Arc::new(move |w, r| h.serve_http(w, r)));
        let client = crate::test_helpers::client();
        let response = client
            .post(format!("{}/api/v1/{}", server.url, route))
            .header("Content-Type", "application/json")
            .header("X-Pulse-Decide", "yes")
            .header("X-Pulse-Settings", "yes")
            .body(body.to_string())
            .send()
            .expect("send confirmed request");
        let status = response.status().as_u16();
        let text = response.text().expect("read confirmed response");
        let result: Value = serde_json::from_str(&text)
            .unwrap_or_else(|err| panic!("write returned invalid JSON: {status} {text}: {err}"));
        (status, result.as_object().expect("object response").clone())
    }

    fn map_fs(entries: &[(&str, &[u8])]) -> Arc<MapFs> {
        Arc::new(MapFs(
            entries
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_vec()))
                .collect::<BTreeMap<_, _>>(),
        ))
    }

    #[test]
    fn static_export() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_f3::additions_store(false);
        crate::test_helpers_f3::additions_project(&s, "one");
        let mut h = new_handler(Some(Arc::new((*s).clone())), None);
        h.ui = map_fs(&[
            ("index.html", b"home"),
            ("project/index.html", b"project"),
            ("records/index.html", b"records"),
            ("_next/static/main.js", b"asset"),
            ("_next/static/main.css", b"style"),
            ("404.html", b"not-found"),
        ]);
        for (path, body, cache) in [
            ("/", "home", "no-cache"),
            ("/project/?slug=one", "project", "no-cache"),
            ("/records/", "records", "no-cache"),
            (
                "/_next/static/main.js",
                "asset",
                "public, max-age=31536000, immutable",
            ),
            (
                "/_next/static/main.css",
                "style",
                "public, max-age=31536000, immutable",
            ),
            ("/unknown-path", "not-found", "no-cache"),
        ] {
            let (status, text, headers) = handler_response(h.clone_for_test(), "GET", path, "");
            assert_eq!(
                (
                    status,
                    text.as_str(),
                    headers["cache-control"].to_str().unwrap()
                ),
                (200, body, cache),
                "status/body/cache for {path}"
            );
        }
        for path in ["/one/", "/p/one/"] {
            let (status, _, headers) = handler_response(h.clone_for_test(), "GET", path, "");
            assert_eq!(status, 302, "legacy redirect status {path}");
            assert_eq!(headers["location"].to_str().unwrap(), "/project/?slug=one");
        }
        h.ui = map_fs(&[("index.html", b"fallback")]);
        let (status, text, _) = handler_response(h.clone_for_test(), "GET", "/absent", "");
        assert_eq!((status, text.as_str()), (200, "fallback"));
        for path in ["/api/v1/unknown", "/api/v2/tasks", "/one/api/v2/posts"] {
            let (status, text, headers) = handler_response(h.clone_for_test(), "GET", path, "");
            assert_eq!(status, 404, "API fell through to HTML: {path} {text}");
            assert!(
                headers["content-type"]
                    .to_str()
                    .unwrap()
                    .contains("application/json")
            );
        }
        let (status, text, _) = handler_response(h, "HEAD", "/", "");
        assert_eq!((status, text.as_str()), (200, ""));
    }

    #[test]
    fn project_presence_does_not_require_docs() {
        let _serial = crate::test_helpers::serial();
        let path = tempdir().expect("git dir");
        let out = Command::new(pulse_gitenv::git_program())
            .arg("init")
            .arg("-q")
            .arg(path.path())
            .env_clear()
            .envs(pulse_gitenv::env().into_iter().filter_map(|kv| {
                kv.split_once('=')
                    .map(|(k, v)| (k.to_string(), v.to_string()))
            }))
            .output()
            .expect("git init");
        assert!(
            out.status.success(),
            "git init: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(crate::board::present(&pulse_store::Project {
            path: path.path().display().to_string(),
            ..pulse_store::Project::default()
        }));
        let plain = tempdir().expect("plain dir");
        assert!(!crate::board::present(&pulse_store::Project {
            path: plain.path().display().to_string(),
            ..pulse_store::Project::default()
        }));
    }

    #[test]
    fn decide_and_settings_writes() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_f3::additions_store(false);
        crate::test_helpers_f3::additions_project(&s, "amistio-template");
        let h = new_handler(Some(Arc::new((*s).clone())), None);
        let (agent, _) = pulse_board::join_agent(
            &s,
            pulse_board::JoinInput {
                name: "hub-test".into(),
                runner: "copilot".into(),
                model: "test".into(),
                session: "hub-tests".into(),
                ..pulse_board::JoinInput::default()
            },
        )
        .expect("join agent");
        for (i, answer) in ["yes", "no"].iter().enumerate() {
            let post = pulse_board::create_post(
                &s,
                pulse_board::PostInput {
                    kind: "answer".into(),
                    project: "amistio-template".into(),
                    area: "hub".into(),
                    thread: format!("golden-decision-{i}"),
                    summary: "Test proposal".into(),
                    body: "Proposal for the human: ship <one> & two.".into(),
                    resolution: "agreed".into(),
                    agent_row: Some(agent.clone()),
                    ..pulse_board::PostInput::default()
                },
            )
            .expect("create post");
            let id_key = if i == 1 { "post_id" } else { "id" };
            let body = format!(
                r#"{{"{id_key}":"{}","answer":"{answer}","words":"Human decision"}}"#,
                post.id
            );
            let (code, data) = confirmed_request(h.clone_for_test(), "decide", &body);
            assert_eq!(code, 200, "decide {answer}: {data:#?}");
            assert_eq!(data["ok"], true);
            assert_eq!(data["answer"], *answer);
            let conn = s.conn().expect("conn");
            let (saved, actor, result): (String, String, String) = conn
                .query_row(
                    "SELECT answer,decided_by,result_post FROM decisions WHERE post_id=?",
                    [&post.id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .expect("decision row");
            assert_eq!(
                (saved.as_str(), actor.as_str(), result.as_str()),
                (*answer, "human", data["result_post"].as_str().unwrap())
            );
            let events: i64 = conn
                .query_row(
                    "SELECT count(*) FROM events WHERE action='decide' AND subject=?",
                    [&post.id],
                    |r| r.get(0),
                )
                .expect("event count");
            assert_eq!(events, 1);
            let jobs: i64 = conn
                .query_row(
                    "SELECT count(*) FROM jobs WHERE source=?",
                    [format!("decide:{}", post.id)],
                    |r| r.get(0),
                )
                .expect("job count");
            assert_eq!(jobs, 1 - i as i64, "decide {answer} queued {jobs} jobs");
        }
        let err =
            pulse_store::validate_setting("workers", "-1").expect_err("fixture should be invalid");
        let (code, data) = confirmed_request(
            h.clone_for_test(),
            "settings",
            r#"{"name":"workers","value":-1}"#,
        );
        assert_eq!(code, 400);
        assert_eq!(data["error"], err.to_string());
        let (code, data) = confirmed_request(h, "settings", r#"{"name":"workers","value":"3"}"#);
        assert_eq!(code, 200);
        assert_eq!(data["ok"], true);
        assert_eq!(s.setting("workers", ""), "3");
        let events: i64 = s
            .conn()
            .expect("conn")
            .query_row("SELECT count(*) FROM events WHERE action='setting' AND subject='workers' AND actor='human:dashboard'", [], |r| r.get(0))
            .expect("setting event count");
        assert!(events > 0, "missing CLI-compatible setting event");
    }

    #[test]
    fn write_guards_and_json() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_f3::additions_store(false);
        let h = new_handler(Some(Arc::new((*s).clone())), None);
        let cases = vec![
            (
                r#"{"name":"workers","value":1}"#.to_string(),
                "application/json",
                "",
                "",
                "",
                403,
            ),
            (
                r#"{"name":"workers","value":1}"#.to_string(),
                "text/plain",
                "yes",
                "",
                "",
                415,
            ),
            (
                r#"{"name":"workers","value":1}"#.to_string(),
                "application/json",
                "yes",
                "https://evil.invalid",
                "",
                403,
            ),
            (
                r#"{"name":"workers","value":1}"#.to_string(),
                "application/json",
                "yes",
                "",
                "cross-site",
                403,
            ),
            (r#"[]"#.to_string(), "application/json", "yes", "", "", 400),
            (
                r#"null"#.to_string(),
                "application/json",
                "yes",
                "",
                "",
                400,
            ),
            (
                r#"{"broken":"#.to_string(),
                "application/json",
                "yes",
                "",
                "",
                400,
            ),
            (
                String::from_utf8_lossy(b"{\"broken\":\"\xff\"}").to_string(),
                "application/json",
                "yes",
                "",
                "",
                400,
            ),
            (
                r#"{"name":"workers","value":1} {}"#.to_string(),
                "application/json",
                "yes",
                "",
                "",
                400,
            ),
            (
                r#"{"name":"workers","value":false}"#.to_string(),
                "application/json",
                "yes",
                "",
                "",
                400,
            ),
            (" ".repeat(16385), "application/json", "yes", "", "", 400),
        ];
        for (body, content_type, header, origin, site, code) in cases {
            let h = Arc::new(h.clone_for_test());
            let server =
                crate::test_helpers::TestServer::start(Arc::new(move |w, r| h.serve_http(w, r)));
            let status = crate::test_helpers::client()
                .post(format!("{}/api/v1/settings", server.url))
                .header("Content-Type", content_type)
                .header("X-Pulse-Settings", header)
                .header("Origin", origin)
                .header("Sec-Fetch-Site", site)
                .body(body)
                .send()
                .expect("guard request")
                .status()
                .as_u16();
            assert_eq!(status, code);
        }
        for route in ["decide", "settings"] {
            let (code, data) = confirmed_request(new_handler(None, None), route, "{}");
            assert_eq!(code, 409, "missing store {route}: {data:#?}");
            assert!(
                data["error"]
                    .as_str()
                    .unwrap()
                    .contains("store is not available")
            );
        }
        let (code, result) = confirmed_request(
            h.clone_for_test(),
            "settings",
            r#"{"name":"evolve.heldout_dir","value":"/private/owner-fixtures"}"#,
        );
        assert_eq!(code, 200, "owner's held-out directory update: {result:#?}");
        let data = h.settings().expect("settings");
        let encoded = pulse_store::gojson::to_string(&data).expect("settings json");
        assert!(!encoded.contains("/private/owner-fixtures"));
        let rows = data["settings"].as_array().expect("settings rows");
        let row = rows
            .iter()
            .find(|row| row["name"] == crate::privacy::HELDOUT_SETTING)
            .expect("heldout row");
        assert_eq!(row["secret"], true);
        assert_eq!(row["value"], crate::privacy::PRIVATE_VALUE);
        assert_eq!(
            s.setting(crate::privacy::HELDOUT_SETTING, ""),
            "/private/owner-fixtures"
        );
    }

    #[test]
    fn settings_storage_failure_is_not_bad_input() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_f3::additions_store(false);
        s.conn()
            .expect("conn")
            .execute_batch("PRAGMA query_only=ON")
            .expect("query only");
        let (code, data) = confirmed_request(
            new_handler(Some(Arc::new((*s).clone())), None),
            "settings",
            r#"{"name":"workers","value":"3"}"#,
        );
        assert_eq!(code, 500, "storage failure was misclassified: {data:#?}");
        assert!(
            data["why"]
                .as_str()
                .unwrap()
                .contains("store could not save")
        );
        assert_eq!(
            s.setting("workers", ""),
            pulse_store::DEFAULT_SETTINGS["workers"]
        );
    }

    #[test]
    fn graceful_shutdown() {
        let _serial = crate::test_helpers::serial();
        let listener = TcpListener::bind("127.0.0.1:0").expect("listen");
        let addr = listener.local_addr().expect("addr");
        let ctx = pulse_bus::CancellationToken::new();
        let (entered_tx, entered_rx) = mpsc::channel();
        let handler: HandlerFn = Arc::new(move |w, r| {
            if r.path() == "/wait" {
                let _ = entered_tx.send(());
                while !r.ctx.is_cancelled() {
                    thread::sleep(Duration::from_millis(10));
                }
                return;
            }
            let _ = w.write(b"ready");
        });
        let server_ctx = ctx.clone();
        let finished = thread::spawn(move || serve(&server_ctx, listener, handler));
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .expect("client");
        let response = client
            .get(format!("http://{addr}/"))
            .send()
            .expect("ready request");
        assert_eq!(response.status().as_u16(), 200);
        drop(response);
        let client2 = client.clone();
        let request_done = thread::spawn(move || {
            let _ = client2.get(format!("http://{addr}/wait")).send();
        });
        entered_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("request did not start");
        ctx.cancel();
        finished
            .join()
            .expect("server thread")
            .expect("server did not stop cleanly");
        request_done.join().expect("request thread");
        assert!(
            client.get(format!("http://{addr}")).send().is_err(),
            "listener remained open after shutdown"
        );
    }

    #[test]
    fn hub_commands() {
        let _serial = crate::test_helpers::serial();
        crate::register::register();
        for name in ["hub", "serve"] {
            let command =
                pulse_cli::lookup(name).unwrap_or_else(|| panic!("{name} not registered"));
            assert_eq!(command.group, "hub");
            for args in [
                vec!["--host".to_string(), "0.0.0.0".to_string()],
                vec!["--host".to_string(), "::1".to_string()],
                vec!["--port".to_string(), "-1".to_string()],
                vec!["--port".to_string(), "65536".to_string()],
            ] {
                let mut flags = pulse_cli::FlagSet::new(name);
                (command.flags.expect("flags"))(&mut flags);
                flags.parse(&args).expect("parse flags");
                let output = pulse_cli::Buffer::new();
                let mut ctx = pulse_cli::Context {
                    store: None,
                    home: String::new(),
                    out: Box::new(output.clone()),
                    err: Box::new(output),
                    json: false,
                    quiet: false,
                    color: false,
                    agent: String::new(),
                    project: String::new(),
                    session: String::new(),
                    now: String::new(),
                    args: Vec::new(),
                };
                let err = (command.run)(&mut ctx, &flags, &[]);
                assert!(
                    err.is_err(),
                    "{name} accepted unsafe configuration {args:?}"
                );
            }
        }
    }

    #[test]
    fn embedded_export() {
        let _serial = crate::test_helpers::serial();
        let h = new_handler(None, None);
        h.ui.read_file("index.html").expect("embedded index.html");
    }

    #[test]
    fn store_copy_is_outside_repository() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_f3::additions_store(false);
        let cwd = std::env::current_dir().expect("cwd");
        let home = std::path::Path::new(&s.home);
        assert!(
            !home.starts_with(&cwd),
            "test home must not be in the repository: {}",
            s.home
        );
    }

    #[test]
    fn watch_binary_fires_once_on_a_changed_stamp() {
        let _serial = crate::test_helpers::serial();
        let old_poll = {
            let mut poll = crate::register::RESTART_POLL.lock().expect("restart poll");
            let old = *poll;
            *poll = Duration::from_millis(5);
            old
        };
        struct Restore(Duration);
        impl Drop for Restore {
            fn drop(&mut self) {
                *crate::register::RESTART_POLL.lock().expect("restart poll") = self.0;
            }
        }
        let _restore = Restore(old_poll);
        let (tx, rx) = mpsc::sync_channel(2);
        let ctx = pulse_bus::CancellationToken::new();
        let watch_ctx = ctx.clone();
        thread::spawn(move || {
            crate::register::watch_binary(
                &watch_ctx,
                "0:0".into(),
                Arc::new(move || tx.send(()).unwrap()),
            )
        });
        rx.recv_timeout(Duration::from_secs(2))
            .expect("a changed stamp did not fire");
        assert!(
            rx.recv_timeout(Duration::from_millis(30)).is_err(),
            "fired twice"
        );
        let quiet = pulse_bus::CancellationToken::new();
        let quiet_watch = quiet.clone();
        let (stopped_tx, stopped_rx) = mpsc::channel();
        let unchanged_fired = Arc::new(AtomicBool::new(false));
        let unchanged = unchanged_fired.clone();
        thread::spawn(move || {
            crate::register::watch_binary(
                &quiet_watch,
                pulse_cli::binary_stamp(),
                Arc::new(move || unchanged.store(true, Ordering::SeqCst)),
            );
            stopped_tx.send(()).unwrap();
        });
        thread::sleep(Duration::from_millis(30));
        quiet.cancel();
        stopped_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("watch did not stop with its context");
        assert!(
            !unchanged_fired.load(Ordering::SeqCst),
            "an unchanged stamp fired"
        );
        assert!(
            !pulse_cli::binary_stamp().is_empty(),
            "BinaryStamp is empty for the test binary"
        );
    }

    #[test]
    fn static_export_slashless_pages() {
        let _serial = crate::test_helpers::serial();
        let h = new_handler(None, None);
        let query = "?slug=demo-app&tab=board&filter=a%2Fb&empty=";
        for route in [
            "/project",
            "/tasks",
            "/votes",
            "/records",
            "/compute",
            "/settings",
            "/agents",
            "/harness",
        ] {
            let want_body =
                h.ui.read_file(&format!("{}/index.html", route.trim_start_matches('/')))
                    .expect("exported page");
            for method in ["GET", "HEAD"] {
                let (status, body, headers) =
                    handler_response(h.clone_for_test(), method, &format!("{route}{query}"), "");
                let want_location = format!("{route}/{query}");
                assert_eq!(status, 308);
                assert_eq!(headers["location"].to_str().unwrap(), want_location);
                if method == "HEAD" {
                    assert_eq!(body, "");
                }
                let (status, body, headers) =
                    handler_response(h.clone_for_test(), method, &want_location, "");
                assert_eq!(status, 200);
                assert!(
                    headers["content-type"]
                        .to_str()
                        .unwrap()
                        .starts_with("text/html")
                );
                if method == "HEAD" {
                    assert_eq!(body, "");
                } else {
                    assert_eq!(body.as_bytes(), want_body.as_slice());
                }
            }
        }
    }

    #[test]
    fn static_export_directory_redirect_urls() {
        let _serial = crate::test_helpers::serial();
        let mut h = new_handler(None, None);
        h.ui = map_fs(&[
            ("nested/tasks/index.html", b"tasks"),
            ("page name/index.html", b"page"),
            ("404.html", b"not-found"),
        ]);
        for (request, location) in [
            ("/nested/tasks", "/nested/tasks/"),
            ("/nested/tasks?", "/nested/tasks/?"),
            ("/page%20name?filter=a%2Fb", "/page%20name/?filter=a%2Fb"),
            ("//nested/tasks", "/nested/tasks/"),
        ] {
            let (status, _, headers) = handler_response(h.clone_for_test(), "GET", request, "");
            assert_eq!(status, 308);
            assert_eq!(headers["location"].to_str().unwrap(), location);
        }
    }

    #[test]
    fn static_export_directory_without_index() {
        let _serial = crate::test_helpers::serial();
        let mut h = new_handler(None, None);
        h.ui = map_fs(&[
            ("_next/static/main.js", b"asset"),
            ("404.html", b"not-found"),
        ]);
        for route in ["/_next/static", "/_next/static/"] {
            let (status, body, headers) = handler_response(h.clone_for_test(), "GET", route, "");
            assert_eq!(status, 200);
            assert_eq!(body, "not-found");
            assert!(headers.get("location").is_none());
        }
    }

    #[test]
    fn static_export_unreadable_directory_index() {
        let _serial = crate::test_helpers::serial();
        struct UnreadableIndexFs(MapFs);
        impl UiFs for UnreadableIndexFs {
            fn read_file(&self, name: &str) -> std::io::Result<Vec<u8>> {
                if name == "tasks/index.html" {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        name.to_string(),
                    ));
                }
                self.0.read_file(name)
            }
            fn is_dir(&self, name: &str) -> bool {
                self.0.is_dir(name)
            }
        }
        let mut h = new_handler(None, None);
        h.ui = Arc::new(UnreadableIndexFs(MapFs(BTreeMap::from([
            ("tasks/index.html".to_string(), b"tasks".to_vec()),
            ("404.html".to_string(), b"not-found".to_vec()),
        ]))));
        for route in ["/tasks", "/tasks/"] {
            let (status, body, _) = handler_response(h.clone_for_test(), "GET", route, "");
            assert_eq!(status, 503);
            assert!(
                body.contains("The UI could not be read"),
                "index read error was hidden: {status} {body:?}"
            );
        }
    }
}
