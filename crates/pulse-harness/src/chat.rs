//! `pulse harness chat`: the harness as a conversation. Every line the
//! user types is a turn of one agent that keeps its context; the model's
//! calls that change things are shown for approval before they run; the
//! model may ask back through the `question` tool; a session can be picked
//! up again later with `--resume ID` or `--continue`. At a terminal the
//! line is edited with history and Tab completion; `@PATH` attaches a
//! file, `!CMD` runs a shell command, `/commands` steer the session.

use crate::commands::{
    Prepared, SharedWriter, add_agent_flags, apply_raw_globals, bind_raw_globals, existing_dir,
    exit_code_for, first_set, model_context_window, prepare_agent,
};
use crate::harness::*;
use crate::memory::Scope;
use anyhow::Result;
use pulse_cli::{Context, FlagSet, errorf, write_json};
use pulse_provider::{CancelToken, Message, Provider, Role};
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::FileHistory;
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::{Behavior, CompletionType, Config as LineConfig, Editor, Helper};
use serde_json::json;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

/// The session's commands: name, argument hint, what it does. `/help`
/// lists them and Tab completes them.
const SLASH_COMMANDS: &[(&str, &str, &str)] = &[
    ("help", "", "these commands"),
    (
        "exit",
        "",
        "leave the session (also /quit, Ctrl-D, or Ctrl-C twice)",
    ),
    (
        "new",
        "",
        "start over with a fresh conversation in a new session (also /clear)",
    ),
    (
        "resume",
        "ID",
        "carry on an earlier session in place of this conversation",
    ),
    ("sessions", "", "recent sessions that ran in this directory"),
    (
        "model",
        "[NAME]",
        "show the model, or switch to NAME for the next turns",
    ),
    (
        "plan",
        "[on|off]",
        "plan mode: the model explores and plans but changes nothing",
    ),
    (
        "compact",
        "",
        "summarise the older conversation to free context",
    ),
    (
        "usage",
        "",
        "model calls, tokens, cost and time so far (also /status)",
    ),
    (
        "approve",
        "",
        "toggle approving edits and changing commands without asking",
    ),
    (
        "permissions",
        "[allow|deny PATTERN | clear]",
        "the permission rules in force; add one (bash(git *), edit(src/*), write) or drop the session's",
    ),
    (
        "procs",
        "[ID [LINES] | stop ID|all]",
        "the background processes the model started; ID shows the tail of one's output, stop ends one or all",
    ),
    (
        "memory",
        "[add [user] TEXT | forget TEXT | clear project|user]",
        "what is remembered for this project and for you across sessions; add one, forget one, or clear a file",
    ),
    (
        "diff",
        "",
        "what changed in the working tree (git status and diff --stat)",
    ),
    (
        "export",
        "[PATH]",
        "write the conversation as markdown (default ~/.pulse/harness/ID.md)",
    ),
    (
        "env",
        "",
        "what the session runs with: model, tools, instructions, skills, hooks, MCP",
    ),
    (
        "init",
        "",
        "have the model write or improve AGENTS.md for this repository",
    ),
    (
        "delegate",
        "[TITLE]",
        "hand the work discussed here to the fleet as a task on the board",
    ),
    ("session", "", "the session id and how to pick it up again"),
];

const CHAT_HELP_TAIL: &str = "\
@PATH in a message attaches the file (a directory: its listing); !CMD runs a shell command and the model sees its output.
A line ending in \\ continues on the next line; Tab completes /commands and @paths; Ctrl-C stops the running turn.";

/// How many trajectories `--continue` and `/sessions` read looking for
/// sessions in this directory.
const CONTINUE_SCAN_LIMIT: usize = 300;
/// Sessions `/sessions` lists.
const SESSIONS_LISTED: usize = 10;
/// Bytes of a mentioned file attached to a message.
const MENTION_LIMIT: usize = 48 * 1024;
/// Entries of a mentioned directory listed.
const LISTING_LIMIT: usize = 200;
/// Characters of `!CMD` output kept.
const SHELL_OUTPUT_LIMIT: usize = 24 * 1024;
/// Path candidates offered for one completion.
const COMPLETION_LIMIT: usize = 200;
/// Lines of input history kept in ~/.pulse/harness/chat_history.
const HISTORY_LIMIT: usize = 2000;
const HISTORY_FILE: &str = "chat_history";
/// A prompt that comes this long after the user last typed rings the
/// terminal bell: they have probably looked away.
const ATTENTION_AFTER: Duration = Duration::from_secs(10);
/// Characters of a tool result kept in an exported transcript.
const EXPORT_TOOL_LIMIT: usize = 4000;

fn chat_help() -> String {
    let mut out = String::new();
    for (name, hint, what) in SLASH_COMMANDS {
        let head = if hint.is_empty() {
            format!("/{name}")
        } else {
            format!("/{name} {hint}")
        };
        out.push_str(&format!("{head:<19}{what}\n"));
    }
    out.push_str(CHAT_HELP_TAIL);
    out
}

/// What the keyboard sends the session: a line, a ^C typed at a prompt,
/// a SIGINT that arrived while nothing ran, or the end of input.
enum Input {
    Line(String),
    Interrupt,
    Signal,
    Eof,
}

/// One line wanted from the user: the prompt to show, whether the answer
/// belongs in the input history, and whether to show it bold.
struct Ask {
    prompt: String,
    history: bool,
    bold: bool,
}

/// The session's side of the keyboard: it asks the prompter thread for a
/// line and waits for the answer, ^C from the signal thread arriving on
/// the same channel. Reading on demand keeps the line editor idle while a
/// turn runs, so the terminal is cooked and ^C reaches the signal thread.
struct Keyboard {
    asks: Sender<Ask>,
    rx: Mutex<Receiver<Input>>,
    /// The line editor drives the terminal: ^C at a prompt is a key it
    /// reports, so a SIGINT between prompts (one that slipped in before
    /// the editor took the terminal back) is nobody's and is ignored.
    editor: bool,
}

impl Keyboard {
    /// The next thing the user did. While `cancel` is cancelled (the turn
    /// was interrupted) it returns `Interrupt` instead of waiting on.
    fn read(&self, ask: Ask, cancel: Option<&CancelToken>) -> Input {
        let rx = self.rx.lock().expect("keyboard poisoned");
        // Whatever arrived since the last ask was for nobody: a ^C while a
        // shell command ran, an answer to a prompt the turn gave up on.
        while rx.try_recv().is_ok() {}
        if self.asks.send(ask).is_err() {
            return Input::Eof;
        }
        loop {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(Input::Signal) if self.editor => {}
                Ok(Input::Signal) => return Input::Interrupt,
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

/// Where lines come from: a terminal the line editor drives (history,
/// completion, multi-line input), or lines read one per ask from a pipe,
/// a file, or a terminal the editor could not open.
enum Prompter {
    Terminal {
        editor: Box<Editor<ChatHelper, FileHistory>>,
        history: Option<PathBuf>,
    },
    Lines {
        input: Box<dyn BufRead + Send>,
        /// Show the prompts (a terminal without the editor); piped input
        /// gets none.
        show: bool,
    },
}

/// Opens the keyboard: the editor when stdin is a terminal, plain lines
/// otherwise. The flag says whether a person is there to answer.
fn open_prompter(cwd: &str, home: &str, err: &mut SharedWriter, quiet: bool) -> (Prompter, bool) {
    #[cfg(test)]
    if let Some(data) = crate::commands::STDIN_FOR_TESTS
        .lock()
        .expect("stdin test hook")
        .clone()
    {
        return (
            Prompter::Lines {
                input: Box::new(io::Cursor::new(data)),
                show: false,
            },
            false,
        );
    }
    let stdin = io::stdin();
    if !stdin.is_terminal() {
        return (
            Prompter::Lines {
                input: Box::new(io::BufReader::new(stdin)),
                show: false,
            },
            false,
        );
    }
    match line_editor(cwd, home) {
        Ok((editor, history)) => (
            Prompter::Terminal {
                editor: Box::new(editor),
                history,
            },
            true,
        ),
        Err(e) => {
            if !quiet {
                let _ = writeln!(err, "pulse harness chat: line editing is off: {e}");
            }
            (
                Prompter::Lines {
                    input: Box::new(io::BufReader::new(stdin)),
                    show: true,
                },
                true,
            )
        }
    }
}

/// The line editor on /dev/tty (so stdout stays the model's words), with
/// the input history under the harness directory.
///
/// rustyline is built without its `signal-hook` feature on purpose: that
/// path waits on the terminal with `select` and never looks at the bytes
/// it already buffered, so anything typed while a turn ran would stall
/// after its first character. Without it rustyline swaps the SIGINT
/// handler only for the length of a readline call, where Ctrl-C is a key
/// anyway, and restores ours afterwards.
fn line_editor(
    cwd: &str,
    home: &str,
) -> rustyline::Result<(Editor<ChatHelper, FileHistory>, Option<PathBuf>)> {
    let config = LineConfig::builder()
        .behavior(Behavior::PreferTerm)
        .max_history_size(HISTORY_LIMIT)?
        .history_ignore_dups(true)?
        .history_ignore_space(true)
        .completion_type(CompletionType::List)
        .auto_add_history(false)
        .build();
    let mut editor = Editor::<ChatHelper, FileHistory>::with_config(config)?;
    editor.set_helper(Some(ChatHelper {
        cwd: PathBuf::from(cwd),
        home: std::env::var_os("HOME").map(PathBuf::from),
    }));
    let dir = Path::new(home).join("harness");
    let history = std::fs::create_dir_all(&dir)
        .is_ok()
        .then(|| dir.join(HISTORY_FILE));
    if let Some(path) = &history
        && path.exists()
    {
        let _ = editor.load_history(path);
    }
    Ok((editor, history))
}

fn spawn_prompter(
    mut prompter: Prompter,
    asks: Receiver<Ask>,
    tx: Sender<Input>,
    mut err: SharedWriter,
) {
    let _ = thread::Builder::new()
        .name("harness-chat-prompt".into())
        .spawn(move || {
            for ask in asks {
                let input = match &mut prompter {
                    Prompter::Terminal { editor, history } => {
                        let styled = if ask.bold {
                            format!("\x1b[1m{}\x1b[0m", ask.prompt)
                        } else {
                            ask.prompt.clone()
                        };
                        match editor.readline(&(ask.prompt.as_str(), styled.as_str())) {
                            Ok(line) => {
                                if ask.history
                                    && !line.trim().is_empty()
                                    && editor.add_history_entry(line.as_str()).unwrap_or(false)
                                    && let Some(path) = history
                                {
                                    let _ = editor.append_history(path);
                                }
                                Input::Line(unescape_continuations(&line))
                            }
                            Err(ReadlineError::Interrupted)
                            | Err(ReadlineError::Signal(rustyline::error::Signal::Interrupt)) => {
                                Input::Interrupt
                            }
                            Err(_) => Input::Eof,
                        }
                    }
                    Prompter::Lines { input, show } => {
                        if *show && !ask.prompt.is_empty() {
                            let _ = write!(err, "{}", ask.prompt);
                            let _ = err.flush();
                        }
                        read_lines(input.as_mut(), *show, &mut err)
                    }
                };
                let eof = matches!(input, Input::Eof);
                if tx.send(input).is_err() || eof {
                    return;
                }
            }
        });
}

/// Reads one message from piped input: a line, continued while it ends
/// in a backslash.
fn read_lines(input: &mut dyn BufRead, show: bool, err: &mut SharedWriter) -> Input {
    let mut text = String::new();
    loop {
        let mut line = String::new();
        match input.read_line(&mut line) {
            Ok(0) | Err(_) => {
                return if text.trim().is_empty() {
                    Input::Eof
                } else {
                    Input::Line(text)
                };
            }
            Ok(_) => {
                let line = line.trim_end_matches(['\n', '\r']);
                if continues(line) {
                    text.push_str(&line[..line.len() - 1]);
                    text.push('\n');
                    if show {
                        let _ = write!(err, "...> ");
                        let _ = err.flush();
                    }
                    continue;
                }
                text.push_str(line);
                return Input::Line(text);
            }
        }
    }
}

/// Whether the line asks for another: it ends in a backslash that is not
/// itself escaped.
fn continues(text: &str) -> bool {
    text.chars().rev().take_while(|c| *c == '\\').count() % 2 == 1
}

/// The editor keeps the continuation backslashes in a multi-line entry;
/// the message does not need them.
fn unescape_continuations(text: &str) -> String {
    text.replace("\\\n", "\n")
}

/// Completion and multi-line input for the line editor: `/` at the start
/// completes the commands, `@` completes paths under the working
/// directory, a trailing backslash asks for another line.
struct ChatHelper {
    cwd: PathBuf,
    home: Option<PathBuf>,
}

impl Helper for ChatHelper {}

impl Hinter for ChatHelper {
    type Hint = String;
}

impl Highlighter for ChatHelper {}

impl Validator for ChatHelper {
    fn validate(&self, ctx: &mut ValidationContext) -> rustyline::Result<ValidationResult> {
        Ok(if continues(ctx.input()) {
            ValidationResult::Incomplete
        } else {
            ValidationResult::Valid(None)
        })
    }
}

impl Completer for ChatHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let (start, found) = complete_line(line, pos, &self.cwd, self.home.as_deref());
        Ok((
            start,
            found
                .into_iter()
                .map(|(display, replacement)| Pair {
                    display,
                    replacement,
                })
                .collect(),
        ))
    }
}

/// Candidates for the text before `pos`: `(display, replacement)` pairs
/// replacing the line from the returned offset.
fn complete_line(
    line: &str,
    pos: usize,
    cwd: &Path,
    home: Option<&Path>,
) -> (usize, Vec<(String, String)>) {
    let before = &line[..pos];
    if let Some(partial) = before.strip_prefix('/')
        && !before.contains(char::is_whitespace)
    {
        let found = SLASH_COMMANDS
            .iter()
            .filter(|(name, _, _)| name.starts_with(partial))
            .map(|(name, hint, _)| {
                let display = if hint.is_empty() {
                    format!("/{name}")
                } else {
                    format!("/{name} {hint}")
                };
                (display, format!("/{name} "))
            })
            .collect();
        return (0, found);
    }
    let start = before
        .char_indices()
        .rev()
        .take_while(|(_, c)| !c.is_whitespace())
        .last()
        .map_or(pos, |(i, _)| i);
    if let Some(partial) = before[start..].strip_prefix('@') {
        let found = complete_path(partial, cwd, home)
            .into_iter()
            .map(|path| {
                let replacement = if path.ends_with('/') {
                    format!("@{path}")
                } else {
                    format!("@{path} ")
                };
                (path, replacement)
            })
            .collect();
        return (start, found);
    }
    (pos, Vec::new())
}

/// Paths starting with `partial`, relative to `cwd` (`~/` is the home
/// directory); directories end in `/`; hidden entries only when asked
/// for with a dot.
fn complete_path(partial: &str, cwd: &Path, home: Option<&Path>) -> Vec<String> {
    let (dir_part, name_part) = match partial.rfind('/') {
        Some(i) => (&partial[..=i], &partial[i + 1..]),
        None => ("", partial),
    };
    let dir = if let Some(rest) = dir_part.strip_prefix("~/") {
        match home {
            Some(h) => h.join(rest),
            None => return Vec::new(),
        }
    } else if dir_part.starts_with('/') {
        PathBuf::from(dir_part)
    } else {
        cwd.join(dir_part)
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut found: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with(name_part)
                || (name.starts_with('.') && !name_part.starts_with('.'))
            {
                return None;
            }
            let slash = if e.path().is_dir() { "/" } else { "" };
            Some(format!("{dir_part}{name}{slash}"))
        })
        .collect();
    found.sort();
    found.truncate(COMPLETION_LIMIT);
    found
}

/// `@PATH` tokens in a message attach the file (a directory: its listing)
/// under the text, so the model has them without a read. A token that
/// names nothing stays as written.
fn expand_mentions(text: &str, cwd: &Path, home: Option<&Path>) -> (String, Vec<String>) {
    let mut attached: Vec<String> = Vec::new();
    let mut blocks = String::new();
    for token in text.split_whitespace() {
        let Some(raw) = token.strip_prefix('@') else {
            continue;
        };
        let name = raw.trim_end_matches([',', ';', ':', ')', '?', '!', '"', '\'']);
        let name = name.strip_suffix('.').unwrap_or(name);
        if name.is_empty() || attached.iter().any(|a| a == name) {
            continue;
        }
        let path = if name.starts_with('/') {
            PathBuf::from(name)
        } else if let Some(rest) = name.strip_prefix("~/") {
            match home {
                Some(h) => h.join(rest),
                None => continue,
            }
        } else {
            cwd.join(name)
        };
        let Some(block) = mention_block(name, &path) else {
            continue;
        };
        blocks.push_str(&block);
        attached.push(name.to_string());
    }
    if attached.is_empty() {
        return (text.to_string(), attached);
    }
    (format!("{text}\n\n{}", blocks.trim_end()), attached)
}

fn mention_block(name: &str, path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.is_dir() {
        let mut names: Vec<String> = std::fs::read_dir(path)
            .ok()?
            .flatten()
            .map(|e| {
                let mut n = e.file_name().to_string_lossy().to_string();
                if e.path().is_dir() {
                    n.push('/');
                }
                n
            })
            .collect();
        names.sort();
        let total = names.len();
        names.truncate(LISTING_LIMIT);
        let mut block = format!(
            "--- @{name} (directory, {total} entries) ---\n{}\n",
            names.join("\n")
        );
        if total > LISTING_LIMIT {
            block.push_str("...\n");
        }
        return Some(block);
    }
    if !meta.is_file() {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    if data.contains(&0) {
        return Some(format!(
            "--- @{name} ({} bytes, binary; not attached) ---\n",
            data.len()
        ));
    }
    let text = String::from_utf8_lossy(&data);
    let (body, note) = if text.len() > MENTION_LIMIT {
        let mut end = MENTION_LIMIT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        (
            &text[..end],
            format!(" (first {end} of {} bytes)", text.len()),
        )
    } else {
        (text.as_ref(), String::new())
    };
    Some(format!("--- @{name}{note} ---\n{}\n", body.trim_end()))
}

/// Runs `!CMD` in the working directory: its output, stderr after stdout,
/// the exit status when it failed, clipped.
fn run_shell(command: &str, cwd: &str) -> String {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output();
    let text = match out {
        Ok(o) => {
            let mut text = String::from_utf8_lossy(&o.stdout).to_string();
            let stderr = String::from_utf8_lossy(&o.stderr);
            if !stderr.trim().is_empty() {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&stderr);
            }
            if !o.status.success() {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&match o.status.code() {
                    Some(code) => format!("(exit {code})"),
                    None => "(killed by a signal)".to_string(),
                });
            }
            text
        }
        Err(e) => format!("could not run sh: {e}"),
    };
    clip_chars(&text, SHELL_OUTPUT_LIMIT)
}

fn clip_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let kept: String = text.chars().take(limit).collect();
    format!(
        "{kept}\n... ({} more characters)",
        text.chars().count() - limit
    )
}

/// Ties the session's lines (ANSI) when stderr is a terminal that takes
/// them.
#[derive(Clone, Copy)]
struct Style {
    on: bool,
}

impl Style {
    fn detect(tty: bool) -> Self {
        Self {
            on: tty
                && io::stderr().is_terminal()
                && std::env::var_os("NO_COLOR").is_none()
                && std::env::var("TERM").map(|t| t != "dumb").unwrap_or(true),
        }
    }

    fn dim(&self, s: &str) -> String {
        if self.on {
            format!("\x1b[2m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
}

/// SIGINT while a turn runs cancels it; otherwise it reaches the keyboard
/// as `Input::Signal`, which counts as a ^C only when plain lines are read
/// (the line editor reports its own). Dropping the guard restores the
/// default disposition.
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
                            if tx.send(Input::Signal).is_err() {
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
    style: Style,
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
            | StepKind::Process
            | StepKind::Memory
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
        let line = match step.kind {
            StepKind::Error | StepKind::Plan => line,
            _ => self.style.dim(&line),
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
    /// When the user last typed: a prompt long after it rings the bell.
    last_input: Arc<Mutex<Instant>>,
}

impl ChatUi {
    fn current_cancel(&self) -> Option<CancelToken> {
        self.running.lock().expect("running turn poisoned").clone()
    }

    /// One line from the user, None when nobody can answer.
    fn answer(&mut self, prompt: &str) -> Option<String> {
        let cancel = self.current_cancel();
        let away = self
            .last_input
            .lock()
            .expect("last input poisoned")
            .elapsed()
            > ATTENTION_AFTER;
        if away && self.tty && io::stderr().is_terminal() {
            let _ = write!(self.err, "\x07");
        }
        let _ = self.err.flush();
        let ask = Ask {
            prompt: prompt.to_string(),
            history: false,
            bold: false,
        };
        let answer = match self.keyboard.read(ask, cancel.as_ref()) {
            Input::Line(line) => Some(line.trim().to_string()),
            Input::Interrupt | Input::Signal => {
                if let Some(c) = cancel {
                    c.cancel();
                }
                None
            }
            Input::Eof => {
                self.closed.store(true, Ordering::SeqCst);
                None
            }
        };
        *self.last_input.lock().expect("last input poisoned") = Instant::now();
        answer
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
        let prompt = if req.always.is_empty() {
            "  [y]es, [n]o, [all] without asking from now on, or a reason to decline: ".to_string()
        } else {
            format!(
                "  [y]es, [n]o, [a]lways for {}, [all] without asking from now on, or a reason to decline: ",
                req.always
            )
        };
        let Some(answer) = self.answer(&prompt) else {
            let _ = writeln!(self.err);
            return Approval::Deny("the user did not answer (interrupted or input closed)".into());
        };
        match answer.to_lowercase().as_str() {
            "" | "y" | "yes" => Approval::Allow,
            "a" | "always" if !req.always.is_empty() => {
                let _ = writeln!(
                    self.err,
                    "  allowing {} for the rest of the session (/permissions lists the rules)",
                    req.always
                );
                Approval::Always
            }
            "a" | "always" | "all" => {
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
        let answer = self.answer("  answer (a number or your words): ")?;
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
    if !id.is_empty() {
        let (id, traj) = load_resume(&c.home, &id).map_err(|e| {
            errorf(
                2,
                &format!("pulse harness chat: --resume {id}: {e}"),
                "",
                "pulse harness sessions lists sessions; a session's trajectory is ~/.pulse/harness/ID.json",
            )
        })?;
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
        let dir = Path::new(&c.home).join("harness");
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

/// The trajectory of session `id` (or at path `id`), with its messages.
fn load_resume(home: &str, id: &str) -> Result<(String, Trajectory)> {
    let path = if id.ends_with(".json") || id.contains('/') {
        PathBuf::from(id)
    } else {
        Path::new(home).join("harness").join(format!("{id}.json"))
    };
    let traj = read_trajectory(&path)?;
    if traj.messages.is_empty() {
        anyhow::bail!("the trajectory has no messages to carry on");
    }
    let label = if id.ends_with(".json") || id.contains('/') {
        path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| id.to_string())
    } else {
        id.to_string()
    };
    Ok((label, traj))
}

fn read_trajectory(path: &Path) -> Result<Trajectory> {
    let data = std::fs::read(path)?;
    Ok(serde_json::from_slice(&data)?)
}

/// Visits the interactive sessions that ran in `cwd`, newest first by
/// trajectory modification time, until `visit` returns false.
fn scan_sessions(
    dir: &Path,
    cwd: &str,
    mut visit: impl FnMut(String, SystemTime, Trajectory) -> bool,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(SystemTime, PathBuf)> = entries
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
    for (modified, path) in files.into_iter().take(CONTINUE_SCAN_LIMIT) {
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
            if !visit(id, modified, traj) {
                return;
            }
        }
    }
}

/// The newest interactive session that ran in `cwd`.
fn newest_session_in(dir: &Path, cwd: &str) -> Option<(String, Trajectory)> {
    let mut found = None;
    scan_sessions(dir, cwd, |id, _, traj| {
        found = Some((id, traj));
        false
    });
    found
}

/// `/sessions`: the recent sessions in `cwd`, one line each.
fn recent_sessions(dir: &Path, cwd: &str, except: &str) -> Vec<String> {
    let mut lines = Vec::new();
    scan_sessions(dir, cwd, |id, modified, traj| {
        if id == except {
            return true;
        }
        let when = chrono::DateTime::<chrono::Local>::from(modified)
            .format("%Y-%m-%d %H:%M")
            .to_string();
        let first = traj
            .messages
            .iter()
            .find(|m| matches!(m.role, Role::User))
            .map(|m| one_line(&m.content, 70))
            .unwrap_or_default();
        let n = traj
            .messages
            .iter()
            .filter(|m| !matches!(m.role, Role::System))
            .count();
        lines.push(format!("  {id}  {when}  {n:>3} messages  {first}"));
        lines.len() < SESSIONS_LISTED
    });
    lines
}

/// `/export`: the conversation as markdown, tool results clipped.
fn transcript_markdown(msgs: &[Message], session_id: &str, model: &str) -> String {
    let mut md = format!("# pulse harness chat {session_id}\n\nmodel: {model}\n");
    for m in msgs {
        match m.role {
            Role::System => continue,
            Role::User => {
                md.push_str("\n## You\n\n");
                md.push_str(m.content.trim_end());
                md.push('\n');
            }
            Role::Assistant => {
                md.push_str("\n## Assistant\n\n");
                if !m.content.trim().is_empty() {
                    md.push_str(m.content.trim_end());
                    md.push('\n');
                }
                for call in &m.tool_calls {
                    md.push_str(&format!(
                        "\n- `{}` {}\n",
                        call.name,
                        one_line(&call.arguments, 200)
                    ));
                }
            }
            Role::Tool => {
                let name = if m.name.is_empty() { "tool" } else { &m.name };
                md.push_str(&format!(
                    "\n<details><summary>{name} result</summary>\n\n```text\n{}\n```\n\n</details>\n",
                    clip_chars(m.content.trim_end(), EXPORT_TOOL_LIMIT).replace("```", "'''")
                ));
            }
        }
    }
    md
}

/// `/diff`: the working tree's changes against the index and HEAD.
fn git_changes(cwd: &str) -> String {
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim_end().to_string())
    };
    let Some(status) = run(&["status", "--short"]) else {
        return format!("{cwd} is not a git repository (or git is not installed)");
    };
    if status.is_empty() {
        return "the working tree is clean".to_string();
    }
    let mut out = status;
    let stat = run(&["diff", "--stat", "HEAD"]).or_else(|| run(&["diff", "--stat"]));
    if let Some(stat) = stat
        && !stat.is_empty()
    {
        out.push('\n');
        out.push_str(&stat);
    }
    clip_chars(&out, SHELL_OUTPUT_LIMIT)
}

const INIT_TASK: &str = "Look at this repository and write (or improve) its AGENTS.md: what an engineer or an agent needs to know before working here. Explore first: the README, how it is built and tested, the layout, the conventions visible in the code, what a newcomer would get wrong. Then write AGENTS.md (keep what an existing one already gets right) with: what the project is, in two lines; how to build, test, lint and run it; the layout and where things go; the conventions to follow and the pitfalls; what to leave alone. Keep it under 150 lines of plain, specific prose and lists, no filler. Finish by telling me in a few lines what you wrote.";

const DELEGATE_TASK: &str = "Write the brief for a task that hands the work discussed in this conversation to another engineer, who has not seen the conversation and cannot ask questions. Make it self-contained: the goal; what is already known (files, findings and decisions so far, with paths); what to do, step by step where it matters; how to verify the result; what to leave alone. Do not run tools and do not start the work. Reply with the brief only, starting with a line `Title: <up to twelve words>`, then the body in plain markdown.";

const PLAN_ON_NOTE: &str = "[Plan mode is on: explore the code and work the plan out with the user; do not change anything until plan mode is off. Tools that change things are not offered and changing commands are refused.]";
const PLAN_OFF_NOTE: &str =
    "[Plan mode is off: you may carry the plan out now, asking for approval as usual.]";

/// What a slash command asks of the loop.
enum Slash {
    Done,
    Exit,
    /// Start over: fresh session, carrying on `resume` if given.
    Restart(Option<Resume>),
    /// Run a turn with this text, then do `then` with the reply.
    Turn {
        text: String,
        then: Then,
    },
}

enum Then {
    Reply,
    Delegate { title: String },
}

struct Session<'a> {
    agent: Arc<Mutex<Agent>>,
    provider: Arc<dyn Provider>,
    session_id: &'a str,
    trajectory_path: &'a str,
    cwd: &'a str,
    home: &'a str,
    err: SharedWriter,
    auto: Arc<AtomicBool>,
    running: Arc<Mutex<Option<CancelToken>>>,
    start: Instant,
    turns: i32,
    /// Blocks the next message carries before the user's text: shell
    /// output the user saw, a plan-mode change.
    context: Vec<String>,
    mode_note: Option<&'static str>,
}

impl Session<'_> {
    /// `/permissions`: the rules in force; `allow PATTERN`/`deny PATTERN`
    /// adds one for the session; `clear` drops the session's.
    fn permissions(&self, arg: &str) -> String {
        use crate::permissions::{Rule, Source};
        let (verb, pattern) = match arg.split_once(char::is_whitespace) {
            Some((v, p)) => (v.trim(), p.trim()),
            None => (arg.trim(), ""),
        };
        match verb {
            "" => {
                let agent = self.agent.lock().expect("agent poisoned");
                let rules = agent.permissions();
                if rules.is_empty() {
                    return "permissions: no rules; edits and changing commands are shown before they run (/permissions allow bash(git *) adds a rule, `a` at an approval adds one for that command)".to_string();
                }
                let mut out = vec![format!(
                    "permissions: {} rule{} (deny beats allow; the rest asks as usual)",
                    rules.len(),
                    if rules.len() == 1 { "" } else { "s" }
                )];
                out.extend(rules.describe().into_iter().map(|l| format!("  {l}")));
                out.join("\n")
            }
            "clear" => {
                let n = self
                    .agent
                    .lock()
                    .expect("agent poisoned")
                    .clear_session_permissions();
                format!(
                    "dropped {n} session rule{}; the flags' and settings' rules stay",
                    if n == 1 { "" } else { "s" }
                )
            }
            "allow" | "deny" if pattern.is_empty() => {
                format!(
                    "usage: /permissions {verb} PATTERN (bash(git *), edit(src/*), write, fetch(docs.rs), mcp__github__*)"
                )
            }
            "allow" | "deny" => match Rule::parse(verb == "allow", pattern, Source::Session) {
                Ok(rule) => {
                    let line = format!(
                        "{} {} for the rest of the session{}",
                        verb,
                        rule.pattern(),
                        if verb == "deny" && rule.filter.is_none() {
                            "; the tool is no longer offered"
                        } else {
                            ""
                        }
                    );
                    self.agent
                        .lock()
                        .expect("agent poisoned")
                        .add_permission(rule);
                    line
                }
                Err(e) => format!("/permissions: {e}"),
            },
            _ => "usage: /permissions [allow PATTERN | deny PATTERN | clear]".to_string(),
        }
    }

    /// `/procs`: the background processes; `ID [LINES]` shows the tail of
    /// one's output so far (the model's unread output stays unread);
    /// `stop ID|all` ends them.
    /// `/memory`: the memory files, `add [user] TEXT`, `forget TEXT`,
    /// `clear project|user`.
    fn memory(&self, arg: &str) -> String {
        let agent = self.agent.lock().expect("agent poisoned");
        let Some(memory) = agent.memory() else {
            return "memory is off for this session (--no-memory, --bare or harness.memory off)"
                .to_string();
        };
        let (verb, rest) = match arg.split_once(char::is_whitespace) {
            Some((v, r)) => (v.trim(), r.trim()),
            None => (arg.trim(), ""),
        };
        match verb {
            "" => memory.list(),
            "add" | "remember" => {
                let (scope, text) = match rest.split_once(char::is_whitespace) {
                    Some(("user", more)) => (Scope::User, more.trim()),
                    Some(("project", more)) => (Scope::Project, more.trim()),
                    _ => (Scope::Project, rest),
                };
                if text.is_empty() {
                    return "usage: /memory add [user] TEXT".to_string();
                }
                match memory.remember(scope, text) {
                    Ok(c) => format!(
                        "remembered for the {} ({} in {}); later sessions read it, this one goes on as it is",
                        c.scope.as_str(),
                        c.count,
                        memory.shown_path(c.scope)
                    ),
                    Err(e) => e,
                }
            }
            "forget" | "remove" => {
                if rest.is_empty() {
                    return "usage: /memory forget TEXT (enough of the memory to match one)"
                        .to_string();
                }
                match memory.forget(None, rest) {
                    Ok(c) => format!(
                        "forgotten from the {} ({} left in {})",
                        c.scope.as_str(),
                        c.count,
                        memory.shown_path(c.scope)
                    ),
                    Err(e) => e,
                }
            }
            "clear" => match Scope::parse(rest) {
                Ok(scope) if !rest.is_empty() => match memory.clear(scope) {
                    Ok(n) => format!(
                        "cleared {} from the {} memory ({})",
                        n,
                        scope.as_str(),
                        memory.shown_path(scope)
                    ),
                    Err(e) => e,
                },
                _ => "usage: /memory clear project|user".to_string(),
            },
            other => format!(
                "unknown /memory {other}; /memory lists, /memory add [user] TEXT, /memory forget TEXT, /memory clear project|user"
            ),
        }
    }

    fn procs(&self, arg: &str) -> String {
        let mut agent = self.agent.lock().expect("agent poisoned");
        // Plan mode withdraws the tool but not what already runs: the list
        // stays readable and stoppable.
        if !agent.offers_background() && agent.procs().is_empty() {
            return "background processes are not offered in this session".to_string();
        }
        let (verb, rest) = match arg.split_once(char::is_whitespace) {
            Some((v, r)) => (v.trim(), r.trim()),
            None => (arg.trim(), ""),
        };
        let procs = agent.procs();
        match verb {
            "" => {
                let lines = procs.describe();
                if lines.is_empty() {
                    return "no background processes (the model starts one with bash background:true)".to_string();
                }
                let mut out = vec![format!(
                    "{} background process{}:",
                    lines.len(),
                    if lines.len() == 1 { "" } else { "es" }
                )];
                out.extend(lines.into_iter().map(|l| format!("  {l}")));
                out.join("\n")
            }
            "stop" if rest == "all" => {
                let stopped = procs.stop_all();
                if stopped.is_empty() {
                    "nothing was running; the list is cleared".to_string()
                } else {
                    format!("stopped {}: {}", stopped.len(), stopped.join("; "))
                }
            }
            "stop" if rest.is_empty() => "usage: /procs stop ID|all".to_string(),
            "stop" => match procs.get(rest) {
                Some(m) => {
                    let was_running = m.stop();
                    let line = if was_running {
                        format!("stopped {}", m.describe())
                    } else {
                        format!("{} had already ended: {}", rest, m.state())
                    };
                    procs.remove(rest);
                    line
                }
                None => format!("no background process {rest}; /procs lists them"),
            },
            id => match procs.get(id) {
                Some(m) => {
                    let n: usize = rest.parse().unwrap_or(20).clamp(1, 500);
                    let head = m.describe();
                    let tail = m.peek_tail(n);
                    if tail.is_empty() {
                        format!("{head}\n  (no output yet)")
                    } else {
                        format!(
                            "{head}\n{}",
                            tail.lines()
                                .map(|l| format!("  {l}"))
                                .collect::<Vec<_>>()
                                .join("\n")
                        )
                    }
                }
                None => format!(
                    "no background process {id}; /procs lists them, /procs stop ID|all ends them"
                ),
            },
        }
    }

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

    fn say(&mut self, line: &str) {
        let _ = writeln!(self.err, "{line}");
        let _ = self.err.flush();
    }

    /// The message a turn sends for `text`: the pending context, the
    /// text, and the files it mentions.
    fn compose(&mut self, text: &str) -> String {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let (text, attached) = expand_mentions(text, Path::new(self.cwd), home.as_deref());
        if !attached.is_empty() {
            let line = format!("attached: {}", attached.join(", "));
            self.say(&line);
        }
        let mut parts: Vec<String> = Vec::new();
        if let Some(note) = self.mode_note.take() {
            parts.push(note.to_string());
        }
        parts.append(&mut self.context);
        parts.push(text);
        parts.join("\n\n")
    }

    /// `!CMD`: runs it, shows the output, and carries it to the model
    /// with the next message.
    fn shell(&mut self, command: &str) {
        let command = command.trim();
        if command.is_empty() {
            self.say("usage: !CMD runs CMD in the working directory");
            return;
        }
        let output = run_shell(command, self.cwd);
        let shown = if output.trim().is_empty() {
            "(no output)".to_string()
        } else {
            output.trim_end().to_string()
        };
        self.say(&shown);
        self.context.push(format!(
            "--- the user ran `{command}` in the shell; its output ---\n{shown}\n--- end of output ---"
        ));
    }

    fn with_cancel<T>(&self, f: impl FnOnce(&CancelToken) -> T) -> T {
        let cancel = CancelToken::new();
        *self.running.lock().expect("running turn poisoned") = Some(cancel.clone());
        let out = f(&cancel);
        *self.running.lock().expect("running turn poisoned") = None;
        out
    }

    fn slash(&mut self, command: &str) -> Slash {
        let mut parts = command.splitn(2, char::is_whitespace);
        let name = parts.next().unwrap_or("").to_lowercase();
        let arg = parts.next().unwrap_or("").trim().to_string();
        let line = match name.as_str() {
            "help" | "?" => chat_help(),
            "exit" | "quit" | "q" => return Slash::Exit,
            "new" | "clear" => return Slash::Restart(None),
            "resume" => {
                if arg.is_empty() {
                    "usage: /resume ID (/sessions lists them)".to_string()
                } else {
                    match load_resume(self.home, &arg) {
                        Ok(found) => return Slash::Restart(Some(Box::new(found))),
                        Err(e) => format!("/resume {arg}: {e}"),
                    }
                }
            }
            "sessions" => {
                let dir = Path::new(self.home).join("harness");
                let lines = recent_sessions(&dir, self.cwd, self.session_id);
                if lines.is_empty() {
                    format!("no earlier sessions ran in {}", self.cwd)
                } else {
                    format!(
                        "earlier sessions in {} (newest first):\n{}\n/resume ID carries one on",
                        self.cwd,
                        lines.join("\n")
                    )
                }
            }
            "model" => {
                if arg.is_empty() {
                    format!(
                        "model: {}",
                        self.agent.lock().expect("agent poisoned").cfg.model
                    )
                } else {
                    let (window, listed) = self.with_cancel(|cancel| {
                        let window = model_context_window(cancel, self.provider.as_ref(), &arg);
                        let listed = self
                            .provider
                            .models(cancel)
                            .map(|ms| ms.iter().any(|m| m.id == arg))
                            .unwrap_or(true);
                        (window, listed)
                    });
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
            "plan" => {
                let current = self.agent.lock().expect("agent poisoned").cfg.read_only;
                let on = match arg.as_str() {
                    "" => !current,
                    "on" => true,
                    "off" => false,
                    _ => {
                        self.say("usage: /plan [on|off]");
                        return Slash::Done;
                    }
                };
                if on != current {
                    self.agent.lock().expect("agent poisoned").set_read_only(on);
                    self.mode_note = Some(if on { PLAN_ON_NOTE } else { PLAN_OFF_NOTE });
                }
                if on {
                    "plan mode: on; the model reads and plans, edits and changing commands are refused (/plan off to act)".to_string()
                } else {
                    "plan mode: off; the model may change things again, asking for approval as usual".to_string()
                }
            }
            "compact" => {
                let out = self.with_cancel(|cancel| {
                    self.agent
                        .lock()
                        .expect("agent poisoned")
                        .compact_now(cancel)
                });
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
            "permissions" => self.permissions(&arg),
            "procs" => self.procs(&arg),
            "memory" => self.memory(&arg),
            "diff" => git_changes(self.cwd),
            "export" => {
                let path = if arg.is_empty() {
                    Path::new(self.home)
                        .join("harness")
                        .join(format!("{}.md", self.session_id))
                } else if let Some(rest) = arg.strip_prefix("~/") {
                    match std::env::var_os("HOME") {
                        Some(h) => PathBuf::from(h).join(rest),
                        None => PathBuf::from(&arg),
                    }
                } else {
                    PathBuf::from(&arg)
                };
                let agent = self.agent.lock().expect("agent poisoned");
                let msgs = agent.messages();
                let n = msgs
                    .iter()
                    .filter(|m| !matches!(m.role, Role::System))
                    .count();
                let md = transcript_markdown(&msgs, self.session_id, &agent.cfg.model);
                drop(agent);
                match path
                    .parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|()| std::fs::write(&path, md))
                {
                    Ok(()) => format!("wrote {n} messages to {}", path.display()),
                    Err(e) => format!("/export: {}: {e}", path.display()),
                }
            }
            "env" => {
                let agent = self.agent.lock().expect("agent poisoned");
                let mut lines = agent.environment();
                drop(agent);
                lines.push(format!(
                    "approvals: {}",
                    if self.auto.load(Ordering::SeqCst) {
                        "changes run without asking"
                    } else {
                        "changes are shown first"
                    }
                ));
                lines.push(format!(
                    "session: {}{}",
                    self.session_id,
                    if self.trajectory_path.is_empty() {
                        String::new()
                    } else {
                        format!("; trajectory {}", self.trajectory_path)
                    }
                ));
                lines.join("\n")
            }
            "init" => {
                return Slash::Turn {
                    text: INIT_TASK.to_string(),
                    then: Then::Reply,
                };
            }
            "delegate" => {
                let talked = self
                    .agent
                    .lock()
                    .expect("agent poisoned")
                    .messages()
                    .iter()
                    .any(|m| matches!(m.role, Role::User));
                if !talked {
                    "nothing to delegate yet: say what the work is first, then /delegate [TITLE]"
                        .to_string()
                } else {
                    return Slash::Turn {
                        text: DELEGATE_TASK.to_string(),
                        then: Then::Delegate { title: arg },
                    };
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
        self.say(&line);
        Slash::Done
    }
}

/// `/delegate`: the title and brief from the model's reply (`Title:`
/// first line, then the body), the title given on the command winning.
fn split_brief(reply: &str, given: &str) -> (String, String) {
    let reply = reply.trim();
    let mut lines = reply.lines();
    let (mut title, body) = match lines.next() {
        Some(first) if first.trim_start().to_lowercase().starts_with("title:") => {
            let t = first.trim_start()[6..].trim().trim_matches('`').to_string();
            (t, lines.collect::<Vec<_>>().join("\n").trim().to_string())
        }
        _ => (String::new(), reply.to_string()),
    };
    if !given.trim().is_empty() {
        title = given.trim().to_string();
    }
    if title.is_empty() {
        title = one_line(
            reply.lines().next().unwrap_or("task from a chat session"),
            100,
        );
    }
    let body = if body.is_empty() {
        reply.to_string()
    } else {
        body
    };
    (title, body)
}

/// Puts the brief on the board as a task for the project this directory
/// belongs to.
fn delegate(
    c: &Context,
    st: Option<&pulse_store::Store>,
    cwd: &str,
    title: &str,
    brief: &str,
) -> std::result::Result<String, String> {
    let Some(s) = st else {
        return Err("no store open (--no-store, or no PULSE home); the board needs one".into());
    };
    let mut project = std::env::var("PULSE_PROJECT").unwrap_or_default();
    if project.is_empty() {
        project = s
            .project_for_dir(cwd)
            .map_err(|e| e.to_string())?
            .map(|p| p.slug)
            .ok_or_else(|| {
                format!(
                    "{cwd} is not inside a registered project; pulse project add PATH registers it, or set PULSE_PROJECT"
                )
            })?;
    }
    let author = if c.agent.is_empty() {
        pulse_store::HUMAN_AGENT.to_string()
    } else {
        c.agent.clone()
    };
    let task = pulse_tasks::tasks::new(s, &project, title, brief, 0, &author)
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "task {} is on the board for {project}: {}\n  the lead picks it up on the next pass; pulse task show {} follows it",
        task.id, task.title, task.id
    ))
}

/// What a session leaves the command with: its exit code, or a fresh
/// session to start (carrying on `resume` if given).
enum Outcome {
    End(i32),
    Restart(Option<Resume>),
}

/// A session to carry on: its label and the trajectory to replay.
type Resume = Box<(String, Trajectory)>;

/// What outlives a session: the keyboard, the person at it, the
/// interrupt state, and the first message.
struct Live {
    out: SharedWriter,
    err: SharedWriter,
    keyboard: Arc<Keyboard>,
    ui: Arc<Mutex<ChatUi>>,
    running: Arc<Mutex<Option<CancelToken>>>,
    auto: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    last_input: Arc<Mutex<Instant>>,
    tty: bool,
    style: Style,
    cwd: String,
    first: Option<String>,
    sessions: u32,
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
    let mut resume = pick_resume(c, &fs, &cwd)?.map(Box::new);

    let out = SharedWriter::new(std::mem::replace(&mut c.out, Box::new(io::sink())));
    c.out = Box::new(out.clone());
    let err = SharedWriter::new(std::mem::replace(&mut c.err, Box::new(io::sink())));
    c.err = Box::new(err.clone());

    let running: Arc<Mutex<Option<CancelToken>>> = Arc::new(Mutex::new(None));
    let auto = Arc::new(AtomicBool::new(fs.get_bool("yes")));
    let closed = Arc::new(AtomicBool::new(false));
    let last_input = Arc::new(Mutex::new(Instant::now()));
    let (tx, rx) = mpsc::channel();
    let (asks, ask_rx) = mpsc::channel();
    let (prompter, tty) = open_prompter(&cwd, &c.home, &mut err.clone(), c.quiet);
    let editor = matches!(prompter, Prompter::Terminal { .. });
    spawn_prompter(prompter, ask_rx, tx.clone(), err.clone());
    let interrupts = Interrupts::watch(tx, running.clone());
    if let Err(e) = &interrupts
        && !c.quiet
    {
        writeln!(
            err.clone(),
            "pulse harness chat: Ctrl-C handling is off: {e}"
        )?;
    }
    let keyboard = Arc::new(Keyboard {
        asks,
        rx: Mutex::new(rx),
        editor,
    });
    let ui = Arc::new(Mutex::new(ChatUi {
        keyboard: keyboard.clone(),
        err: err.clone(),
        auto: auto.clone(),
        tty,
        quiet: c.quiet,
        closed: closed.clone(),
        running: running.clone(),
        last_input: last_input.clone(),
    }));
    let mut live = Live {
        out,
        err,
        keyboard,
        ui,
        running,
        auto,
        closed,
        last_input,
        tty,
        style: Style::detect(tty),
        cwd,
        first: (!first.trim().is_empty()).then_some(first),
        sessions: 0,
    };
    let code = loop {
        match run_session(c, &fs, &mut live, resume.take())? {
            Outcome::End(code) => break code,
            Outcome::Restart(next) => {
                resume = next;
                // A fresh session gets its own id even when --session named this one.
                fs.set("session", "")?;
            }
        }
    };
    drop(interrupts);
    if code != 0 {
        return Err(errorf(code, "", "", "").into());
    }
    Ok(())
}

/// One session from its first prompt to its end: the agent, its turns and
/// the commands between them.
fn run_session(
    c: &mut Context,
    fs: &FlagSet,
    live: &mut Live,
    resume: Option<Resume>,
) -> Result<Outcome> {
    let quiet = c.quiet;
    let json = c.json;
    let streamed = Arc::new(Mutex::new(String::new()));
    let sink = ChatSink {
        out: live.out.clone(),
        err: live.err.clone(),
        streamed: streamed.clone(),
        quiet,
        json,
        style: live.style,
    };
    let mut prepared: Prepared = prepare_agent(
        c,
        fs,
        "chat",
        String::new(),
        true,
        vec![Box::new(sink)],
        false,
    )?;
    let agent = prepared.shared.clone();
    live.sessions += 1;
    let mut e = live.err.clone();

    if let Some(resume) = resume {
        let (label, traj) = *resume;
        let n = traj
            .messages
            .iter()
            .filter(|m| !matches!(m.role, Role::System))
            .count();
        agent
            .lock()
            .expect("agent poisoned")
            .resume_messages(traj.messages)?;
        if !quiet {
            writeln!(e, "resumed session {label}: {n} messages carried over")?;
        }
    }
    {
        let mut a = agent.lock().expect("agent poisoned");
        a.set_ui(live.ui.clone());
        if !json {
            let streamed = streamed.clone();
            let mut out = live.out.clone();
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
        if live.sessions == 1 {
            writeln!(
                e,
                "pulse harness chat: {} {} in {} (session {})",
                prepared.provider_name, prepared.model_name, prepared.cwd, prepared.session_id
            )?;
            writeln!(
                e,
                "{}",
                if live.auto.load(Ordering::SeqCst) {
                    "edits and changing commands run without asking (--yes); /help lists the commands, /exit leaves"
                } else {
                    "edits and changing commands are shown for approval first; /help lists the commands, /exit leaves"
                }
            )?;
        } else {
            writeln!(
                e,
                "new session {} ({} {})",
                prepared.session_id, prepared.provider_name, prepared.model_name
            )?;
        }
    }

    let session_id = prepared.session_id.clone();
    let trajectory_path = prepared.trajectory_path.clone();
    let cwd = live.cwd.clone();
    let home = c.home.clone();
    let mut session = Session {
        agent: agent.clone(),
        provider: prepared.provider.clone(),
        session_id: &session_id,
        trajectory_path: &trajectory_path,
        cwd: &cwd,
        home: &home,
        err: live.err.clone(),
        auto: live.auto.clone(),
        running: live.running.clone(),
        start: Instant::now(),
        turns: 0,
        context: Vec::new(),
        mode_note: None,
    };
    let mut pending = live.first.take();
    let mut idle_interrupts = 0;
    let mut exit_code = 0;
    let mut end_status = EXIT_ENDED.to_string();
    let mut restart: Option<Option<Resume>> = None;
    loop {
        let text = match pending.take() {
            Some(t) => t,
            None => {
                if live.closed.load(Ordering::SeqCst) {
                    break;
                }
                let ask = Ask {
                    prompt: if live.tty && !quiet { "you> " } else { "" }.to_string(),
                    history: true,
                    bold: true,
                };
                match live.keyboard.read(ask, None) {
                    Input::Eof => {
                        if live.tty && !quiet {
                            writeln!(e)?;
                        }
                        break;
                    }
                    Input::Interrupt | Input::Signal => {
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
        *live.last_input.lock().expect("last input poisoned") = Instant::now();
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if !live.tty && !quiet {
            writeln!(e, "you> {}", one_line(text, 200))?;
        }
        let (message, then) = if let Some(command) = text.strip_prefix('/') {
            match session.slash(command) {
                Slash::Done => continue,
                Slash::Exit => break,
                Slash::Restart(next) => {
                    restart = Some(next);
                    break;
                }
                Slash::Turn { text, then } => (session.compose(&text), then),
            }
        } else if let Some(command) = text.strip_prefix('!') {
            session.shell(command);
            continue;
        } else {
            (session.compose(text), Then::Reply)
        };
        let cancel = CancelToken::new();
        *live.running.lock().expect("running turn poisoned") = Some(cancel.clone());
        let stop_watch = prepared.watch_cancel(&cancel);
        let outcome = agent
            .lock()
            .expect("agent poisoned")
            .turn(&cancel, &message);
        *live.running.lock().expect("running turn poisoned") = None;
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
                if let Then::Delegate { title } = then {
                    let (title, brief) = split_brief(&reply, &title);
                    let line = match delegate(c, prepared.st.as_ref(), &cwd, &title, &brief) {
                        Ok(line) => line,
                        Err(why) => format!("/delegate: {why}"),
                    };
                    writeln!(e, "{line}")?;
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
    Ok(match restart {
        Some(next) => Outcome::Restart(next),
        None => Outcome::End(exit_code),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn continues_on_an_unescaped_trailing_backslash() {
        assert!(continues("hello \\"));
        assert!(!continues("hello"));
        assert!(!continues("a path\\\\"));
        assert!(continues("three\\\\\\"));
        assert!(!continues(""));
        assert_eq!(unescape_continuations("one \\\ntwo"), "one \ntwo");
    }

    #[test]
    fn completes_slash_commands_at_the_start_only() {
        let (start, found) = complete_line("/pl", 3, Path::new("."), None);
        assert_eq!(start, 0);
        assert_eq!(
            found,
            vec![("/plan [on|off]".to_string(), "/plan ".to_string())]
        );
        let (_, all) = complete_line("/", 1, Path::new("."), None);
        assert_eq!(all.len(), SLASH_COMMANDS.len());
        let (_, none) = complete_line("say /pl", 7, Path::new("."), None);
        assert!(none.is_empty());
        let (_, none) = complete_line("plain words", 11, Path::new("."), None);
        assert!(none.is_empty());
    }

    #[test]
    fn completes_mentioned_paths_under_the_working_directory() {
        let dir = tmp();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("README.md"), "# hi").unwrap();
        std::fs::write(dir.path().join(".hidden"), "").unwrap();
        let line = "look at @sr";
        let (start, found) = complete_line(line, line.len(), dir.path(), None);
        assert_eq!(start, 8);
        assert_eq!(found, vec![("src/".to_string(), "@src/".to_string())]);
        let line = "look at @src/ma";
        let (_, found) = complete_line(line, line.len(), dir.path(), None);
        assert_eq!(
            found,
            vec![("src/main.rs".to_string(), "@src/main.rs ".to_string())]
        );
        let (_, found) = complete_line("@", 1, dir.path(), None);
        let names: Vec<&str> = found.iter().map(|(d, _)| d.as_str()).collect();
        assert_eq!(names, vec!["README.md", "src/"]);
        let (_, found) = complete_line("@.h", 3, dir.path(), None);
        assert_eq!(found[0].0, ".hidden");
        let (_, found) = complete_line("@nowhere/x", 10, dir.path(), None);
        assert!(found.is_empty());
    }

    #[test]
    fn mentions_attach_files_and_listings_and_leave_unknown_tokens() {
        let dir = tmp();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn x() {}\n").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "remember\n").unwrap();
        std::fs::write(dir.path().join("blob.bin"), b"\x00\x01\x02").unwrap();
        let (text, attached) = expand_mentions(
            "explain @src/lib.rs, list @src and @notes.txt. ping @nobody and @blob.bin",
            dir.path(),
            None,
        );
        assert_eq!(attached, vec!["src/lib.rs", "src", "notes.txt", "blob.bin"]);
        assert!(text.starts_with("explain @src/lib.rs, list @src and @notes.txt. ping @nobody"));
        assert!(text.contains("--- @src/lib.rs ---\npub fn x() {}\n"));
        assert!(text.contains("--- @src (directory, 1 entries) ---\nlib.rs\n"));
        assert!(text.contains("--- @notes.txt ---\nremember\n"));
        assert!(text.contains("--- @blob.bin (3 bytes, binary; not attached) ---"));
        let (same, none) = expand_mentions("mail me@example.com", dir.path(), None);
        assert_eq!(same, "mail me@example.com");
        assert!(none.is_empty());
    }

    #[test]
    fn long_mentions_are_clipped_at_a_char_boundary() {
        let dir = tmp();
        let big = format!("a{}", "é".repeat(MENTION_LIMIT));
        std::fs::write(dir.path().join("big.txt"), &big).unwrap();
        let (text, _) = expand_mentions("@big.txt", dir.path(), None);
        assert!(text.contains(&format!(
            "(first {} of {} bytes)",
            MENTION_LIMIT - 1,
            big.len()
        )));
    }

    #[test]
    fn shell_runs_in_the_directory_and_reports_failure() {
        let dir = tmp();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        let out = run_shell("ls", dir.path().to_str().unwrap());
        assert_eq!(out.trim(), "a.txt");
        let out = run_shell("echo oops >&2; exit 3", dir.path().to_str().unwrap());
        assert_eq!(out, "oops\n(exit 3)");
    }

    #[test]
    fn briefs_split_into_title_and_body() {
        let (title, body) = split_brief(
            "Title: Fix the login retry\n\nDo the thing.\nThen verify.",
            "",
        );
        assert_eq!(title, "Fix the login retry");
        assert_eq!(body, "Do the thing.\nThen verify.");
        let (title, body) = split_brief("Title: Model's title\nbody", "Given title");
        assert_eq!(title, "Given title");
        assert_eq!(body, "body");
        let (title, body) = split_brief("Just a brief without a title line.", "");
        assert_eq!(title, "Just a brief without a title line.");
        assert_eq!(body, "Just a brief without a title line.");
    }

    #[test]
    fn transcript_markdown_skips_the_system_prompt_and_clips_tools() {
        let msgs = vec![
            Message {
                role: Role::System,
                content: "secret prompt".into(),
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
            },
            Message {
                role: Role::User,
                content: "hello".into(),
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
            },
            Message {
                role: Role::Tool,
                content: "x".repeat(EXPORT_TOOL_LIMIT + 10),
                tool_calls: Vec::new(),
                tool_call_id: "1".into(),
                name: "bash".into(),
            },
            Message {
                role: Role::Assistant,
                content: "hi there".into(),
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
            },
        ];
        let md = transcript_markdown(&msgs, "S1", "m");
        assert!(!md.contains("secret prompt"));
        assert!(md.contains("## You\n\nhello"));
        assert!(md.contains("bash result"));
        assert!(md.contains("more characters"));
        assert!(md.contains("## Assistant\n\nhi there"));
    }

    #[test]
    fn recent_sessions_lists_interactive_sessions_in_the_directory() {
        let dir = tmp();
        let write = |name: &str, cwd: &str, interactive: bool, first: &str| {
            let traj = json!({
                "trajectory_format": "pulse-harness-v1",
                "info": {
                    "model_stats": {"api_calls": 1, "prompt_tokens": 1, "completion_tokens": 1, "cost": 0.0},
                    "exit_status": "ended",
                    "submission": "",
                    "config": {"model": "m", "provider": "p", "max_steps": 1, "step_timeout_seconds": 1, "cwd": cwd, "interactive": interactive}
                },
                "messages": [{"role": "system", "content": "s"}, {"role": "user", "content": first}],
                "steps": []
            });
            std::fs::write(
                dir.path().join(format!("{name}.json")),
                serde_json::to_vec(&traj).unwrap(),
            )
            .unwrap();
        };
        write("here-1", "/work/a", true, "first question");
        write("elsewhere", "/work/b", true, "other");
        write("job", "/work/a", false, "job task");
        write("current", "/work/a", true, "this one");
        let lines = recent_sessions(dir.path(), "/work/a", "current");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("here-1"));
        assert!(lines[0].contains("1 messages"));
        assert!(lines[0].contains("first question"));
        let newest = newest_session_in(dir.path(), "/work/b").expect("found");
        assert_eq!(newest.0, "elsewhere");
    }
}
