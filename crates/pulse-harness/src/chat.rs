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
        "rewind",
        "[N [files|conversation]]",
        "list the turns, or put the files the model edited and the conversation back to before turn N (files or conversation alone on request)",
    ),
    (
        "diff",
        "",
        "what changed in the working tree (git status and diff --stat)",
    ),
    (
        "review",
        "[REF]",
        "a read-only review of the working tree's changes, or of this branch since REF (e.g. main)",
    ),
    (
        "pr",
        "[TITLE]",
        "write the description and open a pull request for this branch (push and gh pr create ask first)",
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
@PATH in a message attaches the file (a directory: its listing); #123 or owner/repo#123 attaches the GitHub issue or pull request (through gh); !CMD runs a shell command and the model sees its output.
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
/// under the text, so the model has them without a read; `#123` and
/// `owner/repo#123` attach the GitHub issue or pull request through `gh`
/// (the program at `gh`). A token that names nothing stays as written;
/// the notes say which GitHub references could not be read and why.
fn expand_mentions(text: &str, cwd: &Path, home: Option<&Path>, gh: &Path) -> Mentions {
    let mut attached: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut looked_up: Vec<&str> = Vec::new();
    let mut blocks = String::new();
    for token in text.split_whitespace() {
        if let Some(raw) = token.strip_prefix('@') {
            let name = trim_mention(raw);
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
            continue;
        }
        let reference = trim_mention(token.trim_start_matches(['(', '[', '"', '\'']));
        let Some((repo, number)) = github_reference(reference) else {
            continue;
        };
        if looked_up.contains(&reference) {
            continue;
        }
        looked_up.push(reference);
        match github_mention_block(reference, repo, number, cwd, gh) {
            Ok(block) => {
                blocks.push_str(&block);
                attached.push(reference.to_string());
            }
            Err(GhError::Missing(why)) => {
                notes.push(format!("{reference}: {why}"));
                break;
            }
            Err(GhError::Failed(why)) => notes.push(format!("{reference}: {why}")),
        }
    }
    let text = if attached.is_empty() {
        text.to_string()
    } else {
        format!("{text}\n\n{}", blocks.trim_end())
    };
    Mentions {
        text,
        attached,
        notes,
    }
}

/// A message with its mentions attached, what was attached, and what
/// could not be.
struct Mentions {
    text: String,
    attached: Vec<String>,
    notes: Vec<String>,
}

/// A mention without the punctuation that ends a sentence around it.
fn trim_mention(raw: &str) -> &str {
    let name = raw.trim_end_matches([',', ';', ':', ')', ']', '?', '!', '"', '\'']);
    name.strip_suffix('.').unwrap_or(name)
}

/// `#123` or `owner/repo#123`: the repository (empty for the one in the
/// working directory) and the number.
fn github_reference(token: &str) -> Option<(&str, &str)> {
    let (repo, number) = token.rsplit_once('#')?;
    if number.is_empty() || number.len() > 9 || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !repo.is_empty() {
        let (owner, name) = repo.split_once('/')?;
        let plain = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
        };
        if !plain(owner) || !plain(name) {
            return None;
        }
    }
    Some((repo, number))
}

enum GhError {
    /// gh is not there; no point asking again this message.
    Missing(String),
    Failed(String),
}

const ISSUE_FIELDS: &str = "number,title,state,url,author,body,labels,comments";
const PR_FIELDS: &str = "number,title,state,url,author,body,comments,additions,deletions,changedFiles,files,baseRefName,headRefName,isDraft";
/// Files of a mentioned pull request listed.
const PR_FILES_LISTED: usize = 50;

/// What `gh` knows about the issue or pull request, as a block like a
/// mentioned file's: tried as an issue first, then as a pull request. gh
/// answers `issue view` for a pull request too (with a `/pull/` url), so
/// one of those is asked again in its own shape, with files and branches.
fn github_mention_block(
    reference: &str,
    repo: &str,
    number: &str,
    cwd: &Path,
    gh: &Path,
) -> std::result::Result<String, GhError> {
    let view = |kind: &str, fields: &str| -> std::result::Result<serde_json::Value, GhError> {
        let mut cmd = std::process::Command::new(gh);
        cmd.args([kind, "view", number, "--json", fields]);
        if !repo.is_empty() {
            cmd.args(["--repo", repo]);
        }
        let out = cmd
            .current_dir(cwd)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    GhError::Missing("gh (the GitHub CLI) is not installed".to_string())
                } else {
                    GhError::Failed(format!("could not run gh: {e}"))
                }
            })?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let mut why = stderr
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("gh failed")
                .to_string();
            if why.contains("Could not resolve to a Repository") {
                why.push_str(
                    " (see gh auth status: the active account may not see it; gh auth switch)",
                );
            }
            return Err(GhError::Failed(why));
        }
        serde_json::from_slice(&out.stdout)
            .map_err(|e| GhError::Failed(format!("gh returned something other than JSON: {e}")))
    };
    let (kind, value) = match view("issue", ISSUE_FIELDS) {
        Ok(v) => {
            let is_pull = v
                .get("url")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|u| u.contains("/pull/"));
            if is_pull {
                match view("pr", PR_FIELDS) {
                    Ok(p) => ("pull request", p),
                    Err(GhError::Missing(why)) => return Err(GhError::Missing(why)),
                    Err(GhError::Failed(_)) => ("pull request", v),
                }
            } else {
                ("issue", v)
            }
        }
        Err(GhError::Missing(why)) => return Err(GhError::Missing(why)),
        Err(GhError::Failed(first)) => match view("pr", PR_FIELDS) {
            Ok(v) => ("pull request", v),
            Err(GhError::Missing(why)) => return Err(GhError::Missing(why)),
            Err(GhError::Failed(_)) => return Err(GhError::Failed(first)),
        },
    };
    Ok(github_block(reference, kind, &value))
}

fn github_block(reference: &str, kind: &str, v: &serde_json::Value) -> String {
    let text = |k: &str| {
        v.get(k)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
    };
    let number = |k: &str| v.get(k).and_then(serde_json::Value::as_u64);
    let login = |v: &serde_json::Value| {
        v.get("author")
            .and_then(|a| a.get("login"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let mut head = format!("--- {reference} ({kind}, {}", text("state").to_lowercase());
    if kind == "pull request" {
        let (base, branch) = (text("baseRefName"), text("headRefName"));
        if !base.is_empty() && !branch.is_empty() {
            head.push_str(&format!("; {branch} -> {base}"));
        }
        if let (Some(files), Some(add), Some(del)) = (
            number("changedFiles"),
            number("additions"),
            number("deletions"),
        ) {
            head.push_str(&format!("; {files} files, +{add} -{del}"));
        }
        if v.get("isDraft").and_then(serde_json::Value::as_bool) == Some(true) {
            head.push_str("; draft");
        }
    }
    let mut block = format!("{head}) ---\n{}\n", text("title"));
    if !text("url").is_empty() {
        block.push_str(&format!("{}\n", text("url")));
    }
    let author = login(v);
    if !author.is_empty() {
        block.push_str(&format!("by {author}\n"));
    }
    let labels: Vec<&str> = v
        .get("labels")
        .and_then(serde_json::Value::as_array)
        .map(|ls| {
            ls.iter()
                .filter_map(|l| l.get("name").and_then(serde_json::Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    if !labels.is_empty() {
        block.push_str(&format!("labels: {}\n", labels.join(", ")));
    }
    if !text("body").is_empty() {
        block.push_str(&format!("\n{}\n", text("body")));
    }
    if let Some(files) = v.get("files").and_then(serde_json::Value::as_array)
        && !files.is_empty()
    {
        block.push_str("\nfiles:\n");
        for f in files.iter().take(PR_FILES_LISTED) {
            let path = f
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?");
            let add = f
                .get("additions")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let del = f
                .get("deletions")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            block.push_str(&format!("  {path} +{add} -{del}\n"));
        }
        if files.len() > PR_FILES_LISTED {
            block.push_str(&format!("  ... {} more\n", files.len() - PR_FILES_LISTED));
        }
    }
    if let Some(comments) = v.get("comments").and_then(serde_json::Value::as_array)
        && !comments.is_empty()
    {
        block.push_str(&format!("\ncomments ({}, latest last):\n", comments.len()));
        for c in comments {
            let when = c
                .get("createdAt")
                .and_then(serde_json::Value::as_str)
                .map(|d| d.get(..10).unwrap_or(d))
                .unwrap_or("");
            let body = c
                .get("body")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim();
            block.push_str(&format!("- {} ({when}): {body}\n", login(c)));
        }
    }
    clip_block(&block, MENTION_LIMIT)
}

/// The block cut at a char boundary inside `limit` bytes, saying so.
fn clip_block(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n... (first {end} of {} bytes)\n",
        text[..end].trim_end(),
        text.len()
    )
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

/// How often the status line redraws while a turn waits on the model.
const STATUS_TICK: Duration = Duration::from_secs(1);

/// One line at the bottom of stderr that shows how long the running turn
/// has waited, so a slow model call reads as moving seconds instead of a
/// frozen row. A timer redraws it between steps; every step and every
/// streamed token erase it first, so it never mixes into the output.
/// Without a terminal it draws nothing.
struct StatusLine {
    err: SharedWriter,
    style: Style,
    tty: bool,
    /// When the running turn started; `None` between turns.
    started: Option<Instant>,
    /// The text on screen right now, if any.
    shown: Option<String>,
    /// How many times the line was drawn or redrawn.
    draws: u64,
}

impl StatusLine {
    fn new(err: SharedWriter, style: Style, tty: bool) -> Self {
        Self {
            err,
            style,
            tty,
            started: None,
            shown: None,
            draws: 0,
        }
    }

    /// A turn started: the next tick draws the line.
    fn start(&mut self, now: Instant) {
        self.started = Some(now);
    }

    /// The turn ended: erase the line and stop drawing.
    fn stop(&mut self) {
        self.clear();
        self.started = None;
    }

    /// Redraws the elapsed time at `now`. The seconds move even when no
    /// step arrives, which is the whole point.
    fn tick(&mut self, now: Instant) {
        if !self.tty {
            return;
        }
        let Some(started) = self.started else {
            return;
        };
        let secs = now.saturating_duration_since(started).as_secs();
        let text = format!("thinking {}:{:02}", secs / 60, secs % 60);
        if self.shown.as_deref() == Some(text.as_str()) {
            return;
        }
        let _ = write!(self.err, "\r\x1b[K{}", self.style.dim(&text));
        let _ = self.err.flush();
        self.shown = Some(text);
        self.draws += 1;
    }

    /// Erases the line so other output starts on a clean row.
    fn clear(&mut self) {
        if self.shown.take().is_some() {
            let _ = write!(self.err, "\r\x1b[K");
            let _ = self.err.flush();
        }
    }
}

/// Redraws the status line every [`STATUS_TICK`] until the returned
/// closure is called, which also erases it.
fn tick_status(status: Arc<Mutex<StatusLine>>) -> impl FnOnce() {
    status
        .lock()
        .expect("status line poisoned")
        .start(Instant::now());
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let ticker = status.clone();
    let handle = thread::spawn(move || {
        loop {
            ticker
                .lock()
                .expect("status line poisoned")
                .tick(Instant::now());
            match stop_rx.recv_timeout(STATUS_TICK) {
                Err(RecvTimeoutError::Timeout) => {}
                Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    move || {
        let _ = stop_tx.send(());
        let _ = handle.join();
        status.lock().expect("status line poisoned").stop();
    }
}

/// The reply's words on stdout as they stream in. A restart (the stream
/// broke and the model is asked again) ends the partial line and forgets
/// it, so the reply is printed again whole and the Assistant step that
/// follows is still not printed twice.
struct ConsoleStream {
    streamed: Arc<Mutex<String>>,
    status: Arc<Mutex<StatusLine>>,
    out: SharedWriter,
}

impl StreamSink for ConsoleStream {
    fn text(&mut self, text: &str) {
        self.status.lock().expect("status line poisoned").clear();
        self.streamed
            .lock()
            .expect("streamed text poisoned")
            .push_str(text);
        let _ = self.out.write_all(text.as_bytes());
        let _ = self.out.flush();
    }

    fn restart(&mut self) {
        self.status.lock().expect("status line poisoned").clear();
        let mut buf = self.streamed.lock().expect("streamed text poisoned");
        if !buf.is_empty() && !buf.ends_with('\n') {
            let _ = self.out.write_all(b"\n");
            let _ = self.out.flush();
        }
        buf.clear();
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
    status: Arc<Mutex<StatusLine>>,
    quiet: bool,
    json: bool,
    style: Style,
}

impl Sink for ChatSink {
    fn step(&mut self, step: Step) {
        self.status.lock().expect("status line poisoned").clear();
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
            // The agent's own words about the command first, as the hub shows it.
            StepKind::Command => match step.title.trim() {
                "" => format!("{indent}$ {}", one_line(&step.command, 160)),
                title => format!("{indent}{title}  $ {}", one_line(&step.command, 120)),
            },
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
            | StepKind::Search
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
pub(crate) fn load_resume(home: &str, id: &str) -> Result<(String, Trajectory)> {
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

/// Where session `id` keeps what `/rewind` needs, beside its trajectory.
fn rewind_dir(home: &str, id: &str) -> PathBuf {
    Path::new(home).join("harness").join(format!("{id}.rewind"))
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

const REVIEW_TASK: &str = "Review the changes below as a careful code reviewer. Look for what would matter in a review and say it with confidence: bugs and logic errors, unhandled errors and edge cases, concurrency and resource problems, security holes, broken or missing tests, docs and messages the code now contradicts. Read the surrounding code with the read-only tools when the diff alone does not tell. Skip style, formatting and naming unless they hide a bug. Reply with the findings first, most serious first, each as `path:line - what is wrong, why it matters, what to do`; then, in a line or two, what the change gets right. If nothing is wrong, say so plainly. This turn is read-only: do not change anything.";

const PR_TASK: &str = "Open a pull request for the current branch. First see what it holds: `git status --short` (if there are uncommitted changes, stop and tell me instead of committing them); the base branch (`gh repo view --json defaultBranchRef --jq .defaultBranchRef.name`); `git log --oneline BASE..HEAD` and `git diff --stat BASE...HEAD`, reading the diff where the commit messages do not explain it. Then write the title (under 70 characters) and the description: what changed and why, how it was tested, what a reviewer should look at first - plain prose and short lists, no filler. Push the branch if it has no upstream yet (`git push -u origin HEAD`) and create the pull request with `gh pr create --title TITLE --body BODY`, quoting the body (`--body \"$(cat <<'EOF' ... EOF)\"` keeps its lines). Finish with the pull request's URL.";

/// Bytes of a diff `/review` hands the model; it reads the files for the
/// rest.
const REVIEW_LIMIT: usize = 48 * 1024;

fn git_in(cwd: &str, args: &[&str]) -> std::result::Result<String, String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("git failed")
        .to_string())
}

/// `/review [REF]`: what there is to review - the working tree against
/// HEAD (untracked files named), or this branch since REF - as a heading
/// and the diff, or why there is nothing.
fn review_diff(cwd: &str, reference: &str) -> std::result::Result<(String, String), String> {
    if git_in(cwd, &["rev-parse", "--is-inside-work-tree"]).is_err() {
        return Err(format!(
            "{cwd} is not a git repository (or git is not installed)"
        ));
    }
    let reference = reference.trim();
    let (what, range) = if reference.is_empty() {
        let has_head = git_in(cwd, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok();
        (
            "the working tree against HEAD".to_string(),
            has_head.then(|| "HEAD".to_string()),
        )
    } else {
        let commit = format!("{reference}^{{commit}}");
        if git_in(cwd, &["rev-parse", "--verify", "-q", &commit]).is_err() {
            return Err(format!(
                "/review {reference}: not a commit, branch or tag here"
            ));
        }
        (
            format!("this branch since {reference}"),
            Some(format!("{reference}...HEAD")),
        )
    };
    let mut stat_args = vec!["diff", "--stat"];
    let mut diff_args = vec!["diff"];
    if let Some(r) = range.as_deref() {
        stat_args.push(r);
        diff_args.push(r);
    }
    let stat = git_in(cwd, &stat_args)?;
    let diff = git_in(cwd, &diff_args)?;
    let untracked = if reference.is_empty() {
        git_in(cwd, &["ls-files", "--others", "--exclude-standard"]).unwrap_or_default()
    } else {
        String::new()
    };
    if diff.trim().is_empty() && untracked.trim().is_empty() {
        return Err(if reference.is_empty() {
            "nothing to review: the working tree matches HEAD (/review REF reviews this branch since REF, e.g. /review main)".to_string()
        } else {
            format!("nothing to review: no changes since {reference}")
        });
    }
    let mut summary = stat.lines().last().unwrap_or("").trim().to_string();
    let untracked: Vec<&str> = untracked.lines().filter(|l| !l.trim().is_empty()).collect();
    if !untracked.is_empty() {
        if !summary.is_empty() {
            summary.push_str("; ");
        }
        summary.push_str(&format!(
            "{} untracked file{}",
            untracked.len(),
            if untracked.len() == 1 { "" } else { "s" }
        ));
    }
    let mut body = String::new();
    if !stat.is_empty() {
        body.push_str(&stat);
        body.push_str("\n\n");
    }
    body.push_str(&diff);
    if !untracked.is_empty() {
        body.push_str("\n\nuntracked files (not in the diff; read the ones that matter):\n");
        for f in &untracked {
            body.push_str(&format!("  {f}\n"));
        }
    }
    Ok((
        format!("{what}: {summary}"),
        clip_block(&body, REVIEW_LIMIT),
    ))
}

/// `/pr`: the branch a pull request would come from, or why not now.
fn pr_branch(cwd: &str) -> std::result::Result<String, String> {
    let branch = git_in(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).map_err(|_| {
        format!("{cwd} is not a git repository with a commit (or git is not installed)")
    })?;
    if branch == "HEAD" {
        return Err(
            "HEAD is detached; switch to a branch first (git switch -c NAME), then /pr".to_string(),
        );
    }
    if matches!(branch.as_str(), "main" | "master" | "develop" | "trunk") {
        return Err(format!(
            "on {branch}: put the change on a branch first (git switch -c NAME), then /pr"
        ));
    }
    Ok(branch)
}

fn gh_installed() -> bool {
    std::process::Command::new("gh")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

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
    Delegate {
        title: String,
    },
    /// `/review` ran the turn read-only; put plan mode back as it was.
    Restore {
        read_only: bool,
    },
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
    /// A `/rewind` dropped messages since the trajectory was last
    /// written; a closed terminal must not bring them back.
    rewound: bool,
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
    /// `/rewind`: the turns so far; `/rewind N [files|conversation]` puts
    /// the checkout and the conversation (or one of them) back to before
    /// turn N.
    fn rewind(&mut self, arg: &str) -> String {
        use crate::rewind::What;
        let mut agent = self.agent.lock().expect("agent poisoned");
        let (number, what) = match arg.split_once(char::is_whitespace) {
            Some((n, w)) => (n.trim(), w.trim()),
            None => (arg.trim(), ""),
        };
        if number.is_empty() {
            let turns = agent.rewind_turns();
            if turns.is_empty() {
                return "no turns yet to rewind".to_string();
            }
            let mut out = vec![format!(
                "{} turn{} (/rewind N puts the files the model edited and the conversation back to before turn N; add `files` or `conversation` for one of them; what shell commands changed is not recorded):",
                turns.len(),
                if turns.len() == 1 { "" } else { "s" }
            )];
            out.extend(turns.into_iter().map(|l| format!("  {l}")));
            return out.join("\n");
        }
        let Ok(n) = number.parse::<usize>() else {
            return "usage: /rewind [N [files|conversation]]".to_string();
        };
        let Some(what) = What::parse(what) else {
            return format!("unknown /rewind {what}; usage: /rewind [N [files|conversation]]");
        };
        // The agent's note step already says what was put back and
        // dropped; only what the note cannot know is added here.
        match agent.rewind(n, what) {
            Ok(report) => {
                self.rewound |= report.truncate_to.is_some();
                if what == What::Files
                    && (!report.restored.is_empty() || !report.removed.is_empty())
                {
                    // The conversation goes on, so the model hears what
                    // the checkout lost with the next message.
                    let mut changed: Vec<String> = report.restored.clone();
                    changed.extend(report.removed.iter().map(|p| format!("{p} (removed)")));
                    self.context.push(format!(
                        "Note: the user put these files back to how they were before turn {n}: {}. Read a file again before relying on what you changed in it.",
                        changed.join(", ")
                    ));
                    return "the model is told with your next message".to_string();
                }
                String::new()
            }
            Err(e) => e,
        }
    }

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
    /// text, and the files and GitHub references it mentions.
    fn compose(&mut self, text: &str) -> String {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let found = expand_mentions(text, Path::new(self.cwd), home.as_deref(), Path::new("gh"));
        if !found.attached.is_empty() {
            let line = format!("attached: {}", found.attached.join(", "));
            self.say(&line);
        }
        for note in &found.notes {
            let line = format!("not attached: {note}");
            self.say(&line);
        }
        self.compose_plain(found.text)
    }

    /// The message for a turn the session wrote itself (`/init`,
    /// `/review`, `/pr`, `/delegate`): nothing in it is a mention.
    fn compose_plain(&mut self, text: String) -> String {
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
                    Some(false) => "nothing compacted; the note above says why".to_string(),
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
            "rewind" => {
                let line = self.rewind(&arg);
                if line.is_empty() {
                    return Slash::Done;
                }
                line
            }
            "diff" => git_changes(self.cwd),
            "review" => match review_diff(self.cwd, &arg) {
                Err(why) => why,
                Ok((heading, block)) => {
                    let read_only = self.agent.lock().expect("agent poisoned").cfg.read_only;
                    if !read_only {
                        self.agent.lock().expect("agent poisoned").set_read_only(true);
                    }
                    self.say(&format!("reviewing {heading} (a read-only turn)"));
                    return Slash::Turn {
                        text: format!("{REVIEW_TASK}\n\n--- {heading} ---\n{block}"),
                        then: Then::Restore { read_only },
                    };
                }
            },
            "pr" => match pr_branch(self.cwd) {
                Err(why) => why,
                Ok(_) if !gh_installed() => {
                    "gh (the GitHub CLI) is not installed; install it and run gh auth login, then /pr".to_string()
                }
                Ok(branch) => {
                    self.say(&format!(
                        "a pull request from {branch}: the push and gh pr create are shown for approval first"
                    ));
                    let mut text = PR_TASK.to_string();
                    if !arg.is_empty() {
                        text.push_str(&format!(" Use this title: {arg}"));
                    }
                    return Slash::Turn {
                        text,
                        then: Then::Reply,
                    };
                }
            },
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
    let status = Arc::new(Mutex::new(StatusLine::new(
        live.err.clone(),
        live.style,
        live.tty && !quiet && !json,
    )));
    let sink = ChatSink {
        out: live.out.clone(),
        err: live.err.clone(),
        streamed: streamed.clone(),
        status: status.clone(),
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

    agent
        .lock()
        .expect("agent poisoned")
        .set_rewind_dir(rewind_dir(&c.home, &prepared.session_id));
    if let Some(resume) = resume {
        let (label, traj) = *resume;
        let n = traj
            .messages
            .iter()
            .filter(|m| !matches!(m.role, Role::System))
            .count();
        let mut a = agent.lock().expect("agent poisoned");
        a.resume_messages(traj.messages)?;
        let rewindable = match a.load_rewind(&rewind_dir(&c.home, &label)) {
            Ok(n) => n,
            Err(err) => {
                if !quiet {
                    writeln!(e, "rewind records of {label} not read: {err}")?;
                }
                0
            }
        };
        drop(a);
        if !quiet {
            writeln!(
                e,
                "resumed session {label}: {n} messages carried over{}",
                if rewindable > 0 {
                    format!(
                        "; {rewindable} turn{} can still be rewound",
                        if rewindable == 1 { "" } else { "s" }
                    )
                } else {
                    String::new()
                }
            )?;
        }
    }
    {
        let mut a = agent.lock().expect("agent poisoned");
        a.set_ui(live.ui.clone());
        if !json {
            a.set_stream(Box::new(ConsoleStream {
                streamed: streamed.clone(),
                status: status.clone(),
                out: live.out.clone(),
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
        rewound: false,
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
                Slash::Done => {
                    if std::mem::take(&mut session.rewound) {
                        prepared.write_trajectory(c)?;
                    }
                    continue;
                }
                Slash::Exit => break,
                Slash::Restart(next) => {
                    restart = Some(next);
                    break;
                }
                Slash::Turn { text, then } => (session.compose_plain(text), then),
            }
        } else if let Some(command) = text.strip_prefix('!') {
            session.shell(command);
            continue;
        } else {
            (session.compose(text), Then::Reply)
        };
        let cancel = CancelToken::new();
        *live.running.lock().expect("running turn poisoned") = Some(cancel.clone());
        let stop_watch = prepared.watch_cancel(&cancel, None);
        let stop_status = tick_status(status.clone());
        let outcome = agent
            .lock()
            .expect("agent poisoned")
            .turn(&cancel, &message);
        stop_status();
        *live.running.lock().expect("running turn poisoned") = None;
        if let Some(stop) = stop_watch {
            stop();
        }
        session.turns += 1;
        if let Then::Restore { read_only } = &then
            && !read_only
        {
            agent.lock().expect("agent poisoned").set_read_only(false);
        }
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
            Ok(Turn::Blocked(reason)) => {
                if json {
                    write_json(
                        &mut c.out,
                        &json!({
                            "session": session_id,
                            "turn": session.turns,
                            "blocked": reason,
                        }),
                    )?;
                } else if c.quiet {
                    // Otherwise the sink has shown the note already.
                    writeln!(e, "{reason}")?;
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

    /// A stderr the test can read back after the status line wrote to it.
    fn captured_err() -> (SharedWriter, Arc<Mutex<Vec<u8>>>) {
        struct Capture(Arc<Mutex<Vec<u8>>>);
        impl Write for Capture {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0
                    .lock()
                    .expect("capture poisoned")
                    .extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let buf = Arc::new(Mutex::new(Vec::new()));
        (SharedWriter::new(Box::new(Capture(buf.clone()))), buf)
    }

    #[test]
    fn status_line_redraws_elapsed_seconds_without_a_new_step() {
        let (err, buf) = captured_err();
        let mut status = StatusLine::new(err, Style { on: false }, true);
        let t0 = Instant::now();
        status.start(t0);
        status.tick(t0);
        assert_eq!(status.draws, 1);
        // The same second again: nothing to redraw.
        status.tick(t0 + Duration::from_millis(300));
        assert_eq!(status.draws, 1);
        // Seconds later, with no step in between, the row moves.
        status.tick(t0 + Duration::from_secs(2));
        assert_eq!(status.draws, 2);
        status.tick(t0 + Duration::from_secs(61));
        assert_eq!(status.draws, 3);
        let text = String::from_utf8(buf.lock().expect("capture poisoned").clone()).expect("utf8");
        assert!(text.contains("thinking 0:00"), "{text:?}");
        assert!(text.contains("thinking 0:02"), "{text:?}");
        assert!(text.contains("thinking 1:01"), "{text:?}");
        // Stopping erases the row and later ticks draw nothing.
        status.stop();
        assert!(text.len() < buf.lock().expect("capture poisoned").len());
        status.tick(t0 + Duration::from_secs(90));
        assert_eq!(status.draws, 3);
    }

    #[test]
    fn command_steps_print_the_agents_description_before_the_command() {
        let (err, buf) = captured_err();
        let (out, _) = captured_err();
        let status = Arc::new(Mutex::new(StatusLine::new(
            err.clone(),
            Style { on: false },
            false,
        )));
        let mut sink = ChatSink {
            out,
            err,
            streamed: Arc::new(Mutex::new(String::new())),
            status,
            quiet: false,
            json: false,
            style: Style { on: false },
        };
        sink.step(Step {
            kind: StepKind::Command,
            command: "cargo test -p pulse-store".into(),
            title: "Run the store tests".into(),
            ..Step::default()
        });
        sink.step(Step {
            kind: StepKind::Command,
            command: "ls".into(),
            ..Step::default()
        });
        let text = String::from_utf8(buf.lock().expect("capture poisoned").clone()).expect("utf8");
        assert!(
            text.contains("Run the store tests  $ cargo test -p pulse-store\n"),
            "{text:?}"
        );
        assert!(
            text.contains("\n$ ls\n") || text.starts_with("$ ls\n"),
            "{text:?}"
        );
    }

    #[test]
    fn status_line_stays_silent_without_a_terminal() {
        let (err, buf) = captured_err();
        let mut status = StatusLine::new(err, Style { on: false }, false);
        let t0 = Instant::now();
        status.start(t0);
        status.tick(t0 + Duration::from_secs(5));
        assert_eq!(status.draws, 0);
        assert!(buf.lock().expect("capture poisoned").is_empty());
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

    /// A `gh` that is not there: no test reaches GitHub.
    fn no_gh() -> PathBuf {
        PathBuf::from("/nonexistent/gh")
    }

    #[test]
    fn mentions_attach_files_and_listings_and_leave_unknown_tokens() {
        let dir = tmp();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn x() {}\n").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "remember\n").unwrap();
        std::fs::write(dir.path().join("blob.bin"), b"\x00\x01\x02").unwrap();
        let found = expand_mentions(
            "explain @src/lib.rs, list @src and @notes.txt. ping @nobody and @blob.bin",
            dir.path(),
            None,
            &no_gh(),
        );
        let (text, attached) = (found.text, found.attached);
        assert_eq!(attached, vec!["src/lib.rs", "src", "notes.txt", "blob.bin"]);
        assert!(text.starts_with("explain @src/lib.rs, list @src and @notes.txt. ping @nobody"));
        assert!(text.contains("--- @src/lib.rs ---\npub fn x() {}\n"));
        assert!(text.contains("--- @src (directory, 1 entries) ---\nlib.rs\n"));
        assert!(text.contains("--- @notes.txt ---\nremember\n"));
        assert!(text.contains("--- @blob.bin (3 bytes, binary; not attached) ---"));
        let found = expand_mentions("mail me@example.com", dir.path(), None, &no_gh());
        assert_eq!(found.text, "mail me@example.com");
        assert!(found.attached.is_empty());
        assert!(found.notes.is_empty());
    }

    #[test]
    fn long_mentions_are_clipped_at_a_char_boundary() {
        let dir = tmp();
        let big = format!("a{}", "é".repeat(MENTION_LIMIT));
        std::fs::write(dir.path().join("big.txt"), &big).unwrap();
        let found = expand_mentions("@big.txt", dir.path(), None, &no_gh());
        assert!(found.text.contains(&format!(
            "(first {} of {} bytes)",
            MENTION_LIMIT - 1,
            big.len()
        )));
    }

    #[test]
    fn github_references_are_recognised() {
        assert_eq!(github_reference("#12"), Some(("", "12")));
        assert_eq!(github_reference("octo/repo#7"), Some(("octo/repo", "7")));
        assert_eq!(
            github_reference("my.org/my-repo_2#301"),
            Some(("my.org/my-repo_2", "301"))
        );
        for not in ["#", "#abc", "foo#1", "a/b/c#1", "#1234567890", "12", "c#"] {
            assert_eq!(github_reference(not), None, "{not}");
        }
        assert_eq!(trim_mention("#12),"), "#12");
        assert_eq!(trim_mention("#12."), "#12");
    }

    #[test]
    fn github_mentions_attach_issues_and_pull_requests_through_gh() {
        let dir = tmp();
        let gh = dir.path().join("gh");
        std::fs::write(
            &gh,
            r#"#!/bin/sh
kind=$1; number=$3; repo=$7
case "$kind-$number" in
  issue-12) printf '%s' '{"number":12,"title":"Login fails on Safari","state":"OPEN","url":"https://github.com/o/r/issues/12","author":{"login":"ana"},"body":"Steps: open the login page in Safari 17.","labels":[{"name":"bug"},{"name":"web"}],"comments":[{"author":{"login":"bo"},"createdAt":"2026-10-01T10:00:00Z","body":"Reproduced on 17.1."}]}' ;;
  issue-7) [ "$repo" = "octo/repo" ] || { echo "wrong repo: $repo" >&2; exit 1; }
        printf '%s' '{"number":7,"title":"Retry the upload","state":"MERGED","url":"https://github.com/octo/repo/pull/7","author":{"login":"cy"},"body":"Retries three times.","labels":[],"comments":[]}' ;;
  pr-7) [ "$repo" = "octo/repo" ] || { echo "wrong repo: $repo" >&2; exit 1; }
        printf '%s' '{"number":7,"title":"Retry the upload","state":"MERGED","url":"https://github.com/octo/repo/pull/7","author":{"login":"cy"},"body":"Retries three times.","comments":[],"additions":40,"deletions":5,"changedFiles":2,"files":[{"path":"src/upload.rs","additions":35,"deletions":5},{"path":"src/lib.rs","additions":5,"deletions":0}],"baseRefName":"main","headRefName":"retry-upload","isDraft":true}' ;;
  issue-500) echo "GraphQL: Could not resolve to a Repository with the name 'o/r'. (repository)" >&2; exit 1 ;;
  pr-500) echo "GraphQL: Could not resolve to a Repository with the name 'o/r'. (repository)" >&2; exit 1 ;;
  issue-8) printf '%s' '{"number":8,"title":"Old pull","state":"OPEN","url":"https://github.com/o/r/pull/8","author":{"login":"di"},"body":"","labels":[],"comments":[]}' ;;
  *) echo "GraphQL: Could not resolve to an issue or pull request with the number of $number." >&2; exit 1 ;;
esac
"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let found = expand_mentions(
            "look at #12 and (octo/repo#7), not #999. #12 again; #8 #500",
            dir.path(),
            None,
            &gh,
        );
        assert_eq!(found.attached, vec!["#12", "octo/repo#7", "#8"]);
        assert_eq!(found.notes.len(), 2, "{:?}", found.notes);
        assert!(found.notes[0].starts_with("#999: GraphQL: Could not resolve"));
        assert!(
            found.notes[0].ends_with("number of 999."),
            "{}",
            found.notes[0]
        );
        assert!(
            found.notes[1].ends_with(
                "(see gh auth status: the active account may not see it; gh auth switch)"
            ),
            "{}",
            found.notes[1]
        );
        let text = found.text;
        assert!(
            text.starts_with("look at #12 and (octo/repo#7), not #999. #12 again; #8 #500\n\n")
        );
        assert!(text.contains(
            "--- #12 (issue, open) ---\nLogin fails on Safari\nhttps://github.com/o/r/issues/12\nby ana\nlabels: bug, web\n\nSteps: open the login page in Safari 17.\n\ncomments (1, latest last):\n- bo (2026-10-01): Reproduced on 17.1.\n"
        ), "{text}");
        assert!(text.contains(
            "--- octo/repo#7 (pull request, merged; retry-upload -> main; 2 files, +40 -5; draft) ---\nRetry the upload\n"
        ), "{text}");
        assert!(text.contains("\nfiles:\n  src/upload.rs +35 -5\n  src/lib.rs +5 -0"));
        assert_eq!(text.matches("--- #12 (").count(), 1);
        // A pull request gh will only show as an issue keeps its true kind.
        assert!(
            text.contains(
                "--- #8 (pull request, open) ---\nOld pull\nhttps://github.com/o/r/pull/8\nby di"
            ),
            "{text}"
        );
    }

    #[test]
    fn a_missing_gh_is_said_once_and_leaves_the_message_alone() {
        let dir = tmp();
        let found = expand_mentions("see #1 and #2", dir.path(), None, &no_gh());
        assert_eq!(found.text, "see #1 and #2");
        assert!(found.attached.is_empty());
        assert_eq!(
            found.notes,
            vec!["#1: gh (the GitHub CLI) is not installed"]
        );
    }

    fn git(cwd: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn review_diff_shows_the_change_or_says_there_is_none() {
        let dir = tmp();
        let cwd = dir.path().to_str().unwrap();
        let plain = tmp();
        assert!(
            review_diff(plain.path().to_str().unwrap(), "")
                .unwrap_err()
                .contains("not a git repository")
        );
        git(dir.path(), &["init", "-q"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "one"]);
        assert!(
            review_diff(cwd, "")
                .unwrap_err()
                .starts_with("nothing to review: the working tree matches HEAD")
        );
        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        std::fs::write(dir.path().join("new.txt"), "fresh\n").unwrap();
        let (heading, block) = review_diff(cwd, "").unwrap();
        assert_eq!(
            heading,
            "the working tree against HEAD: 1 file changed, 1 insertion(+), 1 deletion(-); 1 untracked file"
        );
        assert!(block.contains("-one\n+two\n"), "{block}");
        assert!(block.contains(
            "untracked files (not in the diff; read the ones that matter):\n  new.txt\n"
        ));
        assert!(
            review_diff(cwd, "nope")
                .unwrap_err()
                .contains("not a commit, branch or tag here")
        );
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "two"]);
        let (heading, block) = review_diff(cwd, "HEAD~1").unwrap();
        assert!(
            heading.starts_with("this branch since HEAD~1: 2 files changed"),
            "{heading}"
        );
        assert!(block.contains("\n+fresh"));
        assert!(!block.contains("untracked"));
        assert!(
            review_diff(cwd, "HEAD")
                .unwrap_err()
                .starts_with("nothing to review: no changes since HEAD")
        );
    }

    #[test]
    fn a_pull_request_needs_a_branch_of_its_own() {
        let dir = tmp();
        let cwd = dir.path().to_str().unwrap();
        assert!(pr_branch(cwd).unwrap_err().contains("not a git repository"));
        git(dir.path(), &["init", "-q", "-b", "main"]);
        git(dir.path(), &["commit", "-q", "--allow-empty", "-m", "root"]);
        assert_eq!(
            pr_branch(cwd).unwrap_err(),
            "on main: put the change on a branch first (git switch -c NAME), then /pr"
        );
        git(dir.path(), &["switch", "-q", "-c", "topic"]);
        assert_eq!(pr_branch(cwd).unwrap(), "topic");
        git(dir.path(), &["checkout", "-q", "--detach"]);
        assert!(pr_branch(cwd).unwrap_err().starts_with("HEAD is detached"));
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
                images: Vec::new(),
            },
            Message {
                role: Role::User,
                content: "hello".into(),
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
                images: Vec::new(),
            },
            Message {
                role: Role::Tool,
                content: "x".repeat(EXPORT_TOOL_LIMIT + 10),
                tool_calls: Vec::new(),
                tool_call_id: "1".into(),
                name: "bash".into(),
                images: Vec::new(),
            },
            Message {
                role: Role::Assistant,
                content: "hi there".into(),
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
                images: Vec::new(),
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
