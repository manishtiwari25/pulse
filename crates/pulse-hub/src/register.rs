use crate::httpx::{HandlerFn, Request, ResponseWriter};
use crate::server::{LoadHubFn, new_handler, open_browser, serve};
use anyhow::{Result, anyhow};
use pulse_cli::flags::FlagSet;
use std::io::Write;
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

pub(crate) static RESTART_POLL: std::sync::LazyLock<Mutex<Duration>> =
    std::sync::LazyLock::new(|| Mutex::new(Duration::from_secs(30)));
/// How long a foreground hub keeps serving after it answers
/// the service-install request, so the page receives the response before the port moves.
pub(crate) static HANDOVER_DELAY: std::sync::LazyLock<Mutex<Duration>> =
    std::sync::LazyLock::new(|| Mutex::new(Duration::from_millis(300)));

/// SIGTERM and SIGINT cancel the token while the guard lives; dropping it
/// unregisters the handlers again.
struct SignalStop {
    handle: signal_hook::iterator::Handle,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SignalStop {
    fn notify(cancel: &pulse_bus::CancellationToken) -> Result<Self> {
        let mut signals = signal_hook::iterator::Signals::new([
            signal_hook::consts::SIGTERM,
            signal_hook::consts::SIGINT,
        ])?;
        let handle = signals.handle();
        let cancel = cancel.clone();
        let thread = std::thread::Builder::new()
            .name("hub-signals".into())
            .spawn(move || {
                if signals.forever().next().is_some() {
                    cancel.cancel();
                }
            })?;
        Ok(Self {
            handle,
            thread: Some(thread),
        })
    }
}

impl Drop for SignalStop {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub(crate) fn watch_binary(
    ctx: &pulse_bus::CancellationToken,
    stamp: String,
    on_change: Arc<dyn Fn() + Send + Sync>,
) {
    if stamp.is_empty() {
        return;
    }
    while !ctx.is_cancelled() {
        let poll = *RESTART_POLL.lock().expect("restart poll lock poisoned");
        std::thread::sleep(poll);
        if ctx.is_cancelled() {
            return;
        }
        let now = pulse_cli::binary_stamp();
        if !now.is_empty() && now != stamp {
            on_change();
            return;
        }
    }
}
static REGISTER_ONCE: std::sync::Once = std::sync::Once::new();
pub fn register() {
    REGISTER_ONCE.call_once(|| {
        for name in ["hub", "serve"] {
            pulse_cli::register(pulse_cli::Command {
                name: name.to_string(),
                group: "hub".into(),
                summary: "Serve the local owner console".into(),
                usage: format!("pulse {name} [--project SLUG] [--port N] [--no-browser] [--host 127.0.0.1] [--restart-on-change]"),
                aliases: vec![],
                no_store: false,
                read_only: false,
                raw: false,
                flags: Some(hub_flags),
                run: if name == "hub" { run_hub } else { run_serve },
            });
        }
    })
}

/// The bind failure as the hub reports it, "listen tcp4 127.0.0.1:8765: bind:
/// address already in use": the errno spelled as libc's strerror, with a
/// lower-case initial and without the "(os error N)" suffix.
fn listen_error(host: &str, port: i64, err: &std::io::Error) -> String {
    let text = err.to_string();
    let text = match text.rfind(" (os error ") {
        Some(i) => &text[..i],
        None => text.as_str(),
    };
    let mut chars = text.chars();
    let reason = match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect::<String>(),
        None => String::new(),
    };
    format!("listen tcp4 {host}:{port}: bind: {reason}")
}

fn hub_flags(fs: &mut FlagSet) {
    fs.int_var("port", 8765, "TCP port (0 chooses a free port)");
    fs.string_var("host", "127.0.0.1", "listen address (127.0.0.1 only)");
    fs.bool_var("no-browser", false, "do not open the browser");
    fs.bool_var(
        "restart-on-change",
        false,
        "exit 3 when the pulse binary changes on disk (the user service restarts it)",
    );
    fs.string_var("project", "", "project slug for serve");
}

fn run_hub(c: &mut pulse_cli::Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    run_command("hub", c, fs, args)
}

fn run_serve(c: &mut pulse_cli::Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    run_command("serve", c, fs, args)
}

fn run_command(
    name: &str,
    c: &mut pulse_cli::Context,
    fs: &FlagSet,
    args: &[String],
) -> Result<()> {
    if !args.is_empty() {
        return Err(pulse_cli::errorf(
            2,
            "Unexpected arguments",
            "hub and serve take flags only",
            &format!("pulse help {name}"),
        )
        .into());
    }
    let host = fs.get_string("host");
    if host != "127.0.0.1" {
        return Err(pulse_cli::errorf(
            2,
            "Remote hub serving is not supported",
            "The owner write routes are loopback-only; hub.allow_remote is not supported yet",
            &format!("pulse {name} --host 127.0.0.1"),
        )
        .into());
    }
    let port = i64::from(fs.get_int("port"));
    if !(0..=65535).contains(&port) {
        return Err(pulse_cli::errorf(
            2,
            "Invalid port",
            "The port must be between 0 and 65535",
            &format!("pulse {name} --port 8765"),
        )
        .into());
    }

    // ADR-038: a store the console opens has the framework's records before anything reads them.
    let store = c
        .store
        .clone()
        .ok_or_else(|| anyhow!("{name}: store is needed"))?;
    match pulse_records::ensure_framework(&store, pulse_store::HUMAN_AGENT) {
        Err(err) => writeln!(c.err, "framework records: {err:#}")?,
        Ok(seeded) if !seeded.written.is_empty() => writeln!(
            c.out,
            "seeded {} framework record(s) from {}",
            seeded.written.len(),
            seeded.from
        )?,
        Ok(_) => {}
    }
    let mut h = new_handler(Some(Arc::new(store)), None);
    h.service = fs.get_bool("restart-on-change");
    if name == "serve" {
        h.project = fs.get_string("project");
        if h.project.is_empty() {
            h.project = c.project.clone();
        }
        if h.project.is_empty() {
            let cwd = std::env::current_dir()?;
            if let Some(s) = &h.store
                && let Some(p) = s.project_for_dir(cwd.to_string_lossy().as_ref())?
            {
                h.project = p.slug;
            }
        }
        if h.project.is_empty() {
            return Err(pulse_cli::errorf(
                2,
                "No project selected",
                "This directory is not a registered project",
                "pulse serve --project SLUG",
            )
            .into());
        }
        let _ = h.entry(h.project.clone())?;
    }

    let listener = TcpListener::bind((host.as_str(), port as u16)).map_err(|err| {
        pulse_cli::errorf(
            1,
            "The hub could not listen",
            &listen_error(&host, port, &err),
            &format!("pulse {name} --port 0"),
        )
    })?;
    h.port = i64::from(listener.local_addr()?.port());
    let address = format!("http://{}", listener.local_addr()?);
    writeln!(
        c.out,
        "Serving on {address} (127.0.0.1 only; writes: decide, settings, tasks, messages, compute; Ctrl-C to stop)"
    )?;
    if !fs.get_bool("no-browser")
        && let Err(err) = open_browser(address.clone())
    {
        writeln!(
            c.err,
            "Could not open the browser: {err}; open {address} manually"
        )?;
    }

    let ctx = pulse_bus::CancellationToken::new();
    let _stop = SignalStop::notify(&ctx)?;
    // Board snapshots past the view-cache TTL can never be served again;
    // drop them on a timer so an idle hub does not keep every board it built.
    {
        let views = h.views.clone();
        let ttl = h.org_cache_ttl;
        let evict_ctx = ctx.clone();
        std::thread::Builder::new()
            .name("hub-evict".into())
            .spawn(move || crate::view_cache::evict_loop(&evict_ctx, &views, ttl))?;
    }
    // The watcher and handover threads only flag what happened; their report
    // lines are written once `serve` returns, the first point at which
    // `c.out`/`c.err` are used again.
    let changed = Arc::new(AtomicBool::new(false));
    if fs.get_bool("restart-on-change") {
        let watch_ctx = ctx.clone();
        let cancel = ctx.clone();
        let changed = changed.clone();
        std::thread::spawn(move || {
            watch_binary(
                &watch_ctx,
                pulse_cli::binary_stamp(),
                Arc::new(move || {
                    changed.store(true, Ordering::SeqCst);
                    cancel.cancel();
                }),
            )
        });
    }
    // ADR-038: the console installed the hub service from this foreground hub;
    // stop serving so the service's hub can bind this port, then load its unit.
    let handover: Arc<Mutex<Option<LoadHubFn>>> = Arc::new(Mutex::new(None));
    {
        let handover = handover.clone();
        let handover_once = Once::new();
        let cancel = ctx.clone();
        h.handover = Some(Arc::new(move |load_hub: LoadHubFn| {
            handover_once.call_once(|| {
                *handover.lock().expect("handover lock poisoned") = Some(load_hub);
                // Let the response reach the page before the listener closes.
                std::thread::sleep(*HANDOVER_DELAY.lock().expect("handover delay lock poisoned"));
                cancel.cancel();
            });
        }));
    }
    let handler: HandlerFn =
        Arc::new(move |w: &mut ResponseWriter, r: &Request| h.serve_http(w, r));
    // serve owns the listener, so it is closed by the time it returns.
    let served = serve(&ctx, listener, handler);
    let load_hub = handover.lock().expect("handover lock poisoned").take();
    if let Some(load_hub) = load_hub {
        writeln!(c.out, "handing over to the pulse.hub service")?;
        if let Err(load_err) = load_hub() {
            return Err(pulse_cli::errorf(
                1,
                "The hub service did not load",
                &format!("{load_err:#}"),
                "pulse service install",
            )
            .into());
        }
        writeln!(
            c.out,
            "the pulse.hub service now serves {address}; this hub exits"
        )?;
        return Ok(());
    }
    if changed.load(Ordering::SeqCst) {
        writeln!(
            c.err,
            "pulse changed on disk; exiting 3 so the service restarts the hub"
        )?;
        return Err(pulse_cli::errorf(3, "", "", "").into());
    }
    served
}
