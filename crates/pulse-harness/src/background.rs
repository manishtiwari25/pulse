//! Background processes: a command the model starts with `bash` and
//! `background: true` keeps running after the call returns, its output is
//! collected as it comes, and the `process` tool reads, waits for or stops
//! it. Every process belongs to the run that started it and ends with it.

use pulse_provider::{CancelToken, Tool};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// The name of the tool that manages background processes.
pub const PROCESS_TOOL: &str = "process";

/// How long a `bash` call with `background: true` waits for the command to
/// end before leaving it in the background, when the call does not say.
pub const BACKGROUND_WAIT_DEFAULT: Duration = Duration::from_secs(2);

/// The most a `bash` call may wait before backgrounding, and the most a
/// `process` wait blocks, whatever the call asks for (the step timeout
/// caps both further).
pub const BACKGROUND_WAIT_LIMIT: Duration = Duration::from_secs(600);

/// How long `process` with action `wait` waits when the call does not say.
pub const PROCESS_WAIT_DEFAULT: Duration = Duration::from_secs(30);

/// Background processes one run may have running at the same time.
pub const BACKGROUND_LIMIT: usize = 8;

/// Bytes of a background process's output kept in memory; the oldest go
/// first once it is full, and the model is told how many.
pub const OUTPUT_BUFFER_LIMIT: usize = 2 * 1024 * 1024;

/// Bytes of the output the model has already read that are kept for the
/// user (`/procs ID` shows the tail).
pub const RECENT_OUTPUT_LIMIT: usize = 64 * 1024;

/// A process the environment started in the background. The environment
/// owns the mechanics (pipes, process group, reaping); the harness owns
/// the bookkeeping around it.
pub trait Process: Send {
    fn pid(&self) -> i32;
    /// Some(exit code) once the process has ended; 128+signal when a
    /// signal ended it.
    fn exit_code(&mut self) -> Option<i32>;
    /// What the process wrote since the last call, stdout and stderr in the
    /// order they arrived, and how many bytes were dropped before that to
    /// keep the buffer within [`OUTPUT_BUFFER_LIMIT`].
    fn take_output(&mut self) -> (String, usize);
    /// The same, left in place.
    fn peek_output(&self) -> String;
    /// Blocks until the process ends, `timeout` passes or `cancel` fires;
    /// true when it has ended.
    fn wait(&mut self, cancel: &CancelToken, timeout: Duration) -> bool;
    /// Ends the process and everything in its group: SIGTERM, a grace
    /// period, then SIGKILL. True when it was still running.
    fn stop(&mut self) -> bool;
}

pub fn process_tool() -> Tool {
    Tool {
        name: PROCESS_TOOL.to_string(),
        description: format!(
            "Manage the commands started with bash background:true. action output returns what the process wrote since the last read, and its exit code once it has ended; wait blocks up to seconds (default {}) for it to end, then returns the same; stop ends it (SIGTERM, then SIGKILL) and returns the rest of its output; list shows every background process of this session with its state. Stop the processes you started when you no longer need them.",
            PROCESS_WAIT_DEFAULT.as_secs()
        ),
        parameters: json!({"type":"object","properties":{
            "action":{"type":"string","enum":["output","wait","stop","list"],"description":"What to do."},
            "id":{"type":"string","description":"The process id bash returned (bg-1). Required for output, wait and stop."},
            "seconds":{"type":"integer","description":"For wait: how long to wait at most for the process to end."}
        },"required":["action"]}),
    }
}

/// One managed background process.
pub struct Managed {
    pub id: String,
    pub command: String,
    proc: Box<dyn Process>,
    started: Instant,
    /// Set the first time the end was observed.
    ended: Option<(Instant, i32)>,
    /// Bytes of output the model has read so far (dropped bytes included).
    read_bytes: usize,
    /// The end of what the model has read, for the user to look at.
    recent: String,
}

impl Managed {
    pub fn pid(&self) -> i32 {
        self.proc.pid()
    }

    /// Looks at the process once more; Some(exit) when it has ended.
    pub fn exit(&mut self) -> Option<i32> {
        if let Some((_, code)) = self.ended {
            return Some(code);
        }
        let code = self.proc.exit_code()?;
        self.ended = Some((Instant::now(), code));
        Some(code)
    }

    pub fn running(&mut self) -> bool {
        self.exit().is_none()
    }

    /// How long the process ran, or has been running.
    pub fn elapsed(&self) -> Duration {
        match self.ended {
            Some((at, _)) => at.saturating_duration_since(self.started),
            None => self.started.elapsed(),
        }
    }

    /// `running 1m03s` or `ended with exit 0 after 12s`.
    pub fn state(&mut self) -> String {
        match self.exit() {
            Some(code) => format!("ended with exit {code} after {}", fmt_dur(self.elapsed())),
            None => format!("running for {}", fmt_dur(self.elapsed())),
        }
    }

    /// The output since the last read; the second value counts bytes the
    /// buffer had to drop before them.
    pub fn take_output(&mut self) -> (String, usize) {
        let (text, dropped) = self.proc.take_output();
        self.read_bytes += text.len() + dropped;
        self.recent.push_str(&text);
        if self.recent.len() > RECENT_OUTPUT_LIMIT {
            let cut = self
                .recent
                .ceil_char_boundary(self.recent.len() - RECENT_OUTPUT_LIMIT);
            self.recent.drain(..cut);
        }
        (text, dropped)
    }

    /// The last `lines` lines the process wrote, read by the model or not;
    /// nothing is consumed.
    pub fn peek_tail(&self, lines: usize) -> String {
        let all = format!("{}{}", self.recent, self.proc.peek_output());
        let mut kept: Vec<&str> = all.lines().rev().take(lines).collect();
        kept.reverse();
        kept.join("\n")
    }

    pub fn read_bytes(&self) -> usize {
        self.read_bytes
    }

    pub fn wait(&mut self, cancel: &CancelToken, timeout: Duration) -> bool {
        if self.ended.is_some() {
            return true;
        }
        let ended = self.proc.wait(cancel, timeout);
        if ended {
            self.exit();
        }
        ended
    }

    /// Stops the process; true when it was still running.
    pub fn stop(&mut self) -> bool {
        if self.exit().is_some() {
            return false;
        }
        let was_running = self.proc.stop();
        self.exit();
        was_running
    }

    /// The one-line description `list` and `/procs` show.
    pub fn describe(&mut self) -> String {
        let state = self.state();
        format!(
            "{} {} pid {}: {}",
            self.id,
            state,
            self.pid(),
            one_line(&self.command, 120)
        )
    }
}

/// The background processes of one run, in start order.
#[derive(Default)]
pub struct Procs {
    list: Vec<Managed>,
    next: usize,
}

impl Procs {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// How many are still running, looked at now.
    pub fn running(&mut self) -> usize {
        self.list
            .iter_mut()
            .map(|m| usize::from(m.exit().is_none()))
            .sum()
    }

    /// How many have not been seen to end, without looking again.
    pub fn unfinished(&self) -> usize {
        self.list.iter().filter(|m| m.ended.is_none()).count()
    }

    /// Registers a started process and returns its id (`bg-1`, `bg-2`, ...).
    pub fn add(&mut self, command: &str, proc: Box<dyn Process>) -> String {
        self.next += 1;
        let id = format!("bg-{}", self.next);
        self.list.push(Managed {
            id: id.clone(),
            command: command.to_string(),
            proc,
            started: Instant::now(),
            ended: None,
            read_bytes: 0,
            recent: String::new(),
        });
        id
    }

    pub fn get(&mut self, id: &str) -> Option<&mut Managed> {
        self.list.iter_mut().find(|m| m.id == id)
    }

    pub fn remove(&mut self, id: &str) -> Option<Managed> {
        let i = self.list.iter().position(|m| m.id == id)?;
        Some(self.list.remove(i))
    }

    pub fn ids(&self) -> Vec<String> {
        self.list.iter().map(|m| m.id.clone()).collect()
    }

    /// One line per process, for `list` and `/procs`.
    pub fn describe(&mut self) -> Vec<String> {
        self.list.iter_mut().map(Managed::describe).collect()
    }

    /// Stops every process still running and forgets them all; returns a
    /// description of each one that had to be stopped.
    pub fn stop_all(&mut self) -> Vec<String> {
        let mut stopped = Vec::new();
        for m in &mut self.list {
            if m.stop() {
                stopped.push(format!(
                    "{} pid {}: {}",
                    m.id,
                    m.pid(),
                    one_line(&m.command, 120)
                ));
            }
        }
        self.list.clear();
        stopped
    }
}

/// What a `process` call asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessAction {
    Output {
        id: String,
    },
    Wait {
        id: String,
        timeout: Option<Duration>,
    },
    Stop {
        id: String,
    },
    List,
}

impl ProcessAction {
    /// Parses the tool's JSON arguments.
    pub fn parse(arguments: &str) -> Result<Self, String> {
        let args: Value = serde_json::from_str(arguments)
            .map_err(|_| "arguments must be a JSON object with a string action".to_string())?;
        let action = args
            .get("action")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        let id = args
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        let need_id = |action: &str| -> Result<String, String> {
            if id.is_empty() {
                Err(format!("action {action} needs the id bash returned (bg-1)"))
            } else {
                Ok(id.clone())
            }
        };
        match action {
            "output" | "read" => Ok(Self::Output {
                id: need_id("output")?,
            }),
            "wait" => {
                let timeout = match args.get("seconds") {
                    None | Some(Value::Null) => None,
                    Some(v) => match v.as_f64() {
                        Some(s) if s >= 0.0 => Some(Duration::from_secs_f64(s)),
                        _ => return Err("seconds must be a non-negative number".to_string()),
                    },
                };
                Ok(Self::Wait {
                    id: need_id("wait")?,
                    timeout,
                })
            }
            "stop" | "kill" => Ok(Self::Stop {
                id: need_id("stop")?,
            }),
            "list" => Ok(Self::List),
            "" => Err("arguments must be a JSON object with a string action".to_string()),
            other => Err(format!(
                "unknown action {other}; one of output, wait, stop, list"
            )),
        }
    }

    pub fn spec(&self) -> String {
        match self {
            Self::Output { id } => format!("output {id}"),
            Self::Wait { id, timeout: None } => format!("wait {id}"),
            Self::Wait {
                id,
                timeout: Some(t),
            } => format!("wait {id} {}s", t.as_secs()),
            Self::Stop { id } => format!("stop {id}"),
            Self::List => "list".to_string(),
        }
    }
}

/// The `[dropped N bytes ...]` marker the model sees when the buffer
/// could not keep everything.
pub fn dropped_marker(dropped: usize) -> String {
    format!("[the process wrote faster than it was read: {dropped} older bytes were dropped]")
}

/// `12s`, `1m03s`, `2h05m`.
pub fn fmt_dur(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        if secs == 0 && d.as_millis() > 0 {
            return format!("{:.1}s", d.as_secs_f64());
        }
        return format!("{secs}s");
    }
    if secs < 3600 {
        return format!("{}m{:02}s", secs / 60, secs % 60);
    }
    format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
}

fn one_line(s: &str, limit: usize) -> String {
    let joined: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= limit {
        return joined;
    }
    let cut: String = joined.chars().take(limit.saturating_sub(1)).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    type Exit = Arc<Mutex<Option<i32>>>;
    type Chunks = Arc<Mutex<Vec<(String, usize)>>>;

    /// A process whose life the test scripts.
    struct Scripted {
        pid: i32,
        exit: Exit,
        chunks: Chunks,
        stopped: Arc<Mutex<bool>>,
    }

    impl Process for Scripted {
        fn pid(&self) -> i32 {
            self.pid
        }
        fn exit_code(&mut self) -> Option<i32> {
            *self.exit.lock().unwrap()
        }
        fn take_output(&mut self) -> (String, usize) {
            let mut chunks = self.chunks.lock().unwrap();
            if chunks.is_empty() {
                (String::new(), 0)
            } else {
                chunks.remove(0)
            }
        }
        fn peek_output(&self) -> String {
            self.chunks
                .lock()
                .unwrap()
                .iter()
                .map(|(t, _)| t.as_str())
                .collect()
        }
        fn wait(&mut self, _cancel: &CancelToken, _timeout: Duration) -> bool {
            self.exit.lock().unwrap().is_some()
        }
        fn stop(&mut self) -> bool {
            *self.stopped.lock().unwrap() = true;
            let mut exit = self.exit.lock().unwrap();
            let was_running = exit.is_none();
            if was_running {
                *exit = Some(143);
            }
            was_running
        }
    }

    fn scripted(pid: i32) -> (Scripted, Exit, Chunks) {
        let exit = Arc::new(Mutex::new(None));
        let chunks = Arc::new(Mutex::new(Vec::new()));
        (
            Scripted {
                pid,
                exit: exit.clone(),
                chunks: chunks.clone(),
                stopped: Arc::new(Mutex::new(false)),
            },
            exit,
            chunks,
        )
    }

    #[test]
    fn ids_count_up_and_the_list_describes_each_state() {
        let mut procs = Procs::default();
        let (a, exit_a, chunks_a) = scripted(100);
        let (b, _exit_b, _) = scripted(200);
        assert_eq!(procs.add("npm run dev", Box::new(a)), "bg-1");
        assert_eq!(procs.add("cargo test", Box::new(b)), "bg-2");
        assert_eq!(procs.len(), 2);
        assert_eq!(procs.running(), 2);
        chunks_a
            .lock()
            .unwrap()
            .push(("listening on 3000\n".to_string(), 0));
        let m = procs.get("bg-1").unwrap();
        assert_eq!(m.peek_tail(5), "listening on 3000");
        assert_eq!(m.take_output(), ("listening on 3000\n".to_string(), 0));
        assert_eq!(m.read_bytes(), 18);
        chunks_a
            .lock()
            .unwrap()
            .push(("GET / 200\nGET /x 404\n".to_string(), 0));
        assert_eq!(m.peek_tail(2), "GET / 200\nGET /x 404");
        assert_eq!(m.peek_tail(10), "listening on 3000\nGET / 200\nGET /x 404");
        assert!(
            m.describe().starts_with("bg-1 running for "),
            "{}",
            m.describe()
        );
        *exit_a.lock().unwrap() = Some(0);
        assert_eq!(procs.get("bg-1").unwrap().exit(), Some(0));
        assert_eq!(procs.running(), 1);
        let lines = procs.describe();
        assert!(lines[0].contains("ended with exit 0 after"), "{}", lines[0]);
        assert!(lines[0].ends_with("pid 100: npm run dev"), "{}", lines[0]);
        assert!(lines[1].contains("running for"), "{}", lines[1]);
        assert!(procs.get("bg-3").is_none());
        // Stopping an ended process is a no-op; stopping a running one ends it.
        assert!(!procs.get("bg-1").unwrap().stop());
        assert!(procs.get("bg-2").unwrap().stop());
        assert_eq!(procs.get("bg-2").unwrap().exit(), Some(143));
        assert!(procs.remove("bg-1").is_some());
        assert_eq!(procs.ids(), vec!["bg-2".to_string()]);
    }

    #[test]
    fn stop_all_reports_only_what_was_still_running_and_clears() {
        let mut procs = Procs::default();
        let (a, exit_a, _) = scripted(100);
        let (b, _, _) = scripted(200);
        procs.add("sleep 300", Box::new(a));
        procs.add("tail -f log", Box::new(b));
        *exit_a.lock().unwrap() = Some(0);
        let stopped = procs.stop_all();
        assert_eq!(stopped, vec!["bg-2 pid 200: tail -f log".to_string()]);
        assert!(procs.is_empty());
        // Ids keep counting after a clear.
        let (c, _, _) = scripted(300);
        assert_eq!(procs.add("x", Box::new(c)), "bg-3");
    }

    #[test]
    fn actions_parse_with_their_ids_and_timeouts() {
        assert_eq!(
            ProcessAction::parse(r#"{"action":"output","id":"bg-1"}"#),
            Ok(ProcessAction::Output { id: "bg-1".into() })
        );
        assert_eq!(
            ProcessAction::parse(r#"{"action":"wait","id":"bg-2","seconds":90}"#),
            Ok(ProcessAction::Wait {
                id: "bg-2".into(),
                timeout: Some(Duration::from_secs(90))
            })
        );
        assert_eq!(
            ProcessAction::parse(r#"{"action":"wait","id":"bg-2"}"#),
            Ok(ProcessAction::Wait {
                id: "bg-2".into(),
                timeout: None
            })
        );
        assert_eq!(
            ProcessAction::parse(r#"{"action":"kill","id":"bg-3"}"#),
            Ok(ProcessAction::Stop { id: "bg-3".into() })
        );
        assert_eq!(
            ProcessAction::parse(r#"{"action":"list"}"#),
            Ok(ProcessAction::List)
        );
        assert_eq!(
            ProcessAction::parse(r#"{"action":"output"}"#).unwrap_err(),
            "action output needs the id bash returned (bg-1)"
        );
        assert!(
            ProcessAction::parse(r#"{"action":"dance"}"#)
                .unwrap_err()
                .starts_with("unknown action dance")
        );
        assert!(ProcessAction::parse("nope").is_err());
        assert!(ProcessAction::parse(r#"{"action":"wait","id":"bg-1","seconds":-1}"#).is_err());
        assert_eq!(
            ProcessAction::Wait {
                id: "bg-1".into(),
                timeout: Some(Duration::from_secs(5))
            }
            .spec(),
            "wait bg-1 5s"
        );
    }

    #[test]
    fn durations_read_well() {
        assert_eq!(fmt_dur(Duration::from_millis(400)), "0.4s");
        assert_eq!(fmt_dur(Duration::from_secs(12)), "12s");
        assert_eq!(fmt_dur(Duration::from_secs(63)), "1m03s");
        assert_eq!(fmt_dur(Duration::from_secs(7500)), "2h05m");
    }
}
