use crate::httpx::{self, HandlerFn};
use crate::server::UiFs;
use pulse_bus::CancellationToken;
use std::collections::BTreeMap;
use std::net::TcpListener;
use std::sync::{Mutex, MutexGuard};
use std::thread;

pub(crate) struct TestServer {
    pub(crate) url: String,
    ctx: CancellationToken,
    join: Option<thread::JoinHandle<anyhow::Result<()>>>,
}

impl TestServer {
    pub(crate) fn start(handler: HandlerFn) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let url = format!(
            "http://{}",
            listener.local_addr().expect("test server address")
        );
        let ctx = CancellationToken::new();
        let server_ctx = ctx.clone();
        let join = thread::spawn(move || httpx::serve(&server_ctx, listener, handler));
        Self {
            url,
            ctx,
            join: Some(join),
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.ctx.cancel();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

pub(crate) fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("test client")
}

pub(crate) fn test_main_setup() {
    *pulse_tasks::LOGIN_SOURCE_CHECK
        .write()
        .expect("login source check lock poisoned") = Some(|_| true);
}

pub(crate) fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn hold_tempdir(dir: tempfile::TempDir) -> std::path::PathBuf {
    static DIRS: std::sync::LazyLock<Mutex<Vec<tempfile::TempDir>>> =
        std::sync::LazyLock::new(|| Mutex::new(Vec::new()));
    let path = dir.path().to_path_buf();
    DIRS.lock().unwrap_or_else(|e| e.into_inner()).push(dir);
    path
}

pub(crate) struct MapFs(pub(crate) BTreeMap<String, Vec<u8>>);

impl UiFs for MapFs {
    fn read_file(&self, name: &str) -> std::io::Result<Vec<u8>> {
        self.0
            .get(name)
            .cloned()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, name.to_string()))
    }
    fn is_dir(&self, name: &str) -> bool {
        let prefix = if name.is_empty() {
            String::new()
        } else {
            format!("{}/", name.trim_matches('/'))
        };
        self.0
            .keys()
            .any(|k| k.starts_with(&prefix) && k.len() > prefix.len())
    }
}
