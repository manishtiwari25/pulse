//! `pulse harness chat`: the harness as a conversation. Every line the
//! user types is a turn of one agent that keeps its context; the model's
//! calls that change things are shown for approval before they run; the
//! model may ask back through the `question` tool; a session can be picked
//! up again later with `--resume ID` or `--continue`.

use crate::commands::{
    Prepared, SharedWriter, add_agent_flags, apply_raw_globals, bind_raw_globals, existing_dir,
    exit_code_for, first_set, model_context_window, prepare_agent,
};
use crate::harness::*;
use anyhow::Result;
use pulse_cli::{Context, FlagSet, errorf, write_json};
use pulse_provider::{CancelToken, Provider};
use serde_json::json;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const CHAT_HELP: &str = "\
/help              these commands
/exit              leave the session (also /quit, Ctrl-D, or Ctrl-C twice)
/model [NAME]      show the model, or switch to NAME for the next turns
/compact           summarise the older conversation to free context
/usage             model calls, tokens, cost and time so far
/approve           toggle approving edits and changing commands without asking
/session           the session id and how to pick it up again
A line ending in \\ continues on the next line. Ctrl-C stops the running turn.";

/// How many trajectories `--continue` reads looking for the newest
/// session in this directory.
const CONTINUE_SCAN_LIMIT: usize = 300;

/// What the keyboard sends the session: a line, a ^C while nothing runs,
/// or the end of input.
enum Input {
    Line(String),
    Interrupt,
    Eof,
}

/// Lines from stdin on their own thread and ^C from the signal thread,
/// on one channel; the session waits on it whether it is idle at the
/// prompt or asking the user something mid-turn.
struct Keyboard {
    rx: Mutex<Receiver<Input>>,
}

impl Keyboard {
    /// The next thing the user did. While `cancel` is cancelled (the turn
    /// was interrupted) it returns `Interrupt` instead of waiting on.
    fn read(&self, cancel: Option<&CancelToken>) -> Input {
        let rx = self.rx.lock().expect("keyboard poisoned");
        loop {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(input) => return input,
                Err(RecvTimeoutError::Timeout) => {
                    if cancel.is_some_and(CancelToken::is_cancelled) {
                        return Input::Interrupt;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return Input::Eof,
            }
        }
    }
}

fn spawn_reader(mut input: Box<dyn BufRead + Send>, tx: Sender<Input>) {
    let _ = thread::Builder::new()
        .name("harness-chat-stdin".into())
        .spawn(move || {
            loop {
                let mut line = String::new();
                match input.read_line(&mut line) {
                    Ok(0) | Err(_) => {
                        let _ = tx.send(Input::Eof);
                        return;
                    }
                    Ok(_) => {
                        if tx.send(Input::Line(line)).is_err() {
                            return;
                        }
                    }
                }
            }
        });
}

/// SIGINT while a turn runs cancels it; while the session waits at the
/// prompt it reaches the loop as `Input::Interrupt`. Dropping the guard
/// restores the default disposition.
struct Interrupts {
    handle: signal_hook::iterator::Handle,
    thread: Option<thread::JoinHandle<()>>,
}

impl Interrupts {
    fn watch(tx: Sender<Input>, running: Arc<Mutex<Option<CancelToken>>>) -> Result<Self> {
        let mut signals = signal_hook::iterator::Signals::new([signal_hook::consts::SIGINT])?;
        let handle = signals.handle();
        let thread = thread::Builder::new()
            .name("harness-chat-signals".into())
            .spawn(move || {
                for _ in signals.forever() {
                    let turn = running.lock().expect("running turn poisoned").clone();
                    match turn {
                        Some(cancel) => cancel.cancel(),
                        None => {
                            if tx.send(Input::Interrupt).is_err() {
                                return;
                            }
                        }
                    }
                }
            })?;
        Ok(Self {
            handle,
            thread: Some(thread),
        })
    }
}

impl Drop for Interrupts {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Where the session's lines come from, and whether a person types them.
fn chat_input() -> (Box<dyn BufRead + Send>, bool) {
    #[cfg(test)]
    if let Some(data) = crate::commands::STDIN_FOR_TESTS
        .lock()
        .expect("stdin test hook")
        .clone()
    {
        return (Box::new(io::Cursor::new(data)), false);
    }
    let stdin = io::stdin();
    let tty = stdin.is_terminal();
    (Box::new(io::BufReader::new(stdin)), tty)
}

/// Prints the conversation: the model's words on stdout as they come,
/// what it does (compactly) on stderr.
struct ChatSink {
    out: SharedWriter,
    err: SharedWriter,
    /// Text of the reply being streamed, so the Assistant step that
    /// follows is not printed twice.
    streamed: Arc<Mutex<String>>,
    quiet: bool,
    json: bool,
}

impl Sink for ChatSink {
    fn step(&mut self, step: Step) {
        if step.kind == StepKind::Assistant && step.depth == 0 {
            if self.json {
                return;
            }
            let mut buf = self.streamed.lock().expect("streamed text poisoned");
            if !buf.is_empty() {
                if !buf.ends_with('\n') {
                    let _ = writeln!(self.out);
                }
                buf.clear();
            } else if !step.content.trim().is_empty() {
                let _ = writeln!(self.out, "{}", step.content.trim_end());
            }
            let _ = self.out.flush();
            return;
        }
        if self.quiet {
            return;
        }
        let indent = "  ".repeat(usize::from(step.depth));
        let line = match step.kind {
            StepKind::Assistant | StepKind::Exit => return,
            StepKind::Command => format!("{indent}$ {}", one_line(&step.command, 160)),
            StepKind::Read
            | StepKind::Grep
            | StepKind::Glob
            | StepKind::Edit
            | StepKind::Write
            | StepKind::Mcp
            | StepKind::Skill
            | StepKind::Fetch
            | StepKind::Hook => {
                format!("{indent}{}: {}", step.kind, one_line(&step.command, 160))
            }
            StepKind::Subagent => {
                format!("{indent}subagent: {}", one_line(&step.content, 160))
            }
            StepKind::Plan => step
                .content
                .trim_end()
                .lines()
                .map(|l| format!("{indent}{l}"))
                .collect::<Vec<_>>()
                .join("\n"),
            // A declined call (126) was already shown with its reason.
            StepKind::Observation => match step.exit_code {
                Some(code) if code != 0 && code != 126 => {
                    format!("{indent}  exit {code}: {}", one_line(&step.content, 160))
                }
                _ => return,
            },
            StepKind::Note => format!("{indent}note: {}", one_line(&step.content, 200)),
            StepKind::FormatRetry => {
                format!("{indent}format retry: {}", one_line(&step.content, 160))
            }
            StepKind::Error => format!("{indent}error: {}", one_line(&step.content, 200)),
        };
        let _ = writeln!(self.err, "{line}");
        let _ = self.err.flush();
    }
}

/// The person at the keyboard: approvals and answers over stderr and the
/// shared keyboard. Without a terminal (input piped in) nothing can be
/// asked: changes are declined unless `--yes` allowed them, questions go
/// unanswered.
struct ChatUi {
    keyboard: Arc<Keyboard>,
    err: SharedWriter,
    /// Approve without asking (`--yes`, `/approve`, or an `always` answer).
    auto: Arc<AtomicBool>,
    tty: bool,
    quiet: bool,
    /// Set when stdin closed while the model was asking: the session ends
    /// after the turn.
    closed: Arc<AtomicBool>,
    /// The running turn's token: ^C during a prompt cancels it.
    running: Arc<Mutex<Option<CancelToken>>>,
}

impl ChatUi {
    fn current_cancel(&self) -> Option<CancelToken> {
        self.running.lock().expect("running turn poisoned").clone()
    }

    /// One line from the user, None when nobody can answer.
    fn answer(&mut self) -> Option<String> {
        let cancel = self.current_cancel();
        match self.keyboard.read(cancel.as_ref()) {
            Input::Line(line) => Some(line.trim().to_string()),
            Input::Interrupt => {
                if let Some(c) = cancel {
                    c.cancel();
                }
                None
            }
            Input::Eof => {
                self.closed.store(true, Ordering::SeqCst);
                None
            }
        }
    }
}

impl Ui for ChatUi {
    fn approve(&mut self, req: &ApprovalRequest) -> Approval {
        if self.auto.load(Ordering::SeqCst) {
            if !self.quiet {
                let _ = writeln!(
                    self.err,
                    "allowed without asking: {} {}",
                    req.tool,
                    one_line(&req.summary, 160)
                );
            }
            return Approval::Allow;
        }
        if !self.tty {
            let _ = writeln!(
                self.err,
                "declined (no terminal to ask): {} {}; run pulse harness chat --yes to allow changes",
                req.tool,
                one_line(&req.summary, 160)
            );
            return Approval::Deny(
                "no terminal to approve it; the user runs without --yes".to_string(),
            );
        }
        let _ = writeln!(self.err, "\napprove {}: {}", req.tool, req.summary);
        if !req.reason.is_empty() {
            let _ = writeln!(self.err, "  why: {}", req.reason);
        }
        for line in req.detail.lines().take(40) {
            let _ = writeln!(self.err, "  {line}");
        }
        if req.detail.lines().count() > 40 {
            let _ = writeln!(self.err, "  ...");
        }
        let _ = write!(
            self.err,
            "  [y]es, [n]o, [a]lways, or a reason to decline: "
        );
        let _ = self.err.flush();
        let Some(answer) = self.answer() else {
            let _ = writeln!(self.err);
            return Approval::Deny("the user did not answer (interrupted or input closed)".into());
        };
        match answer.to_lowercase().as_str() {
            "" | "y" | "yes" => Approval::Allow,
            "a" | "always" => {
                self.auto.store(true, Ordering::SeqCst);
                let _ = writeln!(
                    self.err,
                    "  approving changes without asking from now on (/approve turns it off)"
                );
                Approval::Allow
            }
            "n" | "no" => Approval::Deny(String::new()),
            _ => Approval::Deny(answer),
        }
    }

    fn ask(&mut self, question: &str, choices: &[String]) -> Option<String> {
        let _ = writeln!(self.err, "\nquestion: {}", question.trim());
        for (i, choice) in choices.iter().enumerate() {
            let _ = writeln!(self.err, "  {}) {}", i + 1, choice);
        }
        if !self.tty {
            let _ = writeln!(self.err, "  (no terminal to answer; the model decides)");
            return None;
        }
        let _ = write!(self.err, "  answer (a number or your words): ");
        let _ = self.err.flush();
        let answer = self.answer()?;
        if answer.is_empty() {
            return None;
        }
        if let Ok(n) = answer.parse::<usize>()
            && n >= 1
            && n <= choices.len()
        {
            return Some(choices[n - 1].clone());
        }
        Some(answer)
    }
}

/// An earlier session to carry on, from `--resume ID` or `--continue`.
fn pick_resume(c: &mut Context, fs: &FlagSet, cwd: &str) -> Result<Option<(String, Trajectory)>> {
    let id = fs.get_string("resume");
    let cont = fs.get_bool("continue");
    if !id.is_empty() && cont {
        return Err(errorf(
            2,
            "pulse harness chat: pass one of --resume ID and --continue",
            "",
            "",
        )
        .into());
    }
    let dir = Path::new(&c.home).join("harness");
    if !id.is_empty() {
        let path = if id.ends_with(".json") || id.contains('/') {
            PathBuf::from(&id)
        } else {
            dir.join(format!("{id}.json"))
        };
        let traj = read_trajectory(&path).map_err(|e| {
            errorf(
                2,
                &format!("pulse harness chat: --resume {id}: {e}"),
                "",
                "pulse harness sessions lists sessions; a session's trajectory is ~/.pulse/harness/ID.json",
            )
        })?;
        if traj.messages.is_empty() {
            return Err(errorf(
                2,
                &format!(
                    "pulse harness chat: --resume {id}: the trajectory has no messages to carry on"
                ),
                "",
                "",
            )
            .into());
        }
        if !traj.info.config.cwd.is_empty() && traj.info.config.cwd != cwd && !c.quiet {
            writeln!(
                c.err,
                "pulse harness chat: session {id} ran in {}; this one runs in {cwd}",
                traj.info.config.cwd
            )?;
        }
        return Ok(Some((id, traj)));
    }
    if cont {
        if let Some(found) = newest_session_in(&dir, cwd) {
            return Ok(Some(found));
        }
        return Err(errorf(
            2,
            &format!("pulse harness chat: no earlier session in {cwd} to continue"),
            "",
            "start one without --continue, or pass --resume ID",
        )
        .into());
    }
    Ok(None)
}

fn read_trajectory(path: &Path) -> Result<Trajectory> {
    let data = std::fs::read(path)?;
    Ok(serde_json::from_slice(&data)?)
}

/// The newest interactive session that ran in `cwd`, by trajectory
/// modification time.
fn newest_session_in(dir: &Path, cwd: &str) -> Option<(String, Trajectory)> {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension().is_some_and(|x| x == "json") {
                let modified = e.metadata().ok()?.modified().ok()?;
                Some((modified, path))
            } else {
                None
            }
        })
        .collect();
    files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    for (_, path) in files.into_iter().take(CONTINUE_SCAN_LIMIT) {
        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        // Job trajectories never carry the flag; skip them without parsing.
        if !data.windows(13).any(|w| w == b"\"interactive\"") {
            continue;
        }
        let Ok(traj) = serde_json::from_slice::<Trajectory>(&data) else {
            continue;
        };
        if traj.info.config.interactive && traj.info.config.cwd == cwd && !traj.messages.is_empty()
        {
            let id = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            return Some((id, traj));
        }
    }
    None
}

/// Reads one message: a line, continued while it ends in a backslash.
fn read_message(keyboard: &Keyboard, err: &mut SharedWriter, tty: bool) -> Input {
    let mut text = String::new();
    loop {
        match keyboard.read(None) {
            Input::Line(line) => {
                let line = line.trim_end_matches(['\n', '\r']);
                if let Some(more) = line.strip_suffix('\\') {
                    text.push_str(more);
                    text.push('\n');
                    if tty {
                        let _ = write!(err, "...> ");
                        let _ = err.flush();
                    }
                    continue;
                }
                text.push_str(line);
                return Input::Line(text);
            }
            Input::Interrupt if text.is_empty() => return Input::Interrupt,
            Input::Interrupt => {
                text.clear();
                return Input::Line(String::new());
            }
            Input::Eof if text.trim().is_empty() => return Input::Eof,
            Input::Eof => return Input::Line(text),
        }
    }
}

enum Slash {
    Done,
    Exit,
}

struct Session<'a> {
    agent: Arc<Mutex<Agent>>,
    provider: Arc<dyn Provider>,
    session_id: &'a str,
    trajectory_path: &'a str,
    err: SharedWriter,
    auto: Arc<AtomicBool>,
    running: Arc<Mutex<Option<CancelToken>>>,
    start: Instant,
    turns: i32,
}

impl Session<'_> {
    fn usage_line(&self) -> String {
        let agent = self.agent.lock().expect("agent poisoned");
        let r = agent.result();
        let elapsed = self.start.elapsed().as_secs();
        let mut line = format!(
            "{} turns, {} model calls, {} tool steps, {} prompt + {} completion tokens",
            self.turns, r.calls, r.steps, r.usage.prompt_tokens, r.usage.completion_tokens
        );
        if r.usage.cost > 0.0 {
            line.push_str(&format!(", cost {:.4}", r.usage.cost));
        }
        line.push_str(&format!(
            ", {:02}:{:02} elapsed; model {}; session {}",
            elapsed / 60,
            elapsed % 60,
            agent.cfg.model,
            self.session_id
        ));
        line
    }

    fn slash(&mut self, command: &str) -> Slash {
        let mut parts = command.splitn(2, char::is_whitespace);
        let name = parts.next().unwrap_or("").to_lowercase();
        let arg = parts.next().unwrap_or("").trim().to_string();
        let line = match name.as_str() {
            "help" | "?" => CHAT_HELP.to_string(),
            "exit" | "quit" | "q" => return Slash::Exit,
            "model" => {
                if arg.is_empty() {
                    format!(
                        "model: {}",
                        self.agent.lock().expect("agent poisoned").cfg.model
                    )
                } else {
                    let cancel = CancelToken::new();
                    *self.running.lock().expect("running turn poisoned") = Some(cancel.clone());
                    let window = model_context_window(&cancel, self.provider.as_ref(), &arg);
                    let listed = self
                        .provider
                        .models(&cancel)
                        .map(|ms| ms.iter().any(|m| m.id == arg))
                        .unwrap_or(true);
                    *self.running.lock().expect("running turn poisoned") = None;
                    self.agent
                        .lock()
                        .expect("agent poisoned")
                        .set_model(&arg, window);
                    if listed {
                        format!("model: {arg} from the next turn")
                    } else {
                        format!(
                            "model: {arg} from the next turn (not in the provider's model list; trying it anyway)"
                        )
                    }
                }
            }
            "compact" => {
                let cancel = CancelToken::new();
                *self.running.lock().expect("running turn poisoned") = Some(cancel.clone());
                let out = self
                    .agent
                    .lock()
                    .expect("agent poisoned")
                    .compact_now(&cancel);
                *self.running.lock().expect("running turn poisoned") = None;
                match out {
                    Some(true) => "compacted the older conversation into a summary".to_string(),
                    Some(false) => "the model gave no summary; nothing compacted".to_string(),
                    None => "nothing to compact yet".to_string(),
                }
            }
            "usage" | "status" => self.usage_line(),
            "approve" => {
                let on = !self.auto.load(Ordering::SeqCst);
                self.auto.store(on, Ordering::SeqCst);
                if on {
                    "approving edits and changing commands without asking: on".to_string()
                } else {
                    "approving edits and changing commands without asking: off".to_string()
                }
            }
            "session" => {
                let mut s = format!("session {}", self.session_id);
                if !self.trajectory_path.is_empty() {
                    s.push_str(&format!("; trajectory {}", self.trajectory_path));
                }
                s.push_str(&format!(
                    "\npick it up again with: pulse harness chat --resume {}",
                    self.session_id
                ));
                s
            }
            _ => format!("unknown command /{name}; /help lists them"),
        };
        let _ = writeln!(self.err, "{line}");
        let _ = self.err.flush();
        Slash::Done
    }
}

pub(crate) fn harness_chat(c: &mut Context, args: &[String]) -> Result<()> {
    let mut fs = FlagSet::new("harness chat");
    fs.set_output(pulse_cli::flags::Output::Discard);
    fs.string_var(
        "p",
        "",
        "the first message; the session then reads the next ones",
    );
    fs.string_var("task", "", "the first message");
    fs.bool_var(
        "yes",
        false,
        "allow edits and changing commands without asking (/approve toggles it in the session)",
    );
    fs.string_var(
        "resume",
        "",
        "carry on session ID (its trajectory under ~/.pulse/harness) in a new session",
    );
    fs.bool_var(
        "continue",
        false,
        "carry on the newest session that ran in this directory",
    );
    add_agent_flags(&mut fs);
    bind_raw_globals(&mut fs);
    fs.parse(args)?;
    apply_raw_globals(c, &fs);
    if fs.narg() != 0 {
        return Err(errorf(
            2,
            &format!(
                "pulse harness chat: unrecognized arguments: {}",
                fs.args().join(" ")
            ),
            "",
            "",
        )
        .into());
    }
    let first = first_set(&fs, "p", "task");
    let cwd = existing_dir(&fs.get_string("C"))?;
    let resume = pick_resume(c, &fs, &cwd)?;

    let out = SharedWriter::new(std::mem::replace(&mut c.out, Box::new(io::sink())));
    c.out = Box::new(out.clone());
    let err = SharedWriter::new(std::mem::replace(&mut c.err, Box::new(io::sink())));
    c.err = Box::new(err.clone());
    let streamed = Arc::new(Mutex::new(String::new()));
    let sink = ChatSink {
        out: out.clone(),
        err: err.clone(),
        streamed: streamed.clone(),
        quiet: c.quiet,
        json: c.json,
    };
    let mut prepared: Prepared = prepare_agent(
        c,
        &fs,
        "chat",
        String::new(),
        true,
        vec![Box::new(sink)],
        false,
    )?;
    let agent = prepared.shared.clone();
    let quiet = c.quiet;
    let json = c.json;

    if let Some((label, traj)) = resume {
        let n = traj
            .messages
            .iter()
            .filter(|m| !matches!(m.role, pulse_provider::Role::System))
            .count();
        agent
            .lock()
            .expect("agent poisoned")
            .resume_messages(traj.messages)?;
        if !quiet {
            writeln!(
                err.clone(),
                "resumed session {label}: {n} messages carried over"
            )?;
        }
    }

    let running: Arc<Mutex<Option<CancelToken>>> = Arc::new(Mutex::new(None));
    let auto = Arc::new(AtomicBool::new(fs.get_bool("yes")));
    let closed = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let (input, tty) = chat_input();
    spawn_reader(input, tx.clone());
    let interrupts = Interrupts::watch(tx, running.clone());
    if let Err(e) = &interrupts
        && !quiet
    {
        writeln!(
            err.clone(),
            "pulse harness chat: Ctrl-C handling is off: {e}"
        )?;
    }
    let keyboard = Arc::new(Keyboard { rx: Mutex::new(rx) });
    let ui = ChatUi {
        keyboard: keyboard.clone(),
        err: err.clone(),
        auto: auto.clone(),
        tty,
        quiet,
        closed: closed.clone(),
        running: running.clone(),
    };
    {
        let mut a = agent.lock().expect("agent poisoned");
        a.set_ui(Arc::new(Mutex::new(ui)));
        if !json {
            let streamed = streamed.clone();
            let mut out = out.clone();
            a.set_stream(Box::new(move |text| {
                streamed
                    .lock()
                    .expect("streamed text poisoned")
                    .push_str(text);
                let _ = out.write_all(text.as_bytes());
                let _ = out.flush();
            }));
        }
    }
    if !quiet {
        let mut e = err.clone();
        writeln!(
            e,
            "pulse harness chat: {} {} in {} (session {})",
            prepared.provider_name, prepared.model_name, prepared.cwd, prepared.session_id
        )?;
        writeln!(
            e,
            "{}",
            if auto.load(Ordering::SeqCst) {
                "edits and changing commands run without asking (--yes); /help lists the commands, /exit leaves"
            } else {
                "edits and changing commands are shown for approval first; /help lists the commands, /exit leaves"
            }
        )?;
    }

    let session_id = prepared.session_id.clone();
    let trajectory_path = prepared.trajectory_path.clone();
    let mut session = Session {
        agent: agent.clone(),
        provider: prepared.provider.clone(),
        session_id: &session_id,
        trajectory_path: &trajectory_path,
        err: err.clone(),
        auto: auto.clone(),
        running: running.clone(),
        start: Instant::now(),
        turns: 0,
    };
    let mut pending = (!first.trim().is_empty()).then_some(first);
    let mut idle_interrupts = 0;
    let mut exit_code = 0;
    let mut end_status = EXIT_ENDED.to_string();
    let mut e = err.clone();
    loop {
        let text = match pending.take() {
            Some(t) => t,
            None => {
                if closed.load(Ordering::SeqCst) {
                    break;
                }
                if tty && !quiet {
                    write!(e, "you> ")?;
                    e.flush()?;
                }
                match read_message(&keyboard, &mut e, tty) {
                    Input::Eof => {
                        if tty && !quiet {
                            writeln!(e)?;
                        }
                        break;
                    }
                    Input::Interrupt => {
                        idle_interrupts += 1;
                        if idle_interrupts >= 2 {
                            exit_code = 130;
                            break;
                        }
                        if !quiet {
                            writeln!(e, "\n(Ctrl-C again to leave, or /exit)")?;
                        }
                        continue;
                    }
                    Input::Line(t) => t,
                }
            }
        };
        idle_interrupts = 0;
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if !tty && !quiet {
            writeln!(e, "you> {}", one_line(text, 200))?;
        }
        if let Some(command) = text.strip_prefix('/') {
            match session.slash(command) {
                Slash::Done => continue,
                Slash::Exit => break,
            }
        }
        let cancel = CancelToken::new();
        *running.lock().expect("running turn poisoned") = Some(cancel.clone());
        let stop_watch = prepared.watch_cancel(&cancel);
        let outcome = agent.lock().expect("agent poisoned").turn(&cancel, text);
        *running.lock().expect("running turn poisoned") = None;
        if let Some(stop) = stop_watch {
            stop();
        }
        session.turns += 1;
        match outcome {
            Ok(Turn::Reply(reply)) => {
                if json {
                    write_json(
                        &mut c.out,
                        &json!({
                            "session": session_id,
                            "turn": session.turns,
                            "reply": reply,
                            "calls": agent.lock().expect("agent poisoned").result().calls,
                        }),
                    )?;
                }
            }
            Ok(Turn::Stopped { status, error }) => {
                if json {
                    write_json(
                        &mut c.out,
                        &json!({
                            "session": session_id,
                            "turn": session.turns,
                            "status": status,
                            "error": error,
                        }),
                    )?;
                } else if quiet {
                    writeln!(e, "turn stopped: {status}: {error}")?;
                }
            }
            Ok(Turn::Ended(result)) => {
                writeln!(e, "session ended: {}: {}", result.exit_status, result.error)?;
                end_status = result.exit_status.clone();
                exit_code = exit_code_for(&result.exit_status);
                prepared.write_trajectory(c)?;
                break;
            }
            Err(error) => {
                writeln!(e, "pulse harness chat: {error}")?;
                if agent.lock().expect("agent poisoned").is_ended() {
                    exit_code = 1;
                    break;
                }
            }
        }
        prepared.write_trajectory(c)?;
    }
    drop(interrupts);
    let result = agent.lock().expect("agent poisoned").end(&end_status);
    prepared.write_trajectory(c)?;
    prepared.finish(c, &result)?;
    if json {
        write_json(
            &mut c.out,
            &json!({
                "session": session_id,
                "turns": session.turns,
                "status": result.exit_status,
                "calls": result.calls,
                "steps": result.steps,
                "usage": result.usage,
                "trajectory": trajectory_path,
            }),
        )?;
    } else if !quiet {
        writeln!(e, "{}", session.usage_line())?;
        if session.turns > 0 {
            writeln!(
                e,
                "pick the session up again with: pulse harness chat --resume {session_id}"
            )?;
        }
    }
    if exit_code != 0 {
        return Err(errorf(exit_code, "", "", "").into());
    }
    Ok(())
}
