#![allow(dead_code)]

//! Port of internal/runner/service.go.

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::{LazyLock, RwLock};

pub(crate) const SERVICE_LABEL: &str = "pulse.daemon";
pub(crate) const HUB_PORT: i32 = 8765;
pub(crate) const LAUNCHER_NAME: &str = "pulse-daemon.sh";
pub(crate) static SERVICE_SLEEP: LazyLock<RwLock<fn(std::time::Duration)>> =
    LazyLock::new(|| RwLock::new(std::thread::sleep));
pub(crate) fn set_service_sleep(f: fn(std::time::Duration)) -> fn(std::time::Duration) {
    std::mem::replace(&mut *SERVICE_SLEEP.write().expect("service sleep lock"), f)
}
pub(crate) type RunServiceFn = dyn Fn(Vec<String>) -> (i32, String) + Send + Sync;
pub(crate) static RUN_SERVICE: LazyLock<RwLock<Box<RunServiceFn>>> = LazyLock::new(|| {
    RwLock::new(Box::new(|argv| {
        if argv.is_empty() {
            return (127, String::new());
        }
        match Command::new(&argv[0]).args(&argv[1..]).output() {
            Ok(out) => {
                let mut text = String::from_utf8_lossy(&out.stdout).to_string();
                text.push_str(&String::from_utf8_lossy(&out.stderr));
                (out.status.code().unwrap_or(-1), text)
            }
            Err(err) => (127, err.to_string()),
        }
    }))
});
pub(crate) fn set_run_service(f: Box<RunServiceFn>) -> Box<RunServiceFn> {
    std::mem::replace(&mut *RUN_SERVICE.write().expect("run service lock"), f)
}
pub(crate) static PROTECTED_FOLDERS: &[&str] = &["Desktop", "Documents", "Downloads"];

#[derive(Debug, Clone, Default)]
pub(crate) struct ServiceUnit {
    pub(crate) name: String,
    pub(crate) label: String,
    pub(crate) unit: String,
    pub(crate) launcher: String,
    pub(crate) log: String,
    pub(crate) desc: String,
    pub(crate) args: Vec<String>,
    pub(crate) guard: bool,
    pub(crate) text: String,
}

#[derive(Debug, Clone, Default)]
pub struct ServiceInstallOptions {
    pub interval: i32,
    pub hub_port: i32,
    pub no_hub: bool,
    pub dry_run: bool,
    /// DeferHub writes the hub's unit but leaves its load to the caller (load_hub in the result): a foreground
    /// hub on the same port must stop listening first, or the service's hub cannot bind.
    pub defer_hub: bool,
}

pub type LoadUnitFn = Arc<dyn Fn() -> anyhow::Result<()> + Send + Sync>;

/// Go: the `func() error` prepareUnit returns. Go's closure captures the install's writer; here the
/// step is handed the writer when it runs, so the load's lines reach the same `out` as the rest.
pub(crate) type LoadStep = Box<dyn Fn(&mut dyn std::io::Write) -> anyhow::Result<()> + Send + Sync>;

/// Go: io.MultiWriter(out, &buf) in InstallService.
struct Tee<'a> {
    out: &'a mut dyn std::io::Write,
    buf: &'a mut Vec<u8>,
}

impl std::io::Write for Tee<'_> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(b);
        self.out.write_all(b)?;
        Ok(b.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

#[derive(Clone, Default, Serialize)]
pub struct ServiceInstall {
    #[serde(rename = "kind")]
    pub kind: String,
    #[serde(rename = "daemon_path")]
    pub daemon_path: String,
    #[serde(rename = "hub_path")]
    pub hub_path: String,
    #[serde(rename = "output")]
    pub output: String,
    #[serde(skip)]
    pub load_hub: Option<LoadUnitFn>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UnitState {
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "path")]
    pub path: String,
    #[serde(rename = "present")]
    pub present: bool,
    #[serde(rename = "loaded")]
    pub loaded: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceState {
    #[serde(rename = "kind")]
    pub kind: String,
    #[serde(rename = "supported")]
    pub supported: bool,
    #[serde(rename = "installed")]
    pub installed: bool,
    #[serde(rename = "units")]
    pub units: Vec<UnitState>,
    #[serde(rename = "binary")]
    pub binary: String,
    #[serde(rename = "logs")]
    pub logs: String,
}

/// Go: func daemonUnit(interval int) serviceUnit
pub(crate) fn daemon_unit(interval: i32) -> ServiceUnit {
    ServiceUnit {
        name: "daemon".into(),
        label: SERVICE_LABEL.into(),
        unit: "pulse-daemon".into(),
        launcher: LAUNCHER_NAME.into(),
        log: "daemon.log".into(),
        desc: "PULSE daemon - the agents' machine".into(),
        args: vec!["daemon".into(), "--interval".into(), interval.to_string()],
        guard: true,
        text: String::new(),
    }
}

/// Go: func hubUnit(port int) serviceUnit
pub(crate) fn hub_unit(port: i32) -> ServiceUnit {
    let port = if port <= 0 { HUB_PORT } else { port };
    ServiceUnit {
        name: "hub".into(),
        label: "pulse.hub".into(),
        unit: "pulse-hub".into(),
        launcher: "pulse-hub.sh".into(),
        log: "hub.log".into(),
        desc: "PULSE hub - the owner's console".into(),
        args: vec![
            "hub".into(),
            "--port".into(),
            port.to_string(),
            "--no-browser".into(),
            "--restart-on-change".into(),
        ],
        guard: false,
        text: String::new(),
    }
}

/// Go: func serviceUnits(interval, port int, noHub bool) []serviceUnit
pub(crate) fn service_units(interval: i32, port: i32, no_hub: bool) -> Vec<ServiceUnit> {
    let mut units = vec![daemon_unit(interval)];
    if !no_hub {
        units.push(hub_unit(port));
    }
    units
}

/// Go: func servicePaths(system, home string) (string, string)
pub(crate) fn service_paths(system: String, home: String) -> (String, String) {
    unit_paths(daemon_unit(0), system, home)
}

/// Go: func unitPaths(u serviceUnit, system, home string) (string, string)
pub(crate) fn unit_paths(u: ServiceUnit, system: String, home: String) -> (String, String) {
    let system = if system.is_empty() {
        pulse_gitenv::GOOS.to_string()
    } else {
        system
    };
    let home = if home.is_empty() {
        crate::util::home_dir()
    } else {
        home
    };
    if system == "macos" || system == "darwin" {
        (
            "launchd".into(),
            Path::new(&home)
                .join("Library")
                .join("LaunchAgents")
                .join(format!("{}.plist", u.label))
                .display()
                .to_string(),
        )
    } else {
        (
            "systemd".into(),
            Path::new(&home)
                .join(".config/systemd/user")
                .join(format!("{}.service", u.unit))
                .display()
                .to_string(),
        )
    }
}

/// Go: func xmlEsc(s string) string
pub(crate) fn xml_esc(s: String) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Go: func serviceFileText(kind string, interval int, binary, logPath, pathEnv, pulseHome string) string
pub(crate) fn service_file_text(
    kind: String,
    interval: i32,
    binary: String,
    log_path: String,
    path_env: String,
    pulse_home: String,
) -> String {
    unit_file_text(
        daemon_unit(interval),
        kind,
        binary,
        log_path,
        path_env,
        pulse_home,
    )
}

/// Go: func unitFileText(u serviceUnit, kind string, binary, logPath, pathEnv, pulseHome string) string
pub(crate) fn unit_file_text(
    u: ServiceUnit,
    kind: String,
    binary: String,
    log_path: String,
    path_env: String,
    pulse_home: String,
) -> String {
    let args = vec![
        "/bin/sh".to_string(),
        unit_launcher_path(u.clone(), pulse_home.clone()),
    ];
    if kind == "launchd" {
        let mut lines = vec![
            r#"<?xml version="1.0" encoding="UTF-8"?>"#.to_string(),
            r#"<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">"#.to_string(),
            r#"<plist version="1.0">"#.to_string(),
            "<dict>".to_string(),
            format!("  <key>Label</key><string>{}</string>", u.label),
            "  <key>ProgramArguments</key>".to_string(),
            "  <array>".to_string(),
        ];
        for a in args {
            lines.push(format!("    <string>{}</string>", xml_esc(a)));
        }
        lines.extend([
            "  </array>".to_string(),
            "  <key>RunAtLoad</key><true/>".to_string(),
            "  <key>KeepAlive</key><true/>".to_string(),
            "  <key>ThrottleInterval</key><integer>30</integer>".to_string(),
            format!(
                "  <key>WorkingDirectory</key><string>{}</string>",
                xml_esc(
                    Path::new(&binary)
                        .parent()
                        .unwrap_or(Path::new(""))
                        .display()
                        .to_string()
                )
            ),
            format!(
                "  <key>StandardOutPath</key><string>{}</string>",
                xml_esc(log_path.clone())
            ),
            format!(
                "  <key>StandardErrorPath</key><string>{}</string>",
                xml_esc(log_path)
            ),
            "  <key>EnvironmentVariables</key>".to_string(),
            "  <dict>".to_string(),
            format!("    <key>PATH</key><string>{}</string>", xml_esc(path_env)),
            format!(
                "    <key>PULSE_HOME</key><string>{}</string>",
                xml_esc(pulse_home)
            ),
            "    <key>LANG</key><string>en_US.UTF-8</string>".to_string(),
            "  </dict>".to_string(),
            "</dict>".to_string(),
            "</plist>".to_string(),
            String::new(),
        ]);
        return lines.join("\n");
    }
    [
        "[Unit]".to_string(),
        format!("Description={}", u.desc),
        "After=network-online.target".to_string(),
        String::new(),
        "[Service]".to_string(),
        format!("ExecStart={}", crate::util::shell_join(&args)),
        format!(
            "WorkingDirectory={}",
            Path::new(&binary)
                .parent()
                .unwrap_or(Path::new(""))
                .display()
        ),
        "Restart=always".to_string(),
        "RestartSec=30".to_string(),
        format!("Environment=PATH={path_env}"),
        format!("Environment=PULSE_HOME={pulse_home}"),
        format!("StandardOutput=append:{log_path}"),
        format!("StandardError=append:{log_path}"),
        String::new(),
        "[Install]".to_string(),
        "WantedBy=default.target".to_string(),
        String::new(),
    ]
    .join("\n")
}

/// Go: func launcherPath(pulseHome string) string
pub(crate) fn launcher_path(pulse_home: String) -> String {
    unit_launcher_path(daemon_unit(0), pulse_home)
}

/// Go: func unitLauncherPath(u serviceUnit, pulseHome string) string
pub(crate) fn unit_launcher_path(u: ServiceUnit, pulse_home: String) -> String {
    Path::new(&pulse_home)
        .join("bin")
        .join(u.launcher)
        .display()
        .to_string()
}

/// Go: func launcherText(binary string, interval int) string
pub(crate) fn launcher_text(binary: String, interval: i32) -> String {
    unit_launcher_text(daemon_unit(interval), binary)
}

/// Go: func unitLauncherText(u serviceUnit, binary string) string
pub(crate) fn unit_launcher_text(u: ServiceUnit, binary: String) -> String {
    let head = vec![
        "#!/bin/sh".to_string(),
        "# Written by `pulse service install`. The service runs pulse through this script so that a".to_string(),
        "# restart is a loop here, not a launchd/systemd throttle, and the crash guard has a place to".to_string(),
        "# live. macOS holds pulse itself responsible for reading your folders; the stable signing".to_string(),
        "# requirement pulse gives every build is what keeps that answer across builds (ADR-035).".to_string(),
    ];
    let mut start_args = vec![binary.clone()];
    start_args.extend(u.args.clone());
    let start = format!("{} &", crate::util::shell_join(&start_args));
    if !u.guard {
        let mut lines = head;
        lines.extend([
            start,
            "pid=$!".into(),
            r#"trap 'kill -TERM "$pid" 2>/dev/null' TERM INT HUP"#.into(),
            r#"wait "$pid""#.into(),
            "code=$?".into(),
            r#"# A SIGKILL'd child becomes a zombie; kill -0 still succeeds, so wait again would loop forever."#.into(),
            r#"while [ "$code" -gt 128 ] && [ "$code" -ne 137 ] && kill -0 "$pid" 2>/dev/null; do"#.into(),
            r#"  wait "$pid""#.into(),
            "  code=$?".into(),
            "done".into(),
            r#"exit "$code""#.into(),
            String::new(),
        ]);
        return lines.join("\n");
    }
    let mut lines = head;
    lines.extend([
        "# Three exits other than 3 within a minute of starting put the previous build back (ADR-035).".into(),
        format!("bin={}", crate::util::shell_join(&[binary])),
        "early=0".into(),
        "stopping=".into(),
        "while :; do".into(),
        "  started=$(date +%s)".into(),
        format!("  {start}"),
        "  pid=$!".into(),
        r#"  trap 'stopping=1; kill -TERM "$pid" 2>/dev/null' TERM INT HUP"#.into(),
        r#"  wait "$pid""#.into(),
        "  code=$?".into(),
        r#"  while [ "$code" -gt 128 ] && [ "$code" -ne 137 ] && kill -0 "$pid" 2>/dev/null; do"#.into(),
        r#"    wait "$pid""#.into(),
        "    code=$?".into(),
        "  done".into(),
        r#"  if [ -n "$stopping" ] || [ "$code" -eq 3 ] || [ $(( $(date +%s) - started )) -ge 60 ]; then"#.into(),
        r#"    exit "$code""#.into(),
        "  fi".into(),
        "  early=$((early + 1))".into(),
        r#"  if [ "$early" -lt 3 ]; then"#.into(),
        "    sleep 2".into(),
        "    continue".into(),
        "  fi".into(),
        "  early=0".into(),
        r#"  if [ ! -f "$bin.prev" ]; then"#.into(),
        r#"    exit "$code""#.into(),
        "  fi".into(),
        r#"  mv -f "$bin" "$bin.failed" && mv -f "$bin.prev" "$bin" || exit "$code""#.into(),
        r#"  echo "restored the previous pulse after 3 early exits (last exit $code)""#.into(),
        r#"  date -u +%Y-%m-%dT%H:%M:%SZ > "$bin.restored""#.into(),
        "  sleep 2".into(),
        "done".into(),
        String::new(),
    ]);
    lines.join("\n")
}

/// Go: func launcherFromPlist(text string) string
pub(crate) fn launcher_from_plist(text: String) -> String {
    for mut line in text.lines().map(str::trim) {
        let mut path = String::new();
        if line.starts_with("<string>") && line.ends_with(".sh</string>") {
            path = line
                .trim_start_matches("<string>")
                .trim_end_matches("</string>")
                .to_string();
        } else if line.starts_with("ExecStart=/bin/sh ") && line.ends_with(".sh") {
            path = line.trim_start_matches("ExecStart=/bin/sh ").to_string();
        }
        line = path.trim_matches(&['\'', '"'][..]);
        if !line.is_empty()
            && Path::new(line)
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|b| b.starts_with("pulse-"))
        {
            return line.to_string();
        }
    }
    String::new()
}

/// Go: func serviceCommands(kind, path, action string) [][]string
pub(crate) fn service_commands(kind: String, path: String, action: String) -> Vec<Vec<String>> {
    unit_commands(daemon_unit(0), kind, path, action)
}

/// Go: func unitCommands(u serviceUnit, kind, path, action string) [][]string
pub(crate) fn unit_commands(
    u: ServiceUnit,
    kind: String,
    path: String,
    action: String,
) -> Vec<Vec<String>> {
    if kind == "launchd" {
        let domain = format!("gui/{}", unsafe { libc::getuid() });
        return match action.as_str() {
            "install" => vec![
                vec![
                    "launchctl".into(),
                    "bootout".into(),
                    format!("{domain}/{}", u.label),
                ],
                vec!["launchctl".into(), "bootstrap".into(), domain, path],
            ],
            "uninstall" => vec![vec![
                "launchctl".into(),
                "bootout".into(),
                format!("{domain}/{}", u.label),
            ]],
            _ => vec![vec![
                "launchctl".into(),
                "print".into(),
                format!("{domain}/{}", u.label),
            ]],
        };
    }
    match action.as_str() {
        "install" => vec![
            vec!["systemctl".into(), "--user".into(), "daemon-reload".into()],
            vec![
                "systemctl".into(),
                "--user".into(),
                "enable".into(),
                "--now".into(),
                u.unit,
            ],
        ],
        "uninstall" => vec![vec![
            "systemctl".into(),
            "--user".into(),
            "disable".into(),
            "--now".into(),
            u.unit,
        ]],
        _ => vec![vec![
            "systemctl".into(),
            "--user".into(),
            "status".into(),
            u.unit,
            "--no-pager".into(),
        ]],
    }
}

/// Go: func pulseBinary() string
pub(crate) fn pulse_binary() -> String {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("pulse"));
    std::fs::canonicalize(&exe)
        .unwrap_or(exe)
        .display()
        .to_string()
}

/// Go: func installService(out io.Writer, interval int, pulseHome, system, userHome string, dryRun bool) (string, error)
pub(crate) fn install_service_inner(
    out: &mut dyn std::io::Write,
    interval: i32,
    pulse_home: String,
    system: String,
    user_home: String,
    dry_run: bool,
) -> anyhow::Result<String> {
    install_unit(
        out,
        daemon_unit(interval),
        pulse_home,
        system,
        user_home,
        dry_run,
    )
}

/// Go: func installUnit(out io.Writer, u serviceUnit, pulseHome, system, userHome string, dryRun bool) (string, error)
pub(crate) fn install_unit(
    out: &mut dyn std::io::Write,
    u: ServiceUnit,
    pulse_home: String,
    system: String,
    user_home: String,
    dry_run: bool,
) -> anyhow::Result<String> {
    let (path, load) = prepare_unit(out, u, pulse_home, system, user_home, dry_run)?;
    if let Some(load) = load {
        load(out)?;
    }
    Ok(path)
}

/// Go: func prepareUnit(out io.Writer, u serviceUnit, pulseHome, system, userHome string, dryRun bool) (string, func() error, error)
pub(crate) fn prepare_unit(
    out: &mut dyn std::io::Write,
    u: ServiceUnit,
    pulse_home: String,
    system: String,
    user_home: String,
    dry_run: bool,
) -> anyhow::Result<(String, Option<LoadStep>)> {
    let (kind, path) = unit_paths(u.clone(), system, user_home);
    let log_dir = Path::new(&pulse_home).join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    let path_env = std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into());
    let binary = pulse_binary();
    let text = unit_file_text(
        u.clone(),
        kind.clone(),
        binary.clone(),
        log_dir.join(&u.log).display().to_string(),
        path_env,
        pulse_home.clone(),
    );
    let launcher = unit_launcher_text(u.clone(), binary);
    let launcher_file = unit_launcher_path(u.clone(), pulse_home);
    writeln!(out, "{} launcher: {}", u.name, launcher_file)?;
    writeln!(out, "{} {} service file: {}", u.name, kind, path)?;
    if dry_run {
        writeln!(out, "{launcher}")?;
        writeln!(out, "{text}")?;
        return Ok((path, None));
    }
    if same_file(path.clone(), text.clone()) && same_file(launcher_file.clone(), launcher.clone()) {
        let status = unit_commands(u.clone(), kind.clone(), path.clone(), "status".into());
        if (RUN_SERVICE.read().expect("run service lock"))(status[0].clone()).0 == 0 {
            writeln!(out, "{}: unchanged and loaded; left running", u.name)?;
            return Ok((path, None));
        }
    }
    if let Some(parent) = Path::new(&launcher_file).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&launcher_file, launcher)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&launcher_file, std::fs::Permissions::from_mode(0o755))?;
    }
    if let Some(parent) = Path::new(&path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, text)?;
    let u2 = u.clone();
    let kind2 = kind.clone();
    let path2 = path.clone();
    Ok((
        path,
        Some(Box::new(move |out: &mut dyn std::io::Write| {
            load_unit(out, u2.clone(), kind2.clone(), path2.clone())
        })),
    ))
}

/// Go: func loadUnit(out io.Writer, u serviceUnit, kind, path string) error
pub(crate) fn load_unit(
    out: &mut dyn std::io::Write,
    u: ServiceUnit,
    kind: String,
    path: String,
) -> anyhow::Result<()> {
    for (i, cmd) in unit_commands(u.clone(), kind.clone(), path.clone(), "install".into())
        .into_iter()
        .enumerate()
    {
        let (mut rc, mut text) = (RUN_SERVICE.read().expect("run service lock"))(cmd.clone());
        if kind == "launchd" && i == 1 && rc == 5 {
            (SERVICE_SLEEP.read().expect("service sleep lock"))(std::time::Duration::from_secs(3));
            (rc, text) = (RUN_SERVICE.read().expect("run service lock"))(cmd.clone());
        }
        let tolerated = kind == "launchd" && i == 0;
        let mut suffix = String::new();
        if rc != 0 {
            if tolerated {
                suffix = " (not loaded before)".into();
            } else {
                suffix = format!(" -> exit {}: {}", rc, text.trim());
            }
        }
        writeln!(out, "{}{}", cmd.join(" "), suffix)?;
        if rc != 0 && !tolerated {
            return Err(anyhow!(
                "could not load the {} service; the file is written, load it by hand: {}",
                u.name,
                cmd.join(" ")
            ));
        }
    }
    Ok(())
}

/// Go: func InstallService(out io.Writer, pulseHome string, o ServiceInstallOptions) (*ServiceInstall, error)
pub fn install_service(
    out: &mut dyn std::io::Write,
    pulse_home: &str,
    o: ServiceInstallOptions,
) -> anyhow::Result<ServiceInstall> {
    let mut o = o;
    let pulse_home = if pulse_home.is_empty() {
        crate::util::home_dir()
    } else {
        pulse_home.to_string()
    };
    if o.interval <= 0 {
        o.interval = crate::daemon::DEFAULT_DAEMON_INTERVAL_SECONDS;
    }
    if o.hub_port <= 0 {
        o.hub_port = HUB_PORT;
    }
    let mut buf = Vec::new();
    let mut w = Tee { out, buf: &mut buf };
    let (kind, daemon_path) = service_paths(String::new(), String::new());
    let mut result = ServiceInstall {
        kind,
        daemon_path,
        ..Default::default()
    };
    if !o.dry_run {
        // Sign before the units load, so the first daemon to run is the one whose folder answer sticks.
        crate::sign::ensure_signed(&mut w, &pulse_binary());
    }
    for u in service_units(o.interval, o.hub_port, o.no_hub) {
        let (path, load) = prepare_unit(
            &mut w,
            u.clone(),
            pulse_home.clone(),
            String::new(),
            String::new(),
            o.dry_run,
        )?;
        if u.name == "hub" {
            result.hub_path = path;
        }
        let Some(load) = load else {
            continue;
        };
        if u.name == "hub" && o.defer_hub {
            // Go's deferred load writes to the install's writer after Output was taken; its only caller
            // (the console's service route) passes no writer, so the lines land in a buffer nobody reads.
            result.load_hub = Some(Arc::new(move || {
                let mut sink = Vec::new();
                load(&mut sink)
            }));
            continue;
        }
        load(&mut w)?;
    }
    result.output = String::from_utf8_lossy(&buf).to_string();
    Ok(result)
}

/// Go: func ServiceStatus(pulseHome string) ServiceState
pub fn service_status(pulse_home: String) -> ServiceState {
    let pulse_home = if pulse_home.is_empty() {
        crate::util::home_dir()
    } else {
        pulse_home
    };
    let (kind, _) = service_paths(String::new(), String::new());
    let mut state = ServiceState {
        kind: kind.clone(),
        supported: matches!(pulse_gitenv::GOOS, "darwin" | "macos" | "linux"),
        binary: pulse_binary(),
        logs: Path::new(&pulse_home).join("logs").display().to_string(),
        ..Default::default()
    };
    for u in service_units(0, 0, false) {
        let (_, path) = unit_paths(u.clone(), String::new(), String::new());
        let mut unit = UnitState {
            name: u.name.clone(),
            path: path.clone(),
            present: crate::util::file_exists(&path),
            loaded: false,
        };
        if unit.present {
            let status = unit_commands(u.clone(), kind.clone(), path, "status".into());
            unit.loaded = (RUN_SERVICE.read().expect("run service lock"))(status[0].clone()).0 == 0;
        }
        if u.name == "daemon" {
            state.installed = unit.present && unit.loaded;
        }
        state.units.push(unit);
    }
    state
}

/// Go: func sameFile(path, text string) bool
pub(crate) fn same_file(path: String, text: String) -> bool {
    std::fs::read_to_string(path).is_ok_and(|data| data == text)
}

/// Go: func UninstallService(out io.Writer) string
pub fn uninstall_service(out: &mut dyn std::io::Write, system: &str, user_home: &str) {
    let _ = uninstall_service_inner(out, system.to_string(), user_home.to_string());
}

/// Go: func uninstallService(out io.Writer, system, userHome string) string
pub(crate) fn uninstall_service_inner(
    out: &mut dyn std::io::Write,
    system: String,
    user_home: String,
) -> String {
    let mut daemon_path = String::new();
    for u in service_units(0, 0, false) {
        let (kind, path) = unit_paths(u.clone(), system.clone(), user_home.clone());
        if u.name == "daemon" {
            daemon_path = path.clone();
        }
        if !crate::util::file_exists(&path) {
            continue;
        }
        for cmd in unit_commands(u, kind, path.clone(), "uninstall".into()) {
            let (rc, text) = (RUN_SERVICE.read().expect("run service lock"))(cmd.clone());
            let suffix = if rc != 0 {
                format!(" -> exit {}: {}", rc, text.trim())
            } else {
                String::new()
            };
            let _ = writeln!(out, "{}{}", cmd.join(" "), suffix);
        }
        if let Ok(data) = std::fs::read_to_string(&path) {
            let launcher = launcher_from_plist(data);
            if !launcher.is_empty() && crate::util::file_exists(&launcher) {
                let _ = std::fs::remove_file(&launcher);
                let _ = writeln!(out, "removed {launcher}");
            }
        }
        let _ = std::fs::remove_file(&path);
        let _ = writeln!(out, "removed {path}");
    }
    daemon_path
}

/// Go: func serviceStatus(out io.Writer, s *store.Store, system, userHome string)
pub(crate) fn service_status_inner(
    out: &mut dyn std::io::Write,
    s: Option<&pulse_store::Store>,
    system: &str,
    user_home: &str,
) {
    for u in service_units(0, 0, false) {
        let (kind, path) = unit_paths(u.clone(), system.to_string(), user_home.to_string());
        let state = if crate::util::file_exists(&path) {
            "present"
        } else {
            "absent"
        };
        let _ = writeln!(out, "{} service file {}: {}", u.name, path, state);
        if state == "absent" {
            continue;
        }
        for cmd in unit_commands(u, kind, path, "status".into()) {
            let (_, text) = (RUN_SERVICE.read().expect("run service lock"))(cmd);
            let mut lines = Vec::new();
            for l in text.lines() {
                if [
                    "state",
                    "pid",
                    "Active",
                    "Main PID",
                    "could not",
                    "not find",
                ]
                .iter()
                .any(|k| l.contains(k))
                {
                    lines.push(l.to_string());
                }
            }
            if lines.len() > 6 {
                lines.truncate(6);
            }
            if lines.is_empty() {
                let _ = writeln!(out, "{}", crate::util::trunc(text.trim(), 400));
            } else {
                let _ = writeln!(out, "{}", lines.join("\n"));
            }
        }
    }
    if let Some(s) = s {
        let _ = writeln!(
            out,
            "daemon     {}",
            crate::onboard::daemon_status(s, s.now())
        );
    }
}

/// Go: func protectedFoldersNote(s *store.Store, system, userHome string) string
pub(crate) fn protected_folders_note(
    s: &pulse_store::Store,
    system: String,
    user_home: String,
) -> String {
    let system = if system.is_empty() {
        pulse_gitenv::GOOS.to_string()
    } else {
        system
    };
    if system != "darwin" && system != "macos" {
        return String::new();
    }
    let mut user_home = if user_home.is_empty() {
        crate::util::home_dir()
    } else {
        user_home
    };
    if let Ok(rp) = std::fs::canonicalize(&user_home) {
        user_home = rp.display().to_string();
    }
    let mut hits = Vec::new();
    let conn = match s.conn() {
        Ok(conn) => conn,
        Err(_) => return String::new(),
    };
    for p in crate::board::project_rows(&conn, false) {
        let mut path = p.str("path");
        if let Ok(rp) = std::fs::canonicalize(&path) {
            path = rp.display().to_string();
        }
        for folder in PROTECTED_FOLDERS {
            let guarded = Path::new(&user_home).join(folder);
            if path == guarded.display().to_string()
                || path.starts_with(&(guarded.join("").display().to_string()))
                || path.starts_with(&format!("{}/", guarded.display()))
            {
                hits.push(format!("{} ({})", p.str("slug"), path));
                break;
            }
        }
    }
    if hits.is_empty() {
        return String::new();
    }
    format!(
        "macOS: {} live under a folder macOS guards ({}).\n  The first time the daemon touches them a dialog asks whether pulse may access that folder: answer Allow once, or the\n  loop waits on it (pulse status then says the daemon is silent). macOS ties the answer to the binary's signing\n  requirement, so pulse signs every build as `pulse` and the answer holds across rebuilds; a build that skipped the\n  signing step asks again. Checkouts outside those folders never ask.",
        hits.join(", "),
        PROTECTED_FOLDERS.join("/")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{LazyLock, Mutex};

    static SERVICE_TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    struct SigningSystemGuard;

    impl Drop for SigningSystemGuard {
        fn drop(&mut self) {
            *crate::sign::SIGNING_SYSTEM.write().unwrap() = String::new();
        }
    }

    #[test]
    fn install_service_and_service_status() {
        let _serial = crate::test_helpers::serial();
        let _guard = SERVICE_TEST_LOCK.lock().unwrap();
        let user_home = tempfile::tempdir().unwrap();
        let _env_1 = pulse_testutil::setenv("HOME", user_home.path());
        let pulse_home = tempfile::tempdir().unwrap();
        let pulse_home_s = pulse_home.path().display().to_string();
        *crate::sign::SIGNING_SYSTEM.write().unwrap() = "linux".into();
        let _signing = SigningSystemGuard;
        let old = set_run_service(Box::new(|_| (0, String::new())));
        let calls = std::sync::Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
        let loaded =
            std::sync::Arc::new(Mutex::new(std::collections::BTreeMap::<String, bool>::new()));
        let which = |argv: &[String]| -> String {
            if argv.join(" ").contains("hub") {
                return "hub".into();
            }
            "daemon".into()
        };
        let _ = set_run_service(Box::new({
            let calls = calls.clone();
            let loaded = loaded.clone();
            move |argv| {
                calls.lock().unwrap().push(argv.clone());
                if argv.len() < 2 {
                    return (0, String::new());
                }
                match argv[1].as_str() {
                    // launchctl print / systemctl status: exit 0 once the unit's load ran
                    "print" | "status" => {
                        if loaded
                            .lock()
                            .unwrap()
                            .get(&which(&argv))
                            .copied()
                            .unwrap_or(false)
                        {
                            return (0, String::new());
                        }
                        return (113, "not loaded".into());
                    }
                    "bootstrap" | "enable" => {
                        loaded.lock().unwrap().insert(which(&argv), true);
                    }
                    _ => {}
                }
                (0, String::new())
            }
        }));
        let (kind, daemon_path) = service_paths(String::new(), String::new());
        let (_, hub_path) = unit_paths(hub_unit(0), String::new(), String::new());

        let before = service_status(pulse_home_s.clone());
        if before.kind != kind
            || !before.supported
            || before.installed
            || before.units.len() != 2
            || before.units[0].name != "daemon"
            || before.units[0].path != daemon_path
            || before.units[0].present
            || before.units[1].name != "hub"
            || before.units[1].path != hub_path
            || before.logs != pulse_home.path().join("logs").display().to_string()
            || !calls.lock().unwrap().is_empty()
        {
            panic!(
                "status before install: {before:?} (calls {:?})",
                calls.lock().unwrap()
            );
        }

        let mut out = Vec::new();
        let dry = install_service(
            &mut out,
            &pulse_home_s,
            ServiceInstallOptions {
                dry_run: true,
                ..Default::default()
            },
        );
        let dry = match dry {
            Ok(dry)
                if dry.kind == kind
                    && dry.daemon_path == daemon_path
                    && dry.hub_path == hub_path
                    && dry.load_hub.is_none() =>
            {
                dry
            }
            Ok(dry) => panic!("dry run: {:?} {:?}", dry.output, dry.hub_path),
            Err(err) => panic!("dry run: {err}"),
        };
        if crate::util::file_exists(&daemon_path)
            || crate::util::file_exists(&hub_path)
            || !calls.lock().unwrap().is_empty()
            || dry.output != String::from_utf8_lossy(&out)
            || !dry.output.contains("--port 8765")
            || !dry.output.contains(&format!(
                "--interval {} &",
                crate::daemon::DEFAULT_DAEMON_INTERVAL_SECONDS
            ))
        {
            panic!(
                "dry run wrote or loaded something: {:?}\n{}",
                calls.lock().unwrap(),
                dry.output
            );
        }

        // A foreground hub installs with the hub's load deferred: the daemon loads, the hub unit is written, its load waits.
        out.clear();
        let deferred = install_service(
            &mut std::io::sink(),
            &pulse_home_s,
            ServiceInstallOptions {
                interval: 45,
                hub_port: 8765,
                defer_hub: true,
                ..Default::default()
            },
        );
        let deferred = match deferred {
            Ok(deferred)
                if deferred.load_hub.is_some()
                    && crate::util::file_exists(&daemon_path)
                    && crate::util::file_exists(&hub_path) =>
            {
                deferred
            }
            Ok(deferred) => panic!("deferred install: {:?}", deferred.output),
            Err(err) => panic!("deferred install: {err}"),
        };
        if !loaded
            .lock()
            .unwrap()
            .get("daemon")
            .copied()
            .unwrap_or(false)
            || loaded.lock().unwrap().get("hub").copied().unwrap_or(false)
        {
            panic!(
                "deferred install loaded the wrong units: {:?}",
                loaded.lock().unwrap()
            );
        }
        let launcher = std::fs::read_to_string(launcher_path(pulse_home_s.clone()));
        if launcher.is_err() || !launcher.as_ref().unwrap().contains("--interval 45 &") {
            panic!("daemon launcher: {launcher:?}");
        }
        if !out.is_empty() || !deferred.output.contains("hub launcher: ") {
            panic!("deferred output:\n{}", deferred.output);
        }
        let mid = service_status(pulse_home_s.clone());
        if !mid.installed || !mid.units[0].loaded || !mid.units[1].present || mid.units[1].loaded {
            panic!("status between: {mid:?}");
        }
        let load_hub = deferred.load_hub.as_ref().unwrap();
        if let Err(err) = load_hub() {
            panic!("LoadHub: {err} {:?}", loaded.lock().unwrap());
        }
        if !loaded.lock().unwrap().get("hub").copied().unwrap_or(false) {
            panic!("LoadHub: {:?}", loaded.lock().unwrap());
        }
        let after = service_status(pulse_home_s.clone());
        if !after.installed || !after.units[1].loaded {
            panic!("status after: {after:?}");
        }

        // Installing again with the same files is a no-op that leaves both running.
        calls.lock().unwrap().clear();
        let again = install_service(
            &mut std::io::sink(),
            &pulse_home_s,
            ServiceInstallOptions {
                interval: 45,
                ..Default::default()
            },
        );
        let again = match again {
            Ok(again)
                if again.load_hub.is_none() && again.output.contains("unchanged and loaded") =>
            {
                again
            }
            Ok(again) => panic!("second install: {:?}", again.output),
            Err(err) => panic!("second install: {err}"),
        };
        let _ = again;
        for argv in calls.lock().unwrap().iter() {
            if argv.len() > 1
                && (argv[1] == "bootstrap"
                    || argv[1] == "bootout"
                    || argv[1] == "enable"
                    || argv[1] == "restart")
            {
                panic!(
                    "an unchanged install reloaded a unit: {:?}",
                    calls.lock().unwrap()
                );
            }
        }
        let data = std::fs::read_to_string(&hub_path);
        if data.is_err() || !data.as_ref().unwrap().contains("pulse-hub.sh") {
            panic!("hub unit: {data:?}");
        }
        let _ = set_run_service(old);
    }
}
