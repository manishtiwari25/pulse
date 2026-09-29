#![allow(unused_variables, dead_code, unused_imports)]
use crate::httpx::{Request, ResponseWriter};
use anyhow::Result;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

pub(crate) type JsonMap = BTreeMap<String, Value>;
pub type LoadHubFn = Box<dyn Fn() -> Result<()> + Send + Sync>;
pub type HandoverFn = Arc<dyn Fn(LoadHubFn) + Send + Sync>;

/// Go: type Handler struct {
pub struct Handler {
    pub store: Option<Arc<pulse_store::Store>>,
    pub source: Option<Arc<dyn crate::board::TallySource>>,
    pub project: String,
    pub ui: Arc<dyn UiFs>,
    pub service: bool,
    pub port: i64,
    pub handover: Option<HandoverFn>,
    pub(crate) provider_mu: Mutex<()>,
    pub(crate) provider_cache: Mutex<JsonMap>,
    pub(crate) org_mu: Mutex<()>,
    pub(crate) org_cache: Mutex<Vec<u8>>,
    pub(crate) org_cache_at: Mutex<Option<SystemTime>>,
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
        todo!("hub::server::serve_http")
    }

    /// Go: func (h *Handler) static(w http.ResponseWriter, r *http.Request)
    pub(crate) fn static_(&self, w: &mut ResponseWriter, r: &Request) {
        todo!("hub::server::static")
    }
}

/// Go: func New(s *store.Store, source TallySource) *Handler
pub fn new_handler(
    store: Option<Arc<pulse_store::Store>>,
    source: Option<Arc<dyn crate::board::TallySource>>,
) -> Handler {
    Handler {
        store,
        source,
        project: String::new(),
        ui: Arc::new(Dist),
        service: false,
        port: 0,
        handover: None,
        provider_mu: Mutex::new(()),
        provider_cache: Mutex::new(JsonMap::new()),
        org_mu: Mutex::new(()),
        org_cache: Mutex::new(Vec::new()),
        org_cache_at: Mutex::new(None),
        org_cache_ttl: Duration::from_secs(60),
    }
}

/// Go: func writeJSON(w http.ResponseWriter, status int, payload any)
pub(crate) fn write_json(w: &mut ResponseWriter, status: i64, payload: Value) {
    todo!("hub::server::write_json")
}

/// Go: func writeError(w http.ResponseWriter, status int, what, why, fix string)
pub(crate) fn write_error(
    w: &mut ResponseWriter,
    status: i64,
    what: String,
    why: String,
    fix: String,
) {
    todo!("hub::server::write_error")
}

/// Go: func readPayload(w http.ResponseWriter, r *http.Request) (map[string]any, bool)
pub(crate) fn read_payload(w: &mut ResponseWriter, r: &Request) -> (JsonMap, bool) {
    todo!("hub::server::read_payload")
}

/// Go: func readMethod(w http.ResponseWriter, r *http.Request) bool
pub(crate) fn read_method(w: &mut ResponseWriter, r: &Request) -> bool {
    todo!("hub::server::read_method")
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
    todo!("hub::server::open_browser")
}
