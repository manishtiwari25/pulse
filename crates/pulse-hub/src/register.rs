#![allow(unused_variables, dead_code, unused_imports)]
use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, UiFs};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
pub(crate) type JsonMap = BTreeMap<String, serde_json::Value>;
pub(crate) static RESTART_POLL: std::sync::LazyLock<std::sync::Mutex<std::time::Duration>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::time::Duration::from_secs(30)));
pub(crate) static HANDOVER_DELAY: std::sync::LazyLock<std::sync::Mutex<std::time::Duration>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::time::Duration::from_millis(300)));
pub(crate) struct CommandSpec {
    pub(crate) name: &'static str,
    pub(crate) group: &'static str,
    pub(crate) summary: &'static str,
    pub(crate) usage: &'static str,
    pub(crate) aliases: &'static [&'static str],
    pub(crate) flags: &'static [(&'static str, &'static str, &'static str)],
}
pub(crate) const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "hub",
        group: "hub",
        summary: "Serve the local owner console",
        usage: "pulse hub [--project SLUG] [--port N] [--no-browser] [--host 127.0.0.1] [--restart-on-change]",
        aliases: &[],
        flags: &[
            ("port", "8765", "TCP port (0 chooses a free port)"),
            ("host", "127.0.0.1", "listen address (127.0.0.1 only)"),
            ("no-browser", "false", "do not open the browser"),
            (
                "restart-on-change",
                "false",
                "exit 3 when the pulse binary changes on disk (the user service restarts it)",
            ),
            ("project", "", "project slug for serve"),
        ],
    },
    CommandSpec {
        name: "serve",
        group: "hub",
        summary: "Serve the local owner console",
        usage: "pulse serve [--project SLUG] [--port N] [--no-browser] [--host 127.0.0.1] [--restart-on-change]",
        aliases: &[],
        flags: &[
            ("port", "8765", "TCP port (0 chooses a free port)"),
            ("host", "127.0.0.1", "listen address (127.0.0.1 only)"),
            ("no-browser", "false", "do not open the browser"),
            (
                "restart-on-change",
                "false",
                "exit 3 when the pulse binary changes on disk (the user service restarts it)",
            ),
            ("project", "", "project slug for serve"),
        ],
    },
];
/// Go: func watchBinary(ctx context.Context, stamp string, onChange func())
pub(crate) fn watch_binary(
    ctx: &pulse_bus::CancellationToken,
    stamp: String,
    on_change: serde_json::Value,
) -> serde_json::Value {
    todo!("hub::register::watch_binary")
}
static REGISTER_ONCE: std::sync::Once = std::sync::Once::new();
/// Go: func init()
pub fn register() {
    REGISTER_ONCE.call_once(|| todo!("hub::register::register"))
}
