//! The load governor: how many runs this machine takes at once and when it
//! leases nothing new because the machine is saturated.
//!
//! `workers` says how many local slots the owner would like; before this
//! governor a laptop with `workers 12` ran twelve `npm ci`/`cargo test`
//! trees at once, swapped, and the kernel killed workers (exit 137) whose
//! jobs then failed and were re-planned. The governor caps the local slots
//! at `daemon.max_jobs` (default derived from the hardware: half the cores,
//! one run per 4 GB of RAM, never more than four), holds every local slot
//! while the one-minute load average is over `daemon.max_load` (default:
//! the number of CPUs) or the memory a new run could use is under
//! `daemon.min_free_mem_mb` (default 1024), and runs the daemon's children
//! at nice [`CHILD_NICE`] so the owner's own work stays responsive. Remote
//! executors keep their own `workers`: their load is not this machine's.
//!
//! [`decide`] is the pure decision the loop and the self-update build use;
//! [`Machine::detect`] and [`pressure`] are the only parts that touch the
//! hardware, and each falls back to "no pressure" where the platform has
//! no reading.

use std::sync::atomic::{AtomicI32, Ordering};

/// Nice level the daemon's job children run at (`setpriority`); children
/// of children inherit it, so a worker's `npm ci` and `cargo test` yield
/// to the owner's foreground work and to the daemon's own loop.
pub(crate) const CHILD_NICE: i32 = 10;
/// The highest nice level `setpriority` accepts; `daemon.nice` is clamped to it.
pub(crate) const MAX_NICE: i32 = 19;
/// The hardware-derived default never admits more local runs than this.
pub(crate) const DEFAULT_MAX_JOBS_CAP: i32 = 4;
/// Megabytes of RAM one run is taken to need when the default is derived:
/// a worker clone with its `npm ci`, a test run and the harness itself.
const MB_PER_JOB: i64 = 4096;

/// The nice level [`renice_child`] applies; 0 until the daemon sets it, so
/// `pulse work` and the other commands run as before.
static CHILD_NICE_LEVEL: AtomicI32 = AtomicI32::new(0);

/// The environment variable Cargo reads its `build.jobs` from; every build a
/// child of the daemon runs (a worker's `cargo test`, the pre-commit
/// verification, the landing verification) gets it from [`build_env`].
pub(crate) const BUILD_JOBS_VAR: &str = "CARGO_BUILD_JOBS";

/// The parallelism [`build_env`] exports; 0 until the daemon sets it, so
/// `pulse work` and the other commands leave the environment alone.
static BUILD_JOBS_LEVEL: AtomicI32 = AtomicI32::new(0);

/// The machine every test runs on (four local runs by default).
#[cfg(test)]
pub(crate) const TEST_MACHINE: Machine = Machine {
    cpus: 12,
    cores: 12,
    mem_mb: 16384,
};

/// What this machine has; read once per process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Machine {
    /// Logical CPUs: the load average counts runnable threads against these.
    pub(crate) cpus: i32,
    /// Physical cores (the logical count where the platform cannot tell).
    pub(crate) cores: i32,
    pub(crate) mem_mb: i64,
}

impl Machine {
    /// The machine the daemon runs on. Under test a fixed 12-core, 16 GB
    /// machine, so a dispatch test leases the same on every test host;
    /// [`Machine::read`] is the hardware reading itself.
    pub(crate) fn detect() -> Self {
        #[cfg(test)]
        {
            TEST_MACHINE
        }
        #[cfg(not(test))]
        {
            Self::read()
        }
    }

    /// What the hardware says.
    pub(crate) fn read() -> Self {
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get() as i32)
            .unwrap_or(1)
            .max(1);
        let cores = physical_cores().unwrap_or(cpus).clamp(1, cpus.max(1));
        let mem_mb = total_mem_mb().unwrap_or(0).max(0);
        Self {
            cpus,
            cores,
            mem_mb,
        }
    }

    /// The default `daemon.max_jobs`: half the cores, at most one run per
    /// 4 GB of RAM (16 GB: four, 8 GB: two), never more than
    /// [`DEFAULT_MAX_JOBS_CAP`] and never under one. A machine whose memory
    /// size is unknown is limited by its cores alone.
    pub(crate) fn default_max_jobs(&self) -> i32 {
        let by_cores = (self.cores / 2).max(1);
        let by_mem = if self.mem_mb > 0 {
            ((self.mem_mb + MB_PER_JOB / 2) / MB_PER_JOB).max(1) as i32
        } else {
            by_cores
        };
        by_cores.min(by_mem).clamp(1, DEFAULT_MAX_JOBS_CAP)
    }

    /// Parallelism for a build the daemon runs for itself (`cargo -j`) and
    /// the default `daemon.build_jobs`: half the cores, so no single build
    /// takes the whole machine from the owner or the other runs.
    pub(crate) fn build_jobs(&self) -> i32 {
        (self.cores / 2).max(1)
    }
}

/// A reading of the machine's pressure now.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Pressure {
    /// The one-minute load average; None where the platform has none.
    pub(crate) load1: Option<f64>,
    /// Megabytes a new run could use without swapping: free, inactive and
    /// speculative pages on macOS, `MemAvailable` on Linux; None elsewhere.
    pub(crate) free_mem_mb: Option<i64>,
    /// The kernel itself reports memory pressure (macOS
    /// `kern.memorystatus_vm_pressure_level` at warning or critical).
    pub(crate) mem_warning: bool,
}

/// The limits in force.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Limits {
    /// Local runs at once.
    pub(crate) max_jobs: i32,
    /// One-minute load average over which nothing new starts.
    pub(crate) max_load: f64,
    /// Megabytes of usable memory under which nothing new starts; 0 off.
    pub(crate) min_free_mem_mb: i64,
    /// Nice level the daemon's own process runs at from startup
    /// (`daemon.nice`); 0 leaves the level the service started it at.
    pub(crate) own_nice: i32,
    /// Compile jobs one build may use (`daemon.build_jobs`, else half the
    /// cores): the `CARGO_BUILD_JOBS` the children get and the `-j` of the
    /// self-update build.
    pub(crate) build_jobs: i32,
    /// Where `max_jobs` came from, for the startup line.
    pub(crate) source: &'static str,
}

/// The limits from the settings, the flag and the machine: a positive
/// `--max-jobs` wins, then a positive `daemon.max_jobs`, then the machine's
/// default; `daemon.max_load` 0 is the number of CPUs; `daemon.min_free_mem_mb`
/// 0 turns the memory floor off; `daemon.build_jobs` 0 is half the cores.
pub(crate) fn limits(s: &pulse_store::Store, machine: &Machine, flag_max_jobs: i32) -> Limits {
    let setting_max_jobs = s.setting_int("daemon.max_jobs") as i32;
    let (max_jobs, source) = if flag_max_jobs > 0 {
        (flag_max_jobs, "--max-jobs")
    } else if setting_max_jobs > 0 {
        (setting_max_jobs, "daemon.max_jobs")
    } else {
        (machine.default_max_jobs(), "auto")
    };
    let max_load = match s.setting_int("daemon.max_load") {
        n if n > 0 => n as f64,
        _ => f64::from(machine.cpus.max(1)),
    };
    let build_jobs = match s.setting_int("daemon.build_jobs") {
        n if n > 0 => n.min(i64::from(i32::MAX)) as i32,
        _ => machine.build_jobs(),
    };
    Limits {
        max_jobs: max_jobs.max(1),
        max_load,
        min_free_mem_mb: s.setting_int("daemon.min_free_mem_mb").max(0),
        own_nice: own_nice_level(s.setting_int("daemon.nice")),
        build_jobs,
        source,
    }
}

/// What the governor says this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Verdict {
    /// Local runs that may still start under the cap (0 while held).
    pub(crate) room: usize,
    /// Why the machine takes nothing new right now, when it is saturated.
    pub(crate) hold: Option<String>,
    /// One local slot stays open to the owner's work alone (the lead's runs,
    /// answers, the jobs of owner tasks): the hold is load alone, the load is
    /// under [`EXPRESS_LOAD_FACTOR`] times the cap, and a slot is free under
    /// `daemon.max_jobs`. The fleet's own builds are what saturate the
    /// machine; the owner's request should not queue behind them.
    pub(crate) express: bool,
}

/// How far over `daemon.max_load` the express lane still opens: a machine at
/// twice the cap is swapping, and one more run would not finish sooner.
pub(crate) const EXPRESS_LOAD_FACTOR: f64 = 2.0;

/// The pure decision: `running` local runs are in flight, `reserved` slots
/// are kept free for something else (a self-update build that waits).
pub(crate) fn decide(
    limits: &Limits,
    pressure: &Pressure,
    running: usize,
    reserved: usize,
) -> Verdict {
    let hold = hold_reason(limits, pressure);
    let cap = limits.max_jobs.max(0) as usize;
    let room = if hold.is_some() {
        0
    } else {
        cap.saturating_sub(running).saturating_sub(reserved)
    };
    let load_only = hold.is_some()
        && !pressure.mem_warning
        && !pressure
            .free_mem_mb
            .is_some_and(|free| limits.min_free_mem_mb > 0 && free < limits.min_free_mem_mb)
        && pressure
            .load1
            .is_some_and(|load| load <= limits.max_load * EXPRESS_LOAD_FACTOR);
    let express = load_only && running + reserved < cap;
    Verdict {
        room,
        hold,
        express,
    }
}

/// Why nothing new should start on this machine, if the pressure says so.
/// The numbers that drift (load, megabytes) are normalised by the idle log,
/// so the line is said once per change and summarised hourly, not per tick.
pub(crate) fn hold_reason(limits: &Limits, pressure: &Pressure) -> Option<String> {
    if let Some(load) = pressure.load1
        && load > limits.max_load
    {
        return Some(format!(
            "machine saturated: load {load:.1} is over daemon.max_load {:.0}",
            limits.max_load
        ));
    }
    if pressure.mem_warning {
        return Some("machine saturated: the kernel reports memory pressure".to_string());
    }
    if let Some(free) = pressure.free_mem_mb
        && limits.min_free_mem_mb > 0
        && free < limits.min_free_mem_mb
    {
        return Some(format!(
            "machine saturated: {free} MB of memory free, under daemon.min_free_mem_mb {}",
            limits.min_free_mem_mb
        ));
    }
    None
}

/// Why a self-update build should wait, if it should: the machine is
/// saturated, or every slot under the cap holds a run (the dispatcher keeps
/// one slot free while the build waits, so it starts within one run).
pub(crate) fn build_hold(limits: &Limits, pressure: &Pressure, running: usize) -> Option<String> {
    if let Some(hold) = hold_reason(limits, pressure) {
        return Some(hold);
    }
    if running >= limits.max_jobs.max(1) as usize {
        return Some(format!(
            "{running} local run(s) in flight fill daemon.max_jobs {}",
            limits.max_jobs
        ));
    }
    None
}

/// Keeps at most `room` local slots in `slots` (remote slots stay: their
/// machines carry their own load); returns how many local slots went.
pub(crate) fn trim_local_slots(slots: &mut Vec<String>, room: usize) -> usize {
    let mut kept = 0usize;
    let before = slots.len();
    slots.retain(|name| {
        if name != crate::executor::LOCAL_EXECUTOR {
            return true;
        }
        kept += 1;
        kept <= room
    });
    before - slots.len()
}

/// The startup line naming the limits in force.
pub(crate) fn describe(machine: &Machine, limits: &Limits) -> String {
    let floor = if limits.min_free_mem_mb > 0 {
        format!("under {} MB of memory free", limits.min_free_mem_mb)
    } else {
        "never for memory".to_string()
    };
    let own = if limits.own_nice > 0 {
        format!(", the daemon itself at nice {}", limits.own_nice)
    } else {
        String::new()
    };
    format!(
        "load governor: up to {} local run(s) at once ({}; {} cores, {} CPUs, {} MB), nothing new over load {:.0} or {floor}; children run at nice {CHILD_NICE}, builds at -j{}{own}",
        limits.max_jobs,
        limits.source,
        machine.cores,
        machine.cpus,
        machine.mem_mb,
        limits.max_load,
        limits.build_jobs
    )
}

/// Lowers the daemon's own scheduling priority to `level` once, at startup,
/// so its planning, sweeps and landing passes yield to the owner's foreground
/// work while the loop still comes before its children at [`CHILD_NICE`];
/// 0 leaves the level the service started it at. A process cannot raise its
/// priority back, so under test (where the daemon runs in the test host's
/// process) nothing is applied.
pub(crate) fn renice_self(level: i32) {
    #[cfg(test)]
    {
        let _ = level;
    }
    #[cfg(not(test))]
    {
        renice_to(own_nice_level(i64::from(level)));
    }
}

/// `daemon.nice` as a level `setpriority` accepts.
pub(crate) fn own_nice_level(setting: i64) -> i32 {
    setting.clamp(0, i64::from(MAX_NICE)) as i32
}

/// The daemon sets the nice level its job children get; [`renice_child`]
/// applies it from a `pre_exec` hook. Other commands leave it at 0.
pub(crate) fn set_child_nice(level: i32) {
    CHILD_NICE_LEVEL.store(level.clamp(0, 19), Ordering::SeqCst);
}

pub(crate) fn child_nice() -> i32 {
    CHILD_NICE_LEVEL.load(Ordering::SeqCst)
}

/// The daemon sets the compile parallelism its children's builds get;
/// [`build_env`] exports it. Re-set each pass so a changed
/// `daemon.build_jobs` applies without a restart; 0 exports nothing.
pub(crate) fn set_build_jobs(jobs: i32) {
    BUILD_JOBS_LEVEL.store(jobs.max(0), Ordering::SeqCst);
}

pub(crate) fn build_jobs() -> i32 {
    BUILD_JOBS_LEVEL.load(Ordering::SeqCst)
}

/// Adds `CARGO_BUILD_JOBS=<daemon.build_jobs>` to a child's environment
/// (`KEY=VALUE` lines) when the daemon set a level and the environment does
/// not carry the variable already: an owner who exports it, or a recipe
/// that sets it, wins. Before this every `cargo test` a worker ran and every
/// landing verification compiled on all the cores at once, four runs deep,
/// and the laptop ran hot for most of the day; Cargo reads this variable as
/// `build.jobs`, and a `-j` on the command line still overrides it.
pub(crate) fn build_env(env: &mut Vec<String>) {
    let jobs = build_jobs();
    if jobs <= 0 {
        return;
    }
    let prefix = format!("{BUILD_JOBS_VAR}=");
    if env.iter().any(|kv| kv.starts_with(&prefix)) {
        return;
    }
    env.push(format!("{prefix}{jobs}"));
}

/// Lowers the calling process's scheduling priority to the level the daemon
/// set, for use inside `CommandExt::pre_exec` (a plain system call, nothing
/// allocated): a no-op in processes that never set a level. Failure is not
/// an error - the child still runs, only at the parent's priority.
pub(crate) fn renice_child() -> std::io::Result<()> {
    renice_to(child_nice());
    Ok(())
}

/// Spawns `cmd` at the nice level the daemon set (nothing changes under
/// `pulse work` and the other commands, which set none): the one line a
/// spawn site adds next to `own_process_group`.
pub(crate) fn nice_child(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: the hook only issues one setpriority system call.
        unsafe {
            cmd.pre_exec(renice_child);
        }
    }
    cmd
}

/// Spawns `cmd` at nice [`CHILD_NICE`] whatever process runs it (the
/// self-update build, which only the daemon runs).
pub(crate) fn low_priority(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: the hook only issues one setpriority system call.
        unsafe {
            cmd.pre_exec(|| {
                renice_to(CHILD_NICE);
                Ok(())
            });
        }
    }
    cmd
}

#[cfg(unix)]
fn renice_to(level: i32) {
    if level > 0 {
        // SAFETY: setpriority on the calling process with a bounded level.
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, level);
        }
    }
}

#[cfg(not(unix))]
fn renice_to(_level: i32) {}

/// The machine's pressure right now, as the daemon sees it. Under test the
/// pinned pressure or a calm machine, so a loaded test host never turns a
/// dispatch test into a hold; [`read_pressure`] is the reading itself.
pub(crate) fn pressure() -> Pressure {
    #[cfg(test)]
    {
        (*TEST_PRESSURE.lock().unwrap_or_else(|e| e.into_inner())).unwrap_or(CALM)
    }
    #[cfg(not(test))]
    {
        read_pressure()
    }
}

/// What the hardware says; each part is None where the platform has no
/// reading, which the decision treats as no pressure.
pub(crate) fn read_pressure() -> Pressure {
    Pressure {
        load1: load1(),
        free_mem_mb: free_mem_mb(),
        mem_warning: mem_warning(),
    }
}

/// An idle machine, the pressure tests see unless they pin another.
#[cfg(test)]
pub(crate) const CALM: Pressure = Pressure {
    load1: Some(0.5),
    free_mem_mb: Some(8192),
    mem_warning: false,
};

/// Tests pin the pressure the daemon sees; [`pin_pressure`] sets it and the
/// guard it returns resets it.
#[cfg(test)]
pub(crate) static TEST_PRESSURE: std::sync::Mutex<Option<Pressure>> = std::sync::Mutex::new(None);

#[cfg(test)]
pub(crate) struct PressureGuard;

#[cfg(test)]
impl Drop for PressureGuard {
    fn drop(&mut self) {
        *TEST_PRESSURE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// The pressure the daemon sees until the guard drops.
#[cfg(test)]
pub(crate) fn pin_pressure(p: Pressure) -> PressureGuard {
    *TEST_PRESSURE.lock().unwrap_or_else(|e| e.into_inner()) = Some(p);
    PressureGuard
}

#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
))]
fn load1() -> Option<f64> {
    let mut loads = [0f64; 3];
    // SAFETY: getloadavg writes at most three doubles into the array given.
    let n = unsafe { libc::getloadavg(loads.as_mut_ptr(), 3) };
    (n >= 1 && loads[0].is_finite()).then_some(loads[0])
}

#[cfg(target_os = "linux")]
fn load1() -> Option<f64> {
    std::fs::read_to_string("/proc/loadavg")
        .ok()?
        .split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .filter(|l| l.is_finite())
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "linux"
)))]
fn load1() -> Option<f64> {
    None
}

#[cfg(target_os = "macos")]
fn sysctl_int(name: &str) -> Option<i64> {
    let cname = std::ffi::CString::new(name).ok()?;
    let mut buf = [0u8; 8];
    let mut len = buf.len();
    // SAFETY: the kernel writes at most `len` bytes into `buf` and updates `len`.
    let rc = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    match len {
        4 => Some(i64::from(i32::from_ne_bytes(buf[..4].try_into().ok()?))),
        8 => Some(i64::from_ne_bytes(buf)),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn physical_cores() -> Option<i32> {
    sysctl_int("hw.physicalcpu")
        .filter(|n| *n > 0)
        .map(|n| n.min(i64::from(i32::MAX)) as i32)
}

#[cfg(target_os = "macos")]
fn total_mem_mb() -> Option<i64> {
    sysctl_int("hw.memsize").map(|b| b / (1024 * 1024))
}

// libc points at the mach2 crate for this call; the workspace does not carry
// it for one function that libc still binds correctly.
#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn host_port() -> libc::mach_port_t {
    static PORT: std::sync::OnceLock<libc::mach_port_t> = std::sync::OnceLock::new();
    // SAFETY: mach_host_self has no preconditions; the port is kept for the
    // process's life rather than leaked once per reading.
    *PORT.get_or_init(|| unsafe { libc::mach_host_self() })
}

#[cfg(target_os = "macos")]
fn free_mem_mb() -> Option<i64> {
    let page = sysctl_int("hw.pagesize").filter(|p| *p > 0)? as u64;
    // SAFETY: vm_statistics64 is plain data; host_statistics64 fills at most
    // `count` integers of it.
    let mut stat: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
    let mut count = libc::HOST_VM_INFO64_COUNT;
    let rc = unsafe {
        libc::host_statistics64(
            host_port(),
            libc::HOST_VM_INFO64,
            (&mut stat as *mut libc::vm_statistics64).cast(),
            &mut count,
        )
    };
    if rc != libc::KERN_SUCCESS {
        return None;
    }
    let pages = u64::from(stat.free_count)
        + u64::from(stat.inactive_count)
        + u64::from(stat.speculative_count);
    Some((pages.saturating_mul(page) / (1024 * 1024)) as i64)
}

#[cfg(target_os = "macos")]
fn mem_warning() -> bool {
    // 1 normal, 2 warning, 4 critical; absent on older kernels.
    sysctl_int("kern.memorystatus_vm_pressure_level").is_some_and(|level| level >= 2)
}

#[cfg(target_os = "linux")]
fn meminfo_mb(key: &str) -> Option<i64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    meminfo_field_mb(&text, key)
}

#[cfg(any(target_os = "linux", test))]
fn meminfo_field_mb(text: &str, key: &str) -> Option<i64> {
    text.lines().find_map(|line| {
        let (name, rest) = line.split_once(':')?;
        if name.trim() != key {
            return None;
        }
        let kb: i64 = rest.split_whitespace().next()?.parse().ok()?;
        Some(kb / 1024)
    })
}

#[cfg(target_os = "linux")]
fn physical_cores() -> Option<i32> {
    None
}

#[cfg(target_os = "linux")]
fn total_mem_mb() -> Option<i64> {
    meminfo_mb("MemTotal")
}

#[cfg(target_os = "linux")]
fn free_mem_mb() -> Option<i64> {
    meminfo_mb("MemAvailable")
}

#[cfg(target_os = "linux")]
fn mem_warning() -> bool {
    false
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn physical_cores() -> Option<i32> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn total_mem_mb() -> Option<i64> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn free_mem_mb() -> Option<i64> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn mem_warning() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(max_jobs: i32, max_load: f64, min_free_mem_mb: i64) -> Limits {
        Limits {
            max_jobs,
            max_load,
            min_free_mem_mb,
            own_nice: 0,
            build_jobs: 6,
            source: "test",
        }
    }

    fn calm() -> Pressure {
        Pressure {
            load1: Some(2.0),
            free_mem_mb: Some(6000),
            mem_warning: false,
        }
    }

    #[test]
    fn default_max_jobs_follows_cores_memory_and_the_cap() {
        for (name, cpus, cores, mem_mb, want) in [
            ("16 GB, 12 cores: four", 12, 12, 16384, 4),
            ("16 GB reported as 15.6 GB rounds to four", 12, 12, 15990, 4),
            ("8 GB, 8 cores: two", 8, 8, 8192, 2),
            ("4 GB, 8 cores: one", 8, 8, 4096, 1),
            ("2 GB, 2 cores: never under one", 2, 2, 2048, 1),
            (
                "64 GB, 32 cores: the cap",
                64,
                32,
                65536,
                DEFAULT_MAX_JOBS_CAP,
            ),
            ("16 GB, 4 cores: two", 8, 4, 16384, 2),
            ("one core", 1, 1, 16384, 1),
            ("memory unknown: cores alone", 12, 12, 0, 4),
        ] {
            let m = Machine {
                cpus,
                cores,
                mem_mb,
            };
            assert_eq!(m.default_max_jobs(), want, "{name}");
        }
        assert_eq!(
            Machine {
                cpus: 12,
                cores: 12,
                mem_mb: 16384
            }
            .build_jobs(),
            6
        );
        assert_eq!(
            Machine {
                cpus: 1,
                cores: 1,
                mem_mb: 1024
            }
            .build_jobs(),
            1
        );
    }

    #[test]
    fn decide_admits_the_room_under_the_cap_and_nothing_under_pressure() {
        let l = limits(4, 12.0, 1024);
        assert_eq!(
            decide(&l, &calm(), 0, 0),
            Verdict {
                room: 4,
                hold: None,
                express: false
            }
        );
        assert_eq!(decide(&l, &calm(), 3, 0).room, 1);
        assert_eq!(decide(&l, &calm(), 4, 0).room, 0, "the cap is full");
        assert_eq!(
            decide(&l, &calm(), 7, 0).room,
            0,
            "over the cap never underflows"
        );
        assert_eq!(
            decide(&l, &calm(), 2, 1).room,
            1,
            "a reserved slot is not offered"
        );
        assert_eq!(decide(&l, &calm(), 3, 1).room, 0);
        let loaded = Pressure {
            load1: Some(40.1),
            ..calm()
        };
        let v = decide(&l, &loaded, 0, 0);
        assert_eq!(v.room, 0, "load over the cap holds every slot: {v:?}");
        assert_eq!(
            v.hold.as_deref(),
            Some("machine saturated: load 40.1 is over daemon.max_load 12")
        );
        assert!(
            !v.express,
            "a load over twice the cap keeps the express lane shut: {v:?}"
        );
        let tight = Pressure {
            free_mem_mb: Some(512),
            ..calm()
        };
        let v = decide(&l, &tight, 0, 0);
        assert_eq!(v.room, 0);
        assert_eq!(
            v.hold.as_deref(),
            Some("machine saturated: 512 MB of memory free, under daemon.min_free_mem_mb 1024")
        );
        assert!(!v.express, "memory pressure has no express lane");
        let warned = Pressure {
            mem_warning: true,
            ..calm()
        };
        assert_eq!(
            decide(&l, &warned, 0, 0).hold.as_deref(),
            Some("machine saturated: the kernel reports memory pressure")
        );
    }

    #[test]
    fn decide_keeps_an_express_slot_for_the_owner_while_load_alone_holds() {
        let l = limits(4, 12.0, 1024);
        let busy = Pressure {
            load1: Some(15.0),
            ..calm()
        };
        let v = decide(&l, &busy, 2, 0);
        assert_eq!(v.room, 0, "the hold still closes the ordinary slots");
        assert!(v.hold.is_some());
        assert!(v.express, "load alone, under the cap: {v:?}");
        assert!(
            !decide(&l, &busy, 4, 0).express,
            "a full cap has no slot to lend"
        );
        assert!(
            !decide(&l, &busy, 3, 1).express,
            "a reserved slot counts against the cap"
        );
        let swamped = Pressure {
            load1: Some(24.1),
            ..calm()
        };
        assert!(!decide(&l, &swamped, 0, 0).express);
        let loaded_and_tight = Pressure {
            load1: Some(15.0),
            free_mem_mb: Some(512),
            ..calm()
        };
        assert!(!decide(&l, &loaded_and_tight, 0, 0).express);
        assert!(
            !decide(&l, &calm(), 0, 0).express,
            "no hold, no lane needed"
        );
    }

    #[test]
    fn decide_treats_a_missing_reading_and_a_zero_floor_as_no_pressure() {
        let l = limits(2, 8.0, 1024);
        let blind = Pressure::default();
        assert_eq!(
            decide(&l, &blind, 0, 0),
            Verdict {
                room: 2,
                hold: None,
                express: false
            }
        );
        let off = limits(2, 8.0, 0);
        let tight = Pressure {
            free_mem_mb: Some(10),
            ..calm()
        };
        assert_eq!(decide(&off, &tight, 0, 0).hold, None, "floor 0 is off");
        let at_the_line = Pressure {
            load1: Some(8.0),
            ..calm()
        };
        assert_eq!(
            decide(&l, &at_the_line, 0, 0).hold,
            None,
            "a load equal to the cap is not over it"
        );
    }

    #[test]
    fn build_hold_waits_for_pressure_or_a_free_slot() {
        let l = limits(4, 12.0, 1024);
        assert_eq!(build_hold(&l, &calm(), 3), None);
        assert_eq!(
            build_hold(&l, &calm(), 4).as_deref(),
            Some("4 local run(s) in flight fill daemon.max_jobs 4")
        );
        assert_eq!(
            build_hold(&l, &calm(), 9).as_deref(),
            Some("9 local run(s) in flight fill daemon.max_jobs 4")
        );
        let loaded = Pressure {
            load1: Some(30.0),
            ..calm()
        };
        assert!(
            build_hold(&l, &loaded, 0)
                .is_some_and(|h| h.starts_with("machine saturated: load 30.0")),
            "pressure holds the build even with every slot free"
        );
    }

    #[test]
    fn trim_local_slots_keeps_remote_slots_and_the_room() {
        let local = crate::executor::LOCAL_EXECUTOR.to_string();
        let mut slots = vec![
            "hand-a".to_string(),
            local.clone(),
            local.clone(),
            "hand-b".to_string(),
            local.clone(),
        ];
        assert_eq!(trim_local_slots(&mut slots, 1), 2);
        assert_eq!(
            slots,
            vec!["hand-a".to_string(), local.clone(), "hand-b".to_string()]
        );
        assert_eq!(trim_local_slots(&mut slots, 0), 1);
        assert_eq!(slots, vec!["hand-a".to_string(), "hand-b".to_string()]);
        assert_eq!(
            trim_local_slots(&mut slots, 0),
            0,
            "nothing local left to trim"
        );
        let mut plenty = vec![local.clone(), local.clone()];
        assert_eq!(
            trim_local_slots(&mut plenty, 5),
            0,
            "room beyond the slots trims nothing"
        );
        assert_eq!(plenty.len(), 2);
    }

    #[test]
    fn describe_names_the_limits_and_their_source() {
        let m = Machine {
            cpus: 12,
            cores: 12,
            mem_mb: 16384,
        };
        let line = describe(&m, &limits(4, 12.0, 1024));
        assert!(
            line.contains("up to 4 local run(s) at once (test; 12 cores, 12 CPUs, 16384 MB)")
                && line.contains("over load 12 or under 1024 MB of memory free")
                && line.ends_with("children run at nice 10, builds at -j6"),
            "{line}"
        );
        let off = describe(&m, &limits(2, 12.0, 0));
        assert!(off.contains("never for memory"), "{off}");
        let own = describe(
            &m,
            &Limits {
                own_nice: 5,
                ..limits(4, 12.0, 1024)
            },
        );
        assert!(
            own.ends_with("children run at nice 10, builds at -j6, the daemon itself at nice 5"),
            "{own}"
        );
    }

    #[test]
    fn own_nice_level_is_clamped_to_what_setpriority_accepts() {
        assert_eq!(own_nice_level(5), 5);
        assert_eq!(own_nice_level(40), MAX_NICE);
        assert_eq!(own_nice_level(-3), 0);
    }

    #[test]
    fn limits_prefer_the_flag_then_the_setting_then_the_machine() {
        let s = crate::test_helpers::new_store();
        let m = Machine {
            cpus: 12,
            cores: 12,
            mem_mb: 16384,
        };
        let l = limits_from(&s, &m, 0);
        assert_eq!((l.max_jobs, l.source), (4, "auto"));
        assert_eq!(l.max_load, 12.0, "daemon.max_load 0 is the CPU count");
        assert_eq!(l.min_free_mem_mb, 1024, "the documented default floor");
        assert_eq!(l.own_nice, 5, "the documented default daemon.nice");
        assert_eq!(l.build_jobs, 6, "daemon.build_jobs 0 is half the cores");
        crate::test_helpers_daemon::set_setting(&s, "daemon.max_jobs", "2");
        crate::test_helpers_daemon::set_setting(&s, "daemon.max_load", "20");
        crate::test_helpers_daemon::set_setting(&s, "daemon.min_free_mem_mb", "0");
        crate::test_helpers_daemon::set_setting(&s, "daemon.nice", "0");
        crate::test_helpers_daemon::set_setting(&s, "daemon.build_jobs", "3");
        let l = limits_from(&s, &m, 0);
        assert_eq!((l.max_jobs, l.source), (2, "daemon.max_jobs"));
        assert_eq!(l.max_load, 20.0);
        assert_eq!(l.min_free_mem_mb, 0);
        assert_eq!(l.own_nice, 0, "daemon.nice 0 leaves the priority alone");
        assert_eq!(l.build_jobs, 3);
        let l = limits_from(&s, &m, 3);
        assert_eq!((l.max_jobs, l.source), (3, "--max-jobs"));
        assert!(
            s.set_setting("human", "daemon.build_jobs", "-1").is_err(),
            "the catalogue refuses a negative parallelism"
        );
    }

    #[test]
    fn build_env_exports_cargo_build_jobs_once_the_daemon_sets_it() {
        let _serial = crate::test_helpers::serial();
        set_build_jobs(0);
        let mut env = vec!["PATH=/bin".to_string()];
        build_env(&mut env);
        assert_eq!(env, ["PATH=/bin"], "nothing is exported under pulse work");
        set_build_jobs(6);
        build_env(&mut env);
        assert_eq!(env, ["PATH=/bin", "CARGO_BUILD_JOBS=6"]);
        // The owner's own value, or a recipe's, is left alone.
        let mut own = vec!["CARGO_BUILD_JOBS=2".to_string()];
        build_env(&mut own);
        assert_eq!(own, ["CARGO_BUILD_JOBS=2"]);
        // A second call does not stack a second entry.
        build_env(&mut env);
        assert_eq!(env.len(), 2);
        // The daemon's own environment for children carries it from here on
        // (unless the test host exports its own, which wins).
        if std::env::var_os(BUILD_JOBS_VAR).is_none() {
            assert!(
                crate::proc::environ().contains(&"CARGO_BUILD_JOBS=6".to_string()),
                "proc::environ() folds the level in"
            );
            set_build_jobs(0);
            assert!(
                !crate::proc::environ()
                    .iter()
                    .any(|kv| kv.starts_with("CARGO_BUILD_JOBS=")),
                "cleared again"
            );
        }
        set_build_jobs(0);
    }

    fn limits_from(s: &crate::test_helpers::TestStore, m: &Machine, flag: i32) -> Limits {
        super::limits(s, m, flag)
    }

    #[test]
    fn meminfo_field_reads_kilobytes_as_megabytes() {
        let text = "MemTotal:       16318940 kB\nMemFree:          512000 kB\nMemAvailable:    8388608 kB\n";
        assert_eq!(meminfo_field_mb(text, "MemAvailable"), Some(8192));
        assert_eq!(meminfo_field_mb(text, "MemTotal"), Some(15936));
        assert_eq!(meminfo_field_mb(text, "SwapFree"), None);
    }

    #[test]
    fn the_machine_and_its_pressure_read_without_panicking() {
        let m = Machine::read();
        assert!(m.cpus >= 1 && m.cores >= 1 && m.cores <= m.cpus, "{m:?}");
        assert!(m.default_max_jobs() >= 1 && m.default_max_jobs() <= DEFAULT_MAX_JOBS_CAP);
        let p = read_pressure();
        assert!(p.load1.is_none_or(|l| l >= 0.0), "{p:?}");
        assert!(p.free_mem_mb.is_none_or(|mb| mb >= 0), "{p:?}");
        #[cfg(target_os = "macos")]
        {
            assert!(
                m.mem_mb > 0 && p.load1.is_some() && p.free_mem_mb.is_some(),
                "{m:?} {p:?}"
            );
        }
        assert_eq!(
            Machine::detect(),
            TEST_MACHINE,
            "tests see the fixed machine"
        );
    }

    #[test]
    fn tests_see_a_calm_machine_unless_they_pin_the_pressure() {
        let _serial = crate::test_helpers::serial();
        assert_eq!(pressure(), CALM);
        {
            let _pinned = pin_pressure(Pressure {
                load1: Some(40.0),
                ..CALM
            });
            assert_eq!(pressure().load1, Some(40.0));
        }
        assert_eq!(pressure(), CALM, "the guard reset the pin");
    }

    #[test]
    fn child_nice_is_zero_until_the_daemon_sets_it() {
        let _serial = crate::test_helpers::serial();
        set_child_nice(0);
        assert_eq!(child_nice(), 0);
        renice_child().unwrap();
        set_child_nice(CHILD_NICE);
        assert_eq!(child_nice(), CHILD_NICE);
        set_child_nice(99);
        assert_eq!(child_nice(), 19, "clamped to the nice range");
        set_child_nice(0);
    }

    /// The nice level a child shell reports for itself.
    #[cfg(unix)]
    fn child_nice_of(cmd: &mut std::process::Command) -> i32 {
        let out = cmd.output().expect("sh runs");
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("nice column: {out:?}"))
    }

    #[cfg(unix)]
    fn nice_probe() -> std::process::Command {
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.args(["-c", "ps -o nice= -p $$ 2>/dev/null || ps -o ni= -p $$"]);
        cmd
    }

    #[cfg(unix)]
    #[test]
    fn low_priority_runs_the_child_at_a_higher_nice() {
        let nice = child_nice_of(low_priority(&mut nice_probe()));
        assert!(
            nice >= CHILD_NICE,
            "child nice {nice}, want at least {CHILD_NICE}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn nice_child_follows_the_level_the_daemon_set() {
        let _serial = crate::test_helpers::serial();
        set_child_nice(0);
        let own = child_nice_of(&mut nice_probe());
        let plain = child_nice_of(nice_child(&mut nice_probe()));
        assert_eq!(
            plain, own,
            "no level set: the child runs at the parent's nice"
        );
        set_child_nice(CHILD_NICE);
        let niced = child_nice_of(nice_child(&mut nice_probe()));
        set_child_nice(0);
        assert!(
            niced >= CHILD_NICE && (niced > own || own >= CHILD_NICE),
            "child nice {niced} with the level set, parent {own}"
        );
    }
}
