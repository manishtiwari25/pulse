//! Keep-awake (`daemon.keep_awake`): while the daemon runs, it holds a power
//! assertion so the machine does not sleep under its runs - the way
//! `caffeinate` does for a terminal - and it notices when the machine slept
//! anyway, so the console can explain the lost hours.
//!
//! The hold is a child process that dies with the daemon: on macOS
//! `caffeinate -s -w <daemon pid>` (no system sleep while on AC power;
//! `always` adds `-i`, no idle sleep on battery either), on Linux
//! `systemd-inhibit --what=sleep:idle --mode=block tail --pid=<daemon pid>
//! -f /dev/null`. A daemon that exits (stopped, restarting) takes the hold
//! with it, so a paused machine sleeps as usual. Closing a laptop's lid still
//! sleeps a Mac unless an external display is attached: no assertion stops
//! clamshell sleep, and the console says so.
//!
//! `on` holds only while the machine is plugged in (`caffeinate -s` ignores
//! battery power), so the daemon also reads the power source each tick -
//! `pmset -g batt` on macOS, `/sys/class/power_supply` on Linux - and the
//! heartbeat says whether the hold is in effect right now; a hold waiting
//! for the charger is said once in the log, not every tick.
//!
//! Sleep detection compares the wall clock's advance with the monotonic
//! clock's between two loop ticks: the monotonic clock stops while the
//! machine sleeps, so a wall clock that got ahead by more than
//! [`SLEEP_GAP_SECONDS`] means the machine slept for about that long. The
//! gap is logged, written as a `daemon.slept` event and kept in the
//! heartbeat for the console.

use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime};

/// The setting: `off` (default), `on` (no sleep while plugged in) or
/// `always` (no idle sleep on battery either).
pub const SETTING: &str = "daemon.keep_awake";
/// The event written when a tick finds the machine slept.
pub const SLEPT_ACTION: &str = "daemon.slept";
/// Wall-clock seconds past the monotonic clock between two ticks before the
/// gap counts as sleep; clock adjustments stay under it.
pub const SLEEP_GAP_SECONDS: u64 = 60;
/// Seconds a failed holder start waits before the next try.
const RETRY_SECONDS: u64 = 600;
/// A holder that exits this soon after it started did not work (an unknown
/// flag, no permission); the next try waits RETRY_SECONDS instead of
/// spawning one per tick.
const EARLY_EXIT_SECONDS: u64 = 5;
/// Seconds between two reads of the power source; a bus-woken loop ticks
/// more often than that.
const POWER_PROBE_SECONDS: u64 = 20;

/// What the setting asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Off,
    /// No system sleep while the machine is on AC power.
    PluggedIn,
    /// No idle sleep on battery either.
    Always,
}

impl Mode {
    pub fn parse(value: &str) -> Mode {
        match value.trim() {
            "on" | "plugged-in" | "plugged_in" | "1" => Mode::PluggedIn,
            "always" | "battery" => Mode::Always,
            _ => Mode::Off,
        }
    }

    /// The setting value the console shows.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::PluggedIn => "on",
            Mode::Always => "always",
        }
    }
}

/// The holder command for this platform: the program and its arguments, or
/// None where nothing can hold the machine awake.
pub fn holder_command(mode: Mode, pid: u32) -> Option<(String, Vec<String>)> {
    if mode == Mode::Off {
        return None;
    }
    holder_for(std::env::consts::OS, mode, pid)
}

fn holder_for(os: &str, mode: Mode, pid: u32) -> Option<(String, Vec<String>)> {
    match os {
        "macos" => {
            // -s: prevent system sleep (AC power only); -i: prevent idle
            // sleep, which keeps a battery awake too; -w: hold until the
            // daemon exits.
            let mut args = vec!["-s".to_string()];
            if mode == Mode::Always {
                args.push("-i".to_string());
            }
            args.push("-w".to_string());
            args.push(pid.to_string());
            Some(("caffeinate".to_string(), args))
        }
        "linux" => Some((
            "systemd-inhibit".to_string(),
            vec![
                "--what=sleep:idle".to_string(),
                "--who=PULSE".to_string(),
                "--why=PULSE runs agents on this machine".to_string(),
                "--mode=block".to_string(),
                "tail".to_string(),
                format!("--pid={pid}"),
                "-f".to_string(),
                "/dev/null".to_string(),
            ],
        )),
        _ => None,
    }
}

/// The caveat the console and the log carry with the hold.
pub fn lid_note() -> &'static str {
    if std::env::consts::OS == "macos" {
        "closing the lid still sleeps a Mac unless an external display is attached"
    } else {
        ""
    }
}

/// Where the machine draws its power from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Power {
    Ac,
    Battery,
    Unknown,
}

impl Power {
    /// The word the heartbeat carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Power::Ac => "ac",
            Power::Battery => "battery",
            Power::Unknown => "unknown",
        }
    }
}

/// The power source now: macOS asks `pmset`, Linux reads the kernel's
/// power supplies; anywhere else it is unknown.
pub fn power_source() -> Power {
    match std::env::consts::OS {
        "macos" => std::process::Command::new("pmset")
            .args(["-g", "batt"])
            .stdin(std::process::Stdio::null())
            .output()
            .ok()
            .map(|out| parse_pmset(&String::from_utf8_lossy(&out.stdout)))
            .unwrap_or(Power::Unknown),
        "linux" => power_from_sysfs(std::path::Path::new("/sys/class/power_supply")),
        _ => Power::Unknown,
    }
}

/// `pmset -g batt` opens with `Now drawing from 'AC Power'` or `'Battery
/// Power'`.
fn parse_pmset(out: &str) -> Power {
    if out.contains("'AC Power'") {
        Power::Ac
    } else if out.contains("'Battery Power'") {
        Power::Battery
    } else {
        Power::Unknown
    }
}

/// Under `/sys/class/power_supply` an adapter (`type` Mains or USB) says
/// `online` 1 when it feeds the machine; a machine with an adapter entry
/// that is offline runs on battery; no adapter entry at all says nothing.
fn power_from_sysfs(root: &std::path::Path) -> Power {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Power::Unknown;
    };
    let mut adapter_online = None;
    for entry in entries.flatten() {
        let dir = entry.path();
        let kind = std::fs::read_to_string(dir.join("type")).unwrap_or_default();
        if !matches!(kind.trim(), "Mains" | "USB") {
            continue;
        }
        let online = std::fs::read_to_string(dir.join("online"))
            .map(|v| v.trim() == "1")
            .unwrap_or(false);
        if online {
            adapter_online = Some(true);
            break;
        }
        adapter_online.get_or_insert(false);
    }
    match adapter_online {
        Some(true) => Power::Ac,
        Some(false) => Power::Battery,
        None => Power::Unknown,
    }
}

/// Whether this platform's holder keeps a machine on battery awake: macOS
/// `caffeinate -s` does not (`-i`, the `always` mode, does); a systemd
/// inhibitor does not care where the power comes from.
fn holds_on_battery(os: &str, mode: Mode) -> bool {
    match os {
        "macos" => mode == Mode::Always,
        _ => true,
    }
}

/// Wall-clock seconds past the monotonic clock between two ticks when the
/// gap says the machine slept.
pub fn sleep_gap(wall: Duration, mono: Duration) -> Option<u64> {
    let extra = wall.saturating_sub(mono).as_secs();
    (extra >= SLEEP_GAP_SECONDS).then_some(extra)
}

/// The heartbeat's view of the hold; the daemon writes it each tick and the
/// watchdog's beat reads it too.
static BEAT: RwLock<serde_json::Value> = RwLock::new(serde_json::Value::Null);

/// The `awake` and `slept` fields the heartbeat carries, Null before the
/// first tick.
pub fn beat() -> serde_json::Value {
    BEAT.read().map(|b| b.clone()).unwrap_or_default()
}

type Holder = Box<dyn Fn(Mode, u32) -> Option<(String, Vec<String>)> + Send>;
type Probe = Box<dyn Fn() -> Power + Send>;

/// The hold and the sleep watch, owned by the daemon loop.
pub struct Awake {
    holder: Holder,
    probe: Probe,
    /// The platform whose rules apply (tests pin one).
    os: &'static str,
    power: Power,
    power_read: Option<Instant>,
    /// The hold is held but does nothing until the charger is back
    /// (`on` on a Mac running on battery).
    charger_wait: bool,
    child: Option<std::process::Child>,
    spawned: Option<Instant>,
    since: String,
    mode: Mode,
    how: String,
    /// Why the hold is not held while the setting asks for it.
    note: String,
    retry_after: Option<Instant>,
    last_wall: SystemTime,
    last_mono: Instant,
    /// The last sleep this daemon noticed: when the loop woke and for how long.
    slept: Option<(String, u64)>,
}

impl Default for Awake {
    fn default() -> Self {
        Self::new()
    }
}

impl Awake {
    pub fn new() -> Self {
        Self::with_holder(Box::new(holder_command))
    }

    /// A hold whose command the caller chooses (tests spawn `sleep`).
    pub fn with_holder(holder: Holder) -> Self {
        Self::with_holder_and_probe(holder, Box::new(power_source))
    }

    /// A hold whose command and power reading the caller chooses.
    pub fn with_holder_and_probe(holder: Holder, probe: Probe) -> Self {
        Awake {
            holder,
            probe,
            os: std::env::consts::OS,
            power: Power::Unknown,
            power_read: None,
            charger_wait: false,
            child: None,
            spawned: None,
            since: String::new(),
            mode: Mode::Off,
            how: String::new(),
            note: String::new(),
            retry_after: None,
            last_wall: SystemTime::now(),
            last_mono: Instant::now(),
            slept: None,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn holding(&self) -> bool {
        self.child.is_some()
    }

    /// The hold is held and keeps the machine awake right now.
    pub fn effective(&self) -> bool {
        self.holding() && !self.charger_wait
    }

    pub fn power(&self) -> Power {
        self.power
    }

    /// One loop tick: take or release the hold to match the setting. Returns
    /// the lines the daemon says.
    pub fn tick(&mut self, s: &pulse_store::Store) -> Vec<String> {
        let mode = Mode::parse(&s.setting(SETTING, "off"));
        let now = s.now();
        self.tick_mode(mode, &now)
    }

    fn tick_mode(&mut self, mode: Mode, now: &str) -> Vec<String> {
        if mode != Mode::Off
            && self
                .power_read
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(POWER_PROBE_SECONDS))
        {
            self.power = (self.probe)();
            self.power_read = Some(Instant::now());
        }
        let was_holding = self.holding();
        let was_waiting = self.charger_wait;
        let mut lines = self.tick_hold(mode, now);
        self.charger_wait =
            self.holding() && self.power == Power::Battery && !holds_on_battery(self.os, self.mode);
        if was_holding && self.holding() && was_waiting != self.charger_wait {
            lines.push(if self.charger_wait {
                "keep-awake: on battery, so the hold waits for the charger (daemon.keep_awake=always holds on battery too)".to_string()
            } else {
                "keep-awake: plugged in again; holding this machine awake".to_string()
            });
        }
        self.publish();
        lines
    }

    fn tick_hold(&mut self, mode: Mode, now: &str) -> Vec<String> {
        let mut lines = Vec::new();
        if mode != self.mode {
            self.retry_after = None;
            self.note.clear();
        }
        let was = self.mode;
        self.mode = mode;
        if mode == Mode::Off {
            if self.release() {
                lines.push("keep-awake off: the machine may sleep again".to_string());
            }
            self.publish();
            return lines;
        }
        if was != mode && self.child.is_some() {
            self.release();
        }
        let waited = self.child.as_mut().map(|child| child.try_wait());
        match waited {
            None => {}
            Some(Ok(None)) => {
                self.publish();
                return lines;
            }
            Some(Ok(Some(status))) => {
                let early = self
                    .spawned
                    .is_some_and(|at| at.elapsed() < Duration::from_secs(EARLY_EXIT_SECONDS));
                self.child = None;
                self.spawned = None;
                self.since.clear();
                if early {
                    self.note = format!(
                        "{} exited at once ({status}); trying again in {}",
                        self.how,
                        pulse_cli::format_duration(Duration::from_secs(RETRY_SECONDS))
                    );
                    self.retry_after = Some(Instant::now() + Duration::from_secs(RETRY_SECONDS));
                    lines.push(format!("keep-awake: {}", self.note));
                    self.publish();
                    return lines;
                }
                lines.push(format!(
                    "keep-awake: {} exited ({status}); taking the hold again",
                    self.how
                ));
            }
            Some(Err(err)) => {
                self.child = None;
                self.spawned = None;
                self.since.clear();
                lines.push(format!(
                    "keep-awake: {} could not be checked: {err}",
                    self.how
                ));
            }
        }
        if self.retry_after.is_some_and(|at| Instant::now() < at) {
            self.publish();
            return lines;
        }
        self.retry_after = None;
        let Some((program, args)) = (self.holder)(mode, std::process::id()) else {
            let note = format!(
                "keep-awake is not available on {}; the setting is on but nothing holds the machine awake",
                std::env::consts::OS
            );
            if self.note != note {
                lines.push(format!("keep-awake: {note}"));
                self.note = note;
            }
            self.publish();
            return lines;
        };
        let how = std::iter::once(program.clone())
            .chain(args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
        match std::process::Command::new(&program)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => {
                let lid = lid_note();
                let battery_now = self.power == Power::Battery && !holds_on_battery(self.os, mode);
                lines.push(format!(
                    "keeping this machine awake while PULSE runs ({how}, pid {}){}{}{}",
                    child.id(),
                    if mode == Mode::Always {
                        ", on battery too"
                    } else {
                        ", while plugged in"
                    },
                    if battery_now {
                        "; on battery now, so the hold waits for the charger"
                    } else {
                        ""
                    },
                    if lid.is_empty() {
                        String::new()
                    } else {
                        format!("; {lid}")
                    }
                ));
                self.child = Some(child);
                self.spawned = Some(Instant::now());
                self.since = now.to_string();
                self.how = how;
                self.note.clear();
            }
            Err(err) => {
                self.note = format!(
                    "{program} could not start ({err}); trying again in {}",
                    pulse_cli::format_duration(Duration::from_secs(RETRY_SECONDS))
                );
                self.how = how;
                self.retry_after = Some(Instant::now() + Duration::from_secs(RETRY_SECONDS));
                lines.push(format!("keep-awake: {}", self.note));
            }
        }
        self.publish();
        lines
    }

    /// Let the machine sleep again; true when a hold was held.
    pub fn release(&mut self) -> bool {
        let Some(mut child) = self.child.take() else {
            return false;
        };
        let _ = child.kill();
        let _ = child.wait();
        self.spawned = None;
        self.since.clear();
        self.charger_wait = false;
        self.publish();
        true
    }

    /// Compare the clocks with the last tick's: the seconds the machine
    /// slept in between, when it did.
    pub fn notice_sleep(&mut self, now: &str) -> Option<u64> {
        let wall = SystemTime::now();
        let mono = Instant::now();
        let gap = sleep_gap(
            wall.duration_since(self.last_wall).unwrap_or_default(),
            mono.duration_since(self.last_mono),
        );
        self.last_wall = wall;
        self.last_mono = mono;
        if let Some(secs) = gap {
            self.slept = Some((now.to_string(), secs));
            self.publish();
        }
        gap
    }

    /// The heartbeat fields: `awake` (the hold) and `slept` (the last sleep).
    pub fn beat_value(&self) -> serde_json::Value {
        let mut awake = serde_json::json!({
            "mode": self.mode.as_str(),
            "holding": self.holding(),
            "since": self.since,
            "how": if self.holding() { self.how.as_str() } else { "" },
            "note": self.note,
            "lid": lid_note(),
            "power": self.power.as_str(),
            "effective": self.effective(),
        });
        if let Some(child) = &self.child {
            awake["pid"] = serde_json::json!(child.id());
        }
        let slept = match &self.slept {
            Some((until, secs)) => serde_json::json!({"until": until, "seconds": secs}),
            None => serde_json::Value::Null,
        };
        serde_json::json!({"awake": awake, "slept": slept})
    }

    fn publish(&self) {
        if let Ok(mut b) = BEAT.write() {
            *b = self.beat_value();
        }
    }
}

impl Drop for Awake {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parses_the_setting_values() {
        assert_eq!(Mode::parse("off"), Mode::Off);
        assert_eq!(Mode::parse(""), Mode::Off);
        assert_eq!(Mode::parse("nonsense"), Mode::Off);
        assert_eq!(Mode::parse("on"), Mode::PluggedIn);
        assert_eq!(Mode::parse(" on "), Mode::PluggedIn);
        assert_eq!(Mode::parse("always"), Mode::Always);
        assert_eq!(Mode::PluggedIn.as_str(), "on");
    }

    #[test]
    fn holder_commands_per_platform() {
        let (p, a) = holder_for("macos", Mode::PluggedIn, 42).unwrap();
        assert_eq!(p, "caffeinate");
        assert_eq!(a, ["-s", "-w", "42"]);
        let (_, a) = holder_for("macos", Mode::Always, 42).unwrap();
        assert_eq!(a, ["-s", "-i", "-w", "42"]);
        let (p, a) = holder_for("linux", Mode::PluggedIn, 7).unwrap();
        assert_eq!(p, "systemd-inhibit");
        assert!(a.contains(&"--pid=7".to_string()));
        assert!(a.contains(&"--what=sleep:idle".to_string()));
        assert!(holder_for("windows", Mode::Always, 1).is_none());
        assert!(holder_command(Mode::Off, 1).is_none());
    }

    #[test]
    fn sleep_gap_ignores_clock_adjustments() {
        assert_eq!(
            sleep_gap(Duration::from_secs(30), Duration::from_secs(30)),
            None
        );
        assert_eq!(
            sleep_gap(Duration::from_secs(89), Duration::from_secs(30)),
            None
        );
        assert_eq!(
            sleep_gap(Duration::from_secs(30), Duration::from_secs(90)),
            None
        );
        assert_eq!(
            sleep_gap(Duration::from_secs(3630), Duration::from_secs(30)),
            Some(3600)
        );
    }

    #[test]
    fn off_holds_nothing_and_says_nothing() {
        let mut a = Awake::with_holder(Box::new(|_, _| {
            Some(("/bin/sleep".to_string(), vec!["30".to_string()]))
        }));
        assert!(a.tick_mode(Mode::Off, "2026-01-01T00:00:00Z").is_empty());
        assert!(!a.holding());
        let beat = a.beat_value();
        assert_eq!(beat["awake"]["mode"], "off");
        assert_eq!(beat["awake"]["holding"], false);
        assert!(beat["slept"].is_null());
    }

    #[test]
    fn on_takes_the_hold_and_off_releases_it() {
        let mut a = Awake::with_holder(Box::new(|_, _| {
            Some(("/bin/sleep".to_string(), vec!["30".to_string()]))
        }));
        let lines = a.tick_mode(Mode::PluggedIn, "2026-01-01T00:00:00Z");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("keeping this machine awake while PULSE runs (/bin/sleep 30, pid")
        );
        assert!(lines[0].contains("while plugged in"));
        assert!(a.holding());
        let beat = a.beat_value();
        assert_eq!(beat["awake"]["mode"], "on");
        assert_eq!(beat["awake"]["holding"], true);
        assert_eq!(beat["awake"]["since"], "2026-01-01T00:00:00Z");
        assert_eq!(beat["awake"]["how"], "/bin/sleep 30");
        assert!(beat["awake"]["pid"].as_u64().is_some());
        // The same mode again: a healthy hold is kept, quietly.
        assert!(
            a.tick_mode(Mode::PluggedIn, "2026-01-01T00:01:00Z")
                .is_empty()
        );
        assert_eq!(a.beat_value()["awake"]["since"], "2026-01-01T00:00:00Z");
        // A changed mode takes a new hold.
        let lines = a.tick_mode(Mode::Always, "2026-01-01T00:02:00Z");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("on battery too"));
        assert_eq!(a.beat_value()["awake"]["since"], "2026-01-01T00:02:00Z");
        let lines = a.tick_mode(Mode::Off, "2026-01-01T00:03:00Z");
        assert_eq!(lines, ["keep-awake off: the machine may sleep again"]);
        assert!(!a.holding());
        assert_eq!(a.beat_value()["awake"]["holding"], false);
    }

    #[test]
    fn a_holder_that_exits_at_once_waits_before_the_next_try() {
        let mut a = Awake::with_holder(Box::new(|_, _| {
            Some(("/bin/sleep".to_string(), vec!["0".to_string()]))
        }));
        let lines = a.tick_mode(Mode::PluggedIn, "2026-01-01T00:00:00Z");
        assert_eq!(lines.len(), 1);
        let mut child_gone = false;
        for _ in 0..50 {
            if let Some(child) = a.child.as_mut()
                && child.try_wait().ok().flatten().is_some()
            {
                child_gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(child_gone, "sleep 0 should have exited");
        let lines = a.tick_mode(Mode::PluggedIn, "2026-01-01T00:00:15Z");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("exited at once"), "{lines:?}");
        assert!(!a.holding());
        assert!(a.retry_after.is_some());
        // Inside the retry wait nothing is spawned and nothing is said.
        assert!(
            a.tick_mode(Mode::PluggedIn, "2026-01-01T00:00:30Z")
                .is_empty()
        );
        assert!(!a.holding());
        assert!(
            a.beat_value()["awake"]["note"]
                .as_str()
                .unwrap()
                .contains("exited at once")
        );
    }

    #[test]
    fn a_missing_program_is_noted_once_and_retried_later() {
        let mut a = Awake::with_holder(Box::new(|_, _| {
            Some(("/nonexistent/pulse-caffeinate".to_string(), vec![]))
        }));
        let lines = a.tick_mode(Mode::PluggedIn, "2026-01-01T00:00:00Z");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("could not start"), "{lines:?}");
        assert!(
            a.tick_mode(Mode::PluggedIn, "2026-01-01T00:00:15Z")
                .is_empty()
        );
        assert!(!a.holding());
    }

    #[test]
    fn no_holder_on_this_platform_is_said_once() {
        let mut a = Awake::with_holder(Box::new(|_, _| None));
        let lines = a.tick_mode(Mode::PluggedIn, "2026-01-01T00:00:00Z");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("not available"), "{lines:?}");
        assert!(
            a.tick_mode(Mode::PluggedIn, "2026-01-01T00:00:15Z")
                .is_empty()
        );
        assert_eq!(a.beat_value()["awake"]["holding"], false);
        assert!(
            a.beat_value()["awake"]["note"]
                .as_str()
                .unwrap()
                .contains("not available")
        );
    }

    #[test]
    fn notice_sleep_records_a_jump_in_the_wall_clock() {
        let mut a = Awake::new();
        assert_eq!(a.notice_sleep("2026-01-01T00:00:00Z"), None);
        // Pretend the last tick was an hour ago by the wall clock but a
        // moment ago by the monotonic clock: the machine slept.
        a.last_wall = SystemTime::now() - Duration::from_secs(3600);
        a.last_mono = Instant::now();
        let gap = a.notice_sleep("2026-01-01T01:00:00Z").expect("a sleep");
        assert!((3599..=3601).contains(&gap), "{gap}");
        let beat = a.beat_value();
        assert_eq!(beat["slept"]["until"], "2026-01-01T01:00:00Z");
        assert_eq!(beat["slept"]["seconds"].as_u64(), Some(gap));
        // The next tick starts from fresh clocks.
        assert_eq!(a.notice_sleep("2026-01-01T01:00:15Z"), None);
    }

    #[test]
    fn pmset_output_names_the_power_source() {
        assert_eq!(
            parse_pmset("Now drawing from 'AC Power'\n -InternalBattery-0 (id=1)\t100%; charged"),
            Power::Ac
        );
        assert_eq!(
            parse_pmset("Now drawing from 'Battery Power'\n -InternalBattery-0\t87%; discharging"),
            Power::Battery
        );
        assert_eq!(parse_pmset(""), Power::Unknown);
        assert_eq!(parse_pmset("pmset: command not found"), Power::Unknown);
    }

    #[test]
    fn sysfs_power_supplies_name_the_power_source() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(power_from_sysfs(root), Power::Unknown);
        std::fs::create_dir(root.join("BAT0")).unwrap();
        std::fs::write(root.join("BAT0/type"), "Battery\n").unwrap();
        assert_eq!(
            power_from_sysfs(root),
            Power::Unknown,
            "a battery alone says nothing"
        );
        std::fs::create_dir(root.join("ADP1")).unwrap();
        std::fs::write(root.join("ADP1/type"), "Mains\n").unwrap();
        std::fs::write(root.join("ADP1/online"), "0\n").unwrap();
        assert_eq!(power_from_sysfs(root), Power::Battery);
        std::fs::write(root.join("ADP1/online"), "1\n").unwrap();
        assert_eq!(power_from_sysfs(root), Power::Ac);
        assert_eq!(power_from_sysfs(&root.join("missing")), Power::Unknown);
    }

    #[test]
    fn battery_rules_follow_the_platform_and_the_mode() {
        assert!(!holds_on_battery("macos", Mode::PluggedIn));
        assert!(holds_on_battery("macos", Mode::Always));
        assert!(holds_on_battery("linux", Mode::PluggedIn));
        assert!(holds_on_battery("linux", Mode::Always));
    }

    #[test]
    fn a_mac_on_battery_waits_for_the_charger_and_says_so_once() {
        use std::sync::{Arc, Mutex};
        let power = Arc::new(Mutex::new(Power::Battery));
        let probe_power = Arc::clone(&power);
        let mut a = Awake::with_holder_and_probe(
            Box::new(|_, _| Some(("/bin/sleep".to_string(), vec!["30".to_string()]))),
            Box::new(move || *probe_power.lock().unwrap()),
        );
        a.os = "macos";
        let lines = a.tick_mode(Mode::PluggedIn, "2026-01-01T00:00:00Z");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("on battery now, so the hold waits for the charger"));
        assert!(a.holding());
        assert!(!a.effective());
        let beat = a.beat_value();
        assert_eq!(beat["awake"]["holding"], true);
        assert_eq!(beat["awake"]["effective"], false);
        assert_eq!(beat["awake"]["power"], "battery");

        // Still on battery: nothing new to say.
        a.power_read = None;
        assert!(
            a.tick_mode(Mode::PluggedIn, "2026-01-01T00:01:00Z")
                .is_empty()
        );

        *power.lock().unwrap() = Power::Ac;
        a.power_read = None;
        let lines = a.tick_mode(Mode::PluggedIn, "2026-01-01T00:02:00Z");
        assert_eq!(
            lines,
            vec!["keep-awake: plugged in again; holding this machine awake"]
        );
        assert!(a.effective());
        assert_eq!(a.beat_value()["awake"]["power"], "ac");

        *power.lock().unwrap() = Power::Battery;
        a.power_read = None;
        let lines = a.tick_mode(Mode::PluggedIn, "2026-01-01T00:03:00Z");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("keep-awake: on battery, so the hold waits for the charger"));
        assert!(!a.effective());

        // `always` holds on battery too, so the wait ends with the mode.
        a.power_read = None;
        let lines = a.tick_mode(Mode::Always, "2026-01-01T00:04:00Z");
        assert!(a.effective(), "{lines:?}");
        assert!(
            lines.iter().all(|l| !l.contains("waits for the charger")),
            "{lines:?}"
        );
        a.release();
    }

    #[test]
    fn a_probe_is_not_read_while_keep_awake_is_off() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let reads = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&reads);
        let mut a = Awake::with_holder_and_probe(
            Box::new(|_, _| Some(("/bin/sleep".to_string(), vec!["30".to_string()]))),
            Box::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                Power::Ac
            }),
        );
        a.tick_mode(Mode::Off, "2026-01-01T00:00:00Z");
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        assert_eq!(a.beat_value()["awake"]["power"], "unknown");
        a.tick_mode(Mode::PluggedIn, "2026-01-01T00:00:00Z");
        a.tick_mode(Mode::PluggedIn, "2026-01-01T00:00:01Z");
        assert_eq!(
            reads.load(Ordering::SeqCst),
            1,
            "one read per {POWER_PROBE_SECONDS}s"
        );
        a.release();
    }

    #[test]
    fn tick_reads_the_setting_from_the_store() {
        let _serial = crate::test_helpers::serial();
        let store = crate::test_helpers::new_store();
        let s: &pulse_store::Store = &store;
        let mut a = Awake::with_holder(Box::new(|_, _| {
            Some(("/bin/sleep".to_string(), vec!["30".to_string()]))
        }));
        assert!(a.tick(s).is_empty());
        assert!(!a.holding());
        s.set_setting(crate::util::HUMAN_AGENT, SETTING, "on")
            .unwrap();
        let lines = a.tick(s);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(a.holding());
        assert_eq!(a.mode(), Mode::PluggedIn);
        s.set_setting(crate::util::HUMAN_AGENT, SETTING, "off")
            .unwrap();
        a.tick(s);
        assert!(!a.holding());
    }
}
