use crate::agents::{AgentDef, describe_for_tool, withheld_tools};
use crate::background::{
    BACKGROUND_LIMIT, BACKGROUND_WAIT_DEFAULT, BACKGROUND_WAIT_LIMIT, PROCESS_TOOL,
    PROCESS_WAIT_DEFAULT, Process, ProcessAction, Procs, dropped_marker, fmt_dur, process_tool,
};
use crate::compact::{
    COMPACT_KEEP_RECENT, COMPACT_MARKER, COMPACT_MIN_GAP_CALLS, apply_compaction, compaction_cut,
    render_transcript, should_compact, summary_request,
};
use crate::fetch::{
    FETCH_BYTE_LIMIT, FETCH_TIMEOUT, FETCH_TOOL, FetchPolicy, fetch, fetch_tool, url_host,
};
use crate::hooks::{
    HOOK_BLOCK_EXIT, HOOK_OUTPUT_LIMIT, HOOK_PAYLOAD_LIMIT, Hook, HookEvent, HookSet, cut_bytes,
    hook_command, run_payload, tool_payload,
};
use crate::memory::{MEMORY_TOOL, Memory, MemoryAction, memory_paragraph, memory_tool};
use crate::permissions::{self, Rule, Rules};
use crate::rewind::{self, Rewind};
use crate::search::{SEARCH_TIMEOUT, WEB_SEARCH_TOOL, WebSearch, web_search_tool};
use crate::skills::{SKILL_TOOL, Skill, load_skill, skill_tool};
use anyhow::anyhow;
use pulse_provider::{
    CancelToken, ImageContent, Message, Provider, Request, Role, Tool, ToolCall, Usage,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

pub const DEFAULT_MAX_STEPS: i32 = 60;
pub const DEFAULT_STEP_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Deadline for a `git commit` (and friends) whose pre-commit hook runs the
/// project's verify command: verify alone commonly runs 10-16 min after a
/// lock wait, so the generic step timeout killed green-in-progress commits.
pub const DEFAULT_COMMIT_TIMEOUT: Duration = Duration::from_secs(25 * 60);
/// First line of the pre-commit hook the runner installs
/// (`pulse_runner::keys::pre_commit_hook_script`); its presence tells the
/// harness that a commit step carries the verify gate.
pub const VERIFY_GATE_HOOK_MARKER: &str = "# pulse: verify-before-commit gate";
pub const DEFAULT_OUTPUT_LIMIT: usize = 20000;
pub const DEFAULT_KEEP_OBSERVATIONS: usize = 5;
pub const DEFAULT_MAX_FORMAT_ERRORS: i32 = 3;
pub const DEFAULT_FORMAT_RETRIES: i32 = 1;

pub use DEFAULT_COMMIT_TIMEOUT as DefaultCommitTimeout;
pub use DEFAULT_FORMAT_RETRIES as DefaultFormatRetries;
pub use DEFAULT_KEEP_OBSERVATIONS as DefaultKeepObservations;
pub use DEFAULT_MAX_FORMAT_ERRORS as DefaultMaxFormatErrors;
pub use DEFAULT_MAX_STEPS as DefaultMaxSteps;
pub use DEFAULT_OUTPUT_LIMIT as DefaultOutputLimit;
pub use DEFAULT_STEP_TIMEOUT as DefaultStepTimeout;

pub fn bash_tool() -> Tool {
    bash_tool_for(false)
}

/// The `bash` tool; with `background` it also takes `background: true`
/// (the command keeps running after the call returns and the `process`
/// tool manages it) and `wait`.
pub fn bash_tool_for(background: bool) -> Tool {
    let mut properties = json!({
        "command":{"type":"string","description":"The shell command to run with /bin/sh -c."},
        "description":{"type":"string","description":"One short plain-English sentence (under 100 characters) saying what the command does, for the person watching: 'Run the unit tests', 'List the files that mention retries'."}
    });
    let mut description = "Run one non-interactive /bin/sh command in the working directory. Each call is a fresh process; chain setup and use with &&. Always give a description: the owner reads it instead of the command.".to_string();
    if background {
        description.push_str(&format!(
            " With background:true the command keeps running after this call returns with its id (bg-1) and first output; use it for servers, watchers and long checks, then read, wait for or stop it with the process tool. wait is how many seconds to give it first (default {}): a command that ends within them returns its whole result like a foreground one.",
            BACKGROUND_WAIT_DEFAULT.as_secs()
        ));
        properties["background"] = json!({"type":"boolean","description":"Start the command in the background and return at once (default false)."});
        properties["wait"] = json!({"type":"integer","description":"With background: seconds to wait for the command to end before leaving it running."});
    }
    Tool {
        name: "bash".to_string(),
        description,
        parameters: json!({"type":"object","properties":properties,"required":["command","description"]}),
    }
}

pub fn read_tool() -> Tool {
    Tool {
        name: "read".to_string(),
        description: "Read one file inside the clone, every line numbered. Give start and end (1-based, inclusive) to read a range; a binary file reports its size.".to_string(),
        parameters: json!({"type":"object","properties":{
            "path":{"type":"string","description":"Path relative to the clone root."},
            "start":{"type":"integer","description":"First line to show (1-based)."},
            "end":{"type":"integer","description":"Last line to show (inclusive)."}
        },"required":["path"]}),
    }
}

pub fn grep_tool() -> Tool {
    Tool {
        name: "grep".to_string(),
        description: "Search file contents inside the clone with a regular expression; every matching line comes back as path:line: text. path is a file, a directory, or a glob such as src/*.rs (default: the whole clone).".to_string(),
        parameters: json!({"type":"object","properties":{
            "pattern":{"type":"string","description":"Rust regex syntax (no look-around)."},
            "path":{"type":"string","description":"File, directory or glob relative to the clone root."}
        },"required":["pattern"]}),
    }
}

pub fn glob_tool() -> Tool {
    Tool {
        name: "glob".to_string(),
        description: format!(
            "List files inside the clone whose path matches a glob; * matches any characters, / included, so *.rs finds every Rust file. Give path to search one directory. At most {GLOB_RESULT_LIMIT} paths come back."
        ),
        parameters: json!({"type":"object","properties":{
            "pattern":{"type":"string","description":"Glob matched against each file's path relative to the searched directory."},
            "path":{"type":"string","description":"Directory to search, relative to the clone root (default: the clone root)."}
        },"required":["pattern"]}),
    }
}

pub fn edit_tool() -> Tool {
    Tool {
        name: "edit".to_string(),
        description: "Edit one file by exact text replacement: search must occur exactly once (include enough surrounding lines to make it unique) and is replaced by replace. An empty search on a path that does not exist creates the file with replace as its content. Several replacements in the same file go in edits, applied in order.".to_string(),
        parameters: json!({"type":"object","properties":{
            "path":{"type":"string","description":"Path relative to the clone root."},
            "search":{"type":"string","description":"Exact text to find once, whitespace included."},
            "replace":{"type":"string","description":"Text that takes its place."},
            "edits":{"type":"array","description":"Several search/replace pairs for the same file, applied in order.","items":{"type":"object","properties":{"search":{"type":"string"},"replace":{"type":"string"}},"required":["search","replace"]}}
        },"required":["path"]}),
    }
}

pub fn write_tool() -> Tool {
    Tool {
        name: "write".to_string(),
        description: "Create a file inside the clone with the given content; missing parent directories are created. The file must not exist unless overwrite is true. For a change inside an existing file use edit.".to_string(),
        parameters: json!({"type":"object","properties":{
            "path":{"type":"string","description":"Path relative to the clone root."},
            "content":{"type":"string","description":"The whole file content."},
            "overwrite":{"type":"boolean","description":"Replace the file when it already exists (default false)."}
        },"required":["path","content"]}),
    }
}

pub fn plan_tool() -> Tool {
    Tool {
        name: "plan".to_string(),
        description: "Keep the checklist of this task's steps: send the whole list each time, each item with its status (todo, doing, done). The owner sees it next to your run; the rendered list comes back to you. Use it for work of three or more steps and update it as steps finish.".to_string(),
        parameters: json!({"type":"object","properties":{
            "items":{"type":"array","items":{"type":"object","properties":{
                "text":{"type":"string","description":"One short step."},
                "status":{"type":"string","enum":["todo","doing","done"]}
            },"required":["text","status"]}}
        },"required":["items"]}),
    }
}

/// The tools every run offers, in the order the model sees them.
pub fn native_tools() -> Vec<Tool> {
    native_tools_for(false)
}

/// The native tools a run offers; a read-only run leaves out `edit` and
/// `write`, which it would only refuse.
pub fn native_tools_for(read_only: bool) -> Vec<Tool> {
    native_tools_with(read_only, false)
}

/// [`native_tools_for`] with the `bash` tool's background option when the
/// environment can keep a process running (`background`).
pub fn native_tools_with(read_only: bool, background: bool) -> Vec<Tool> {
    let mut out = vec![
        bash_tool_for(background),
        read_tool(),
        grep_tool(),
        glob_tool(),
    ];
    if !read_only {
        out.push(edit_tool());
        out.push(write_tool());
    }
    out.push(plan_tool());
    out
}

pub const NATIVE_TOOL_NAMES: &[&str] = &["bash", "read", "grep", "glob", "edit", "write", "plan"];

/// The native tools a read-only run offers.
pub const READ_ONLY_TOOL_NAMES: &[&str] = &["bash", "read", "grep", "glob", "plan"];

/// The native tools whose calls neither change the clone nor depend on
/// each other, so a reply that makes several runs them at the same time.
pub const PARALLEL_TOOL_NAMES: &[&str] = &["read", "grep", "glob"];

/// How many parallel-safe tool calls of one reply run at once.
pub const PARALLEL_TOOL_LIMIT: usize = 8;

/// The name of the tool that runs a helper agent on a sub-task.
pub const SUBAGENT_TOOL: &str = "subagent";

/// Model calls a subagent gets when the call does not say.
pub const SUBAGENT_DEFAULT_STEPS: i32 = 20;

/// Model calls the parent must still have after the subagent's budget so
/// it can read the report and finish.
pub const SUBAGENT_RESERVE: i32 = 4;

/// Seconds of its own wall time the parent keeps back from a subagent.
pub const SUBAGENT_TIME_RESERVE: Duration = Duration::from_secs(60);

/// Characters of a subagent's report the parent reads.
pub const SUBAGENT_REPORT_LIMIT: usize = 12_000;

/// The `subagent` tool; with custom agents (see [`crate::agents`]) it takes
/// an `agent` too.
pub fn subagent_tool(agents: &[AgentDef]) -> Tool {
    let mut properties = json!({
        "task":{"type":"string","description":"The complete brief for the helper, including what to report back."},
        "max_steps":{"type":"integer","description":"Model calls the helper may spend (capped by what remains of this run)."}
    });
    if !agents.is_empty() {
        let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
        properties["agent"] = json!({
            "type":"string",
            "enum": names,
            "description":"A custom agent to run the sub-task as; leave it out for a plain helper."
        });
    }
    Tool {
        name: SUBAGENT_TOOL.to_string(),
        description: format!(
            "Delegate one self-contained sub-task to a helper agent with the same tools in the same clone; it works on its own and returns a report. Give it everything it needs in task (goal, files, what to report back): it does not see this conversation. Use it for parallel-free, bounded work such as surveying part of the code or running and summarising a long check. Default budget: {SUBAGENT_DEFAULT_STEPS} model calls.{}",
            describe_for_tool(agents)
        ),
        parameters: json!({"type":"object","properties":properties,"required":["task"]}),
    }
}

/// Paths a `glob` call lists at most; the rest is counted.
pub const GLOB_RESULT_LIMIT: usize = 500;

/// MCP tools advertised with their full schema on every model call. Past
/// this many, the run advertises `mcp_search` instead and loads the tools
/// the model looks up, so one catalogue-sized server does not take the
/// context of every call.
pub const MCP_INLINE_TOOL_LIMIT: usize = 32;

/// The name of the lookup tool a run with many MCP tools offers.
pub const MCP_SEARCH_TOOL: &str = "mcp_search";

/// Prefix of every MCP tool name the model sees: `mcp__<server>__<tool>`.
pub const MCP_TOOL_PREFIX: &str = "mcp__";

pub use crate::instructions::{
    INSTRUCTION_FILE_LIMIT, INSTRUCTION_FILES, INSTRUCTIONS_LIMIT, Instructions, PathRules,
    load_instructions,
};

pub const DEFAULT_DENY: &[&str] = &[
    r"(?i)(^|[;&|]\s*)git\s+push\b",
    r"(?i)(^|[;&|]\s*)git\s+remote\s+(add|set-url|remove)\b",
    r"(?i)(^|[;&|]\s*)gh\s+",
    // Credential paths: the pulse home's private directory under any spelling
    // (`~/.pulse/private`, `/Users/x/.pulse/private`, `$PULSE_HOME/private`,
    // an isolated measurement home's `private/providers`), never the macOS
    // filesystem root `/private/` that `/tmp` and `/var` resolve to.
    r#"(^|[\s"'`])(~/.ssh/|\.netrc\b|~/.copilot\b|~/.config/gh\b)|\.pulse/private\b|PULSE_HOME\}?/private\b|/private/(providers|heldout)\b"#,
];

pub use DEFAULT_DENY as DefaultDeny;

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub model: String,
    pub task: String,
    pub system_prompt: String,
    pub system_prompt_template: String,
    pub cwd: String,
    pub env: Vec<String>,
    pub max_steps: i32,
    pub step_timeout: Duration,
    /// Deadline for a command that runs `git commit`/`merge`/`rebase`/
    /// `cherry-pick`/`am` in a clone whose pre-commit hook is the runner's
    /// verify gate; zero means [`DEFAULT_COMMIT_TIMEOUT`]. Capped by
    /// `wall_time` when set; never shorter than `step_timeout`.
    pub commit_timeout: Duration,
    pub wall_time: Duration,
    pub max_cost: f64,
    pub output_limit: usize,
    pub keep_observations: usize,
    pub context_window: i32,
    pub max_format_errors: i32,
    pub format_retries: i32,
    pub deny: Vec<String>,
    pub provider: String,
    pub cancel_file: String,
    pub cancel_reason: String,
    /// Repository instructions (AGENTS.md and friends, see
    /// [`load_instructions`]) rendered into the system prompt; empty when
    /// the clone has none or the owner turned them off.
    pub instructions: String,
    /// What earlier runs remembered (see [`crate::memory`]), rendered into
    /// the system prompt where the template's `{{.Memory}}` key stands;
    /// empty when nothing is remembered or memory is off.
    pub memory: String,
    /// Whether the run offers the `subagent` tool.
    pub subagents: bool,
    /// How many parents this run has: 0 for a run the owner started, 1 for
    /// its subagents. A subagent never offers `subagent` itself.
    pub depth: u8,
    /// Percent of the context window the prompt may use before the older
    /// part of the conversation is compacted into a summary; 0 is off.
    pub compact_at: i32,
    /// A run that may look but not touch: `edit` and `write` are not
    /// offered, commands that would change the clone are refused, and a
    /// command that changes it anyway is reverted. Set for `answer` and
    /// `ballot` jobs, whose findings are the whole deliverable.
    pub read_only: bool,
    /// Reasoning effort to ask the model for (`minimal`..`xhigh`, see
    /// [`pulse_provider::REASONING_EFFORTS`]); empty leaves the vendor's
    /// default in place. Providers without the knob ignore it.
    pub reasoning: String,
    /// An interactive session (`pulse harness chat`) rather than a job: the
    /// conversation goes on turn by turn through [`Agent::turn`], a reply
    /// without a tool call is the answer to the user instead of a report,
    /// the step and time budgets apply per turn, and the prompt is
    /// [`CHAT_PROMPT_TEMPLATE`].
    pub interactive: bool,
    /// Teacher mode (`--teach`, `harness.teach on`): the agent still does
    /// the work, but explains the why of each design or coding choice as it
    /// goes - what it chose, the alternative it passed over, the reason - so a
    /// junior reading along learns from it. A job reports the lessons as
    /// `LESSON:` lines; the template's `{{.Teach}}` key places the paragraph
    /// ([`teach_paragraph`]).
    pub teach: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepKind {
    Assistant,
    Command,
    Read,
    Grep,
    Glob,
    Edit,
    Write,
    Plan,
    Mcp,
    Skill,
    Fetch,
    Subagent,
    Hook,
    /// A `process` tool call (output, wait, stop, list of a background
    /// process).
    Process,
    /// A `memory` tool call (remember, forget, list).
    Memory,
    /// A `web_search` tool call.
    Search,
    Observation,
    Note,
    FormatRetry,
    Error,
    Exit,
}

pub const STEP_ASSISTANT: StepKind = StepKind::Assistant;
pub const STEP_COMMAND: StepKind = StepKind::Command;
pub const STEP_EDIT: StepKind = StepKind::Edit;
pub const STEP_OBSERVATION: StepKind = StepKind::Observation;
pub const STEP_FORMAT_RETRY: StepKind = StepKind::FormatRetry;
pub const STEP_GREP: StepKind = StepKind::Grep;
pub const STEP_GLOB: StepKind = StepKind::Glob;
pub const STEP_WRITE: StepKind = StepKind::Write;
pub const STEP_PLAN: StepKind = StepKind::Plan;
pub const STEP_MCP: StepKind = StepKind::Mcp;
pub const STEP_SKILL: StepKind = StepKind::Skill;
pub const STEP_FETCH: StepKind = StepKind::Fetch;
pub const STEP_SUBAGENT: StepKind = StepKind::Subagent;
pub const STEP_HOOK: StepKind = StepKind::Hook;
pub const STEP_NOTE: StepKind = StepKind::Note;
pub const STEP_READ: StepKind = StepKind::Read;
pub const STEP_ERROR: StepKind = StepKind::Error;
pub const STEP_EXIT: StepKind = StepKind::Exit;

pub use STEP_ASSISTANT as StepAssistant;
pub use STEP_COMMAND as StepCommand;
pub use STEP_EDIT as StepEdit;
pub use STEP_ERROR as StepError;
pub use STEP_EXIT as StepExit;
pub use STEP_FORMAT_RETRY as StepFormatRetry;
pub use STEP_GREP as StepGrep;
pub use STEP_NOTE as StepNote;
pub use STEP_OBSERVATION as StepObservation;
pub use STEP_READ as StepRead;

impl std::fmt::Display for StepKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Assistant => "assistant",
            Self::Command => "command",
            Self::Read => "read",
            Self::Grep => "grep",
            Self::Glob => "glob",
            Self::Edit => "edit",
            Self::Write => "write",
            Self::Plan => "plan",
            Self::Mcp => "mcp",
            Self::Skill => "skill",
            Self::Fetch => "fetch",
            Self::Subagent => "subagent",
            Self::Hook => "hook",
            Self::Process => "process",
            Self::Memory => "memory",
            Self::Search => "search",
            Self::Observation => "observation",
            Self::Note => "note",
            Self::FormatRetry => "format_retry",
            Self::Error => "error",
            Self::Exit => "exit",
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Step {
    pub n: i32,
    pub at: String,
    pub kind: StepKind,
    pub content: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    /// What a command does, in the agent's own words (the bash tool's
    /// `description`); shown to the owner in place of the command line.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub duration_ms: i64,
    pub usage: Usage,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// Provider finish reason of an assistant step (`stop`, `tool_calls`,
    /// `length`, ...); None on every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    /// Number of choices the provider returned for an assistant step; None
    /// on other kinds or when the provider did not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choices: Option<i64>,
    /// How deep the step is: 0 for the run itself, 1 for a subagent's step.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub depth: u8,
}

fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

impl Default for Step {
    fn default() -> Self {
        Self {
            n: 0,
            at: String::new(),
            kind: StepKind::Note,
            content: String::new(),
            command: String::new(),
            title: String::new(),
            exit_code: None,
            duration_ms: 0,
            usage: Usage::default(),
            model: String::new(),
            finish_reason: None,
            choices: None,
            depth: 0,
        }
    }
}

fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}

pub trait Sink: Send {
    fn step(&mut self, step: Step);
}

pub struct SinkFunc<F: FnMut(Step) + Send>(pub F);

impl<F: FnMut(Step) + Send> Sink for SinkFunc<F> {
    fn step(&mut self, step: Step) {
        (self.0)(step);
    }
}

pub const EXIT_SUBMITTED: &str = "submitted";
pub const EXIT_STEP_LIMIT: &str = "step_limit";
pub const EXIT_TIME_LIMIT: &str = "time_limit";
pub const EXIT_COST_LIMIT: &str = "cost_limit";
pub const EXIT_FORMAT_ERROR: &str = "format_error";
pub const EXIT_PROVIDER_ERROR: &str = "provider_error";
pub const EXIT_NOT_LOGGED_IN: &str = "not_logged_in";
pub const EXIT_CANCELLED: &str = "cancelled";
/// The run's working directory disappeared under it (a swept clone, a
/// removed worktree): every further command would fail the same way, so the
/// run ends at once instead of spending its budget on `os error 2`.
pub const EXIT_SANDBOX_LOST: &str = "sandbox_lost";
/// The runner asked the run to hold (it restarts for an update): the run
/// stopped at the next safe point with its trajectory written, and the
/// job's next run carries on from it (`pulse harness run --resume ID`).
pub const EXIT_HELD: &str = pulse_store::HARNESS_HELD;
/// The process exit code of a held run: EX_TEMPFAIL, "try again later".
pub const HELD_EXIT_CODE: i32 = pulse_store::HARNESS_HELD_EXIT_CODE;
/// The error text of a held run.
pub const HELD_REASON: &str = "held: the runner restarts and the next run carries on from here";

pub use EXIT_CANCELLED as ExitCancelled;
pub use EXIT_COST_LIMIT as ExitCostLimit;
pub use EXIT_FORMAT_ERROR as ExitFormatError;
pub use EXIT_NOT_LOGGED_IN as ExitNotLoggedIn;
pub use EXIT_PROVIDER_ERROR as ExitProviderError;
pub use EXIT_SANDBOX_LOST as ExitSandboxLost;
pub use EXIT_STEP_LIMIT as ExitStepLimit;
pub use EXIT_SUBMITTED as ExitSubmitted;
pub use EXIT_TIME_LIMIT as ExitTimeLimit;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ResultSummary {
    pub exit_status: String,
    pub submission: String,
    pub steps: i32,
    pub calls: i32,
    pub usage: Usage,
    pub format_errors: i32,
    /// Times the older conversation was compacted into a summary.
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub compactions: i32,
    /// Subagent runs the run started.
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub subagents: i32,
    #[serde(skip)]
    pub elapsed: Duration,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}

pub type Result = ResultSummary;

/// Where streamed reply text goes; see [`Agent::set_stream`]. Any
/// `FnMut(&str) + Send` is a sink that ignores restarts.
pub trait StreamSink: Send {
    /// A piece of the reply, as it arrives.
    fn text(&mut self, text: &str);
    /// The reply starts over: the stream broke part way and the model is
    /// being asked again, so what was shown for this reply is stale.
    fn restart(&mut self) {}
}

impl<F: FnMut(&str) + Send> StreamSink for F {
    fn text(&mut self, text: &str) {
        self(text)
    }
}

/// How many times a reply whose stream broke part way is asked for
/// again before the turn stops.
pub const STREAM_RETRIES: u32 = 2;

/// The most of a bash call's `description` kept as the step's title.
const TITLE_LIMIT: usize = 120;

#[derive(Clone, Debug, Default)]
pub struct Observation {
    pub output: String,
    pub exit_code: i32,
    pub timed_out: bool,
    pub duration: Duration,
}

pub trait Environment: Send {
    fn execute(&mut self, cancel: &CancelToken, command: &str, timeout: Duration) -> Observation;
    fn info(&self) -> BTreeMap<String, String>;
    /// Called once when the run ends, whatever the exit status: the
    /// environment ends whatever the commands left running and reports it.
    fn close(&mut self) -> Vec<pulse_cli::procs::KilledProc> {
        Vec::new()
    }
    /// A second environment on the same working directory for a subagent;
    /// None when the environment cannot be shared, which turns the
    /// `subagent` tool off.
    fn fork(&self) -> Option<Box<dyn Environment>> {
        None
    }
    /// Whether [`Environment::spawn`] works: `bash` then offers
    /// `background: true` and the `process` tool is on.
    fn offers_background(&self) -> bool {
        false
    }
    /// Starts a command that outlives the call; the harness reads, waits
    /// for and stops it through the returned [`Process`].
    fn spawn(&mut self, command: &str) -> std::result::Result<Box<dyn Process>, String> {
        let _ = command;
        Err("this environment cannot keep a process running".to_string())
    }
}

pub struct Agent {
    p: Arc<dyn Provider>,
    env: Box<dyn Environment>,
    pub cfg: Config,
    pub(crate) sinks: Vec<Box<dyn Sink>>,
    deny: Vec<DenyRule>,
    msgs: Vec<Message>,
    steps: Vec<Step>,
    next_n: i32,
    ran: bool,
    result: ResultSummary,
    started: Instant,
    cancel_reason: String,
    /// Raised (with the cancel token) when the runner asks the run to hold:
    /// the run ends `held` instead of `cancelled` wherever it notices.
    hold: Arc<AtomicBool>,
    /// The message a resumed job run starts with instead of its task: the
    /// task is already the first user message of the carried-over
    /// conversation ([`Agent::resume_run`]).
    continuation: Option<String>,
    /// Set by a command step when the run cannot go on (the working
    /// directory is gone); the loop ends the run with it before the next
    /// model call.
    fatal: Option<(&'static str, String)>,
    /// The MCP servers this run may call, as tools.
    mcp: McpTools,
    /// Notes the caller left before the run (an MCP server that did not
    /// answer, the instruction files read); emitted as the first steps.
    start_notes: Vec<String>,
    /// The skills the `skill` tool can load; empty means no tool.
    skills: Vec<Skill>,
    /// The owner's and the clone's hooks.
    hooks: HookSet,
    /// Memory across runs, when the owner keeps it on (`memory` tool).
    memory: Option<Memory>,
    /// What each turn of a session changed, for `/rewind`.
    rewind: Rewind,
    /// Instruction files that wait for the paths they speak of; None
    /// when instruction files are off.
    path_rules: Option<PathRules>,
    /// The custom agents the `subagent` tool can run a helper as.
    agents: Vec<AgentDef>,
    /// The hosts `fetch` may read; empty means no tool.
    fetch: FetchPolicy,
    web_search: WebSearch,
    /// The background processes the model started and has not stopped.
    procs: Procs,
    /// The permission rules in force: what runs without asking, what is
    /// refused outright.
    permissions: Rules,
    /// What the call being run is about - the command, the paths, the
    /// host, the arguments - for the allow rules at approval time.
    current_subjects: Vec<String>,
    /// The person at the keyboard of an interactive session: approves the
    /// calls that change things and answers the `question` tool. None in
    /// a job.
    ui: Option<Arc<Mutex<dyn Ui>>>,
    /// Where the text of a reply goes as it streams in, when the provider
    /// streams; the Assistant step still carries the whole text.
    stream: Option<Box<dyn StreamSink>>,
    /// The loop's state, kept across the turns of an interactive session.
    state: LoopState,
    /// Set once [`Agent::finish`] ran: the end hooks are done and the
    /// environment is closed.
    ended: bool,
}

/// What the loop carries from one model call to the next; a field of the
/// agent so an interactive session keeps it between turns.
#[derive(Debug, Default)]
struct LoopState {
    initiator: String,
    counted_format_errors: i32,
    used_format_retries: i32,
    keep_observations: usize,
    last_prompt_tokens: i32,
    last_prune_level: i32,
    budget_notice_sent: bool,
    emitted_prune_notes: BTreeSet<String>,
    calls_at_compaction: i32,
    /// `result.calls` when the current turn began; the step budget of an
    /// interactive session counts from here.
    turn_calls_start: i32,
    /// When the current turn began; the time budget of an interactive
    /// session counts from here.
    turn_started: Option<Instant>,
    /// Set once a report that arrived before any tool call was sent back
    /// for one more turn; the second such report is accepted so no loop
    /// forms.
    evidence_free_report_bounced: bool,
    /// Set once the assistant issued any tool call or action block, so a
    /// later report is backed by at least an attempt at evidence.
    tool_call_seen: bool,
}

/// How a pass of the loop ended.
enum Drive {
    /// The model replied without a tool call: a job's report, a turn's
    /// answer.
    Answered(String),
    /// A budget, a cancellation, a provider error or a lost working
    /// directory stopped it.
    Stopped { status: &'static str, error: String },
}

/// The outcome of one [`Agent::turn`].
#[derive(Clone, Debug)]
pub enum Turn {
    /// The model's answer to the user; the turn is over.
    Reply(String),
    /// A `user_prompt` hook kept the message from being sent (exit 2);
    /// the reason is what it printed. Nothing reached the model.
    Blocked(String),
    /// The turn stopped short (a budget, an interruption, a provider error):
    /// the session goes on and the user may say continue.
    Stopped { status: String, error: String },
    /// The session cannot go on (the working directory is gone); it has
    /// been ended.
    Ended(ResultSummary),
}

/// The person at the keyboard of an interactive session.
pub trait Ui: Send {
    /// Whether a call that changes things may run. `Approval::Deny` carries
    /// the user's reason, which the model is told.
    fn approve(&mut self, req: &ApprovalRequest) -> Approval;
    /// The user's answer to the model's question, None when nobody can
    /// answer (stdin closed).
    fn ask(&mut self, question: &str, choices: &[String]) -> Option<String>;
}

/// A call the model wants to make that changes the checkout or leaves the
/// machine, shown to the user before it runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApprovalRequest {
    /// The tool: `bash`, `edit`, `write`, or an MCP tool's name.
    pub tool: String,
    /// The command, the path, or the MCP call in one line.
    pub summary: String,
    /// What it would do: the edit blocks, the file's head, the arguments.
    pub detail: String,
    /// Why it needs a look: what the command changes, or the policy it
    /// crosses.
    pub reason: String,
    /// The permission rule `Approval::Always` would add, so the prompt can
    /// show it: `bash(git commit *)`, `edit`, `fetch(docs.rs)`.
    pub always: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Approval {
    Allow,
    /// Allow this call and every later call matching the request's
    /// `always` pattern for the rest of the session.
    Always,
    Deny(String),
}

/// The name of the tool an interactive session offers for asking the user.
pub const QUESTION_TOOL: &str = "question";

/// The status an interactive session ends with.
pub const EXIT_ENDED: &str = "ended";

pub fn question_tool() -> Tool {
    Tool {
        name: QUESTION_TOOL.into(),
        description: "Ask the user one question and wait for the answer. Use it when a decision is theirs: which of several approaches, whether to go further, a detail the request leaves out. Give short choices when they exist; the user may also answer freely.".into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "question": {"type": "string", "description": "the question, one or two sentences"},
                "choices": {"type": "array", "items": {"type": "string"}, "description": "short answers to pick from, if any"}
            },
            "required": ["question"]
        }),
    }
}

/// One call to an MCP tool, from the harness's side: the client behind
/// `pulse mcp` in a run, a fake in tests.
pub trait McpBackend: Send {
    fn call_tool(&mut self, name: &str, arguments: Value) -> anyhow::Result<McpToolResult>;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpToolResult {
    pub is_error: bool,
    pub text: String,
}

impl McpBackend for pulse_mcp::Client {
    fn call_tool(&mut self, name: &str, arguments: Value) -> anyhow::Result<McpToolResult> {
        let r = pulse_mcp::Client::call_tool(self, name, arguments)?;
        Ok(McpToolResult {
            is_error: r.is_error,
            text: r.text,
        })
    }
}

/// A connected MCP server and the tools it listed, ready for
/// [`Agent::add_mcp_server`].
pub struct McpServerTools {
    /// The server's registry name (`pulse mcp add <name>`).
    pub server: String,
    /// The tools as the server lists them: remote name, description, input
    /// schema.
    pub tools: Vec<pulse_mcp::ToolInfo>,
    pub backend: Box<dyn McpBackend>,
}

struct McpToolEntry {
    server: usize,
    remote: String,
    tool: Tool,
}

#[derive(Default)]
struct McpTools {
    servers: Vec<McpServerTools>,
    /// Exposed name (`mcp__server__tool`) to its entry.
    table: BTreeMap<String, McpToolEntry>,
    /// Tools the model looked up through `mcp_search`; advertised with
    /// their schema while the catalogue is deferred.
    loaded: BTreeSet<String>,
}

impl McpTools {
    fn deferred(&self) -> bool {
        self.table.len() > MCP_INLINE_TOOL_LIMIT
    }

    fn advertised(&self) -> Vec<Tool> {
        if !self.deferred() {
            return self.table.values().map(|e| e.tool.clone()).collect();
        }
        let mut out = vec![self.search_tool()];
        out.extend(
            self.loaded
                .iter()
                .filter_map(|n| self.table.get(n))
                .map(|e| e.tool.clone()),
        );
        out
    }

    fn search_tool(&self) -> Tool {
        let servers = self
            .servers
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let n = self.table.values().filter(|e| e.server == i).count();
                format!("{} ({n} tools)", s.server)
            })
            .collect::<Vec<_>>()
            .join(", ");
        Tool {
            name: MCP_SEARCH_TOOL.to_string(),
            description: format!(
                "Find tools of the connected MCP servers - {servers} - by keywords in their name or description. The matches come back with their schema and become callable tools from your next reply on."
            ),
            parameters: json!({"type":"object","properties":{"query":{"type":"string","description":"Words to look for; a server name narrows to that server."}},"required":["query"]}),
        }
    }

    /// Tools matching `query`; the matches become advertised.
    fn search(&mut self, query: &str) -> String {
        let words: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 1)
            .map(|w| w.to_ascii_lowercase())
            .collect();
        if words.is_empty() {
            return "mcp_search: give one or more words to look for".to_string();
        }
        let mut scored: Vec<(usize, &String, &McpToolEntry)> = self
            .table
            .iter()
            .filter_map(|(name, e)| {
                let hay = format!(
                    "{} {} {}",
                    name.to_ascii_lowercase(),
                    self.servers[e.server].server.to_ascii_lowercase(),
                    e.tool.description.to_ascii_lowercase()
                );
                let score = words.iter().filter(|w| hay.contains(w.as_str())).count();
                (score > 0).then_some((score, name, e))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        scored.truncate(10);
        if scored.is_empty() {
            return format!(
                "mcp_search: no tool matches {query:?}; try other words or a server name"
            );
        }
        let mut out = String::new();
        let names: Vec<String> = scored.iter().map(|(_, n, _)| (*n).clone()).collect();
        for (_, name, e) in &scored {
            out.push_str(&format!(
                "{name}: {}\n  parameters: {}\n",
                one_line(&e.tool.description, 300),
                compact_json(&e.tool.parameters, 600)
            ));
        }
        self.loaded.extend(names);
        out.push_str("These tools are callable from your next reply.");
        out
    }

    fn has(&self, name: &str) -> bool {
        self.table.contains_key(name)
    }

    fn call(&mut self, name: &str, arguments: Value) -> anyhow::Result<McpToolResult> {
        let Some(entry) = self.table.get(name) else {
            return Err(anyhow!("unknown MCP tool {name}"));
        };
        let (server, remote) = (entry.server, entry.remote.clone());
        self.servers[server].backend.call_tool(&remote, arguments)
    }

    fn add(&mut self, server: McpServerTools) {
        let idx = self.servers.len();
        let server_slug = tool_name_slug(&server.server);
        for info in &server.tools {
            let mut name = format!(
                "{MCP_TOOL_PREFIX}{server_slug}__{}",
                tool_name_slug(&info.name)
            );
            name.truncate(60);
            let base = name.clone();
            let mut n = 2;
            while self.table.contains_key(&name) {
                name = format!("{base}_{n}");
                n += 1;
            }
            let parameters = if info.input_schema.is_object() {
                info.input_schema.clone()
            } else {
                json!({"type":"object","properties":{}})
            };
            self.table.insert(
                name.clone(),
                McpToolEntry {
                    server: idx,
                    remote: info.name.clone(),
                    tool: Tool {
                        name,
                        description: if info.description.is_empty() {
                            format!("{} tool of the {} MCP server.", info.name, server.server)
                        } else {
                            info.description.clone()
                        },
                        parameters,
                    },
                },
            );
        }
        self.servers.push(server);
    }
}

/// A tool name part the providers accept: letters, digits, `_` and `-`.
fn tool_name_slug(s: &str) -> String {
    let slug: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let slug = slug.trim_matches('_').to_string();
    if slug.is_empty() {
        "tool".to_string()
    } else {
        slug
    }
}

fn compact_json(v: &Value, limit: usize) -> String {
    let s = serde_json::to_string(v).unwrap_or_default();
    if s.len() <= limit {
        s
    } else {
        format!("{}...", &s[..s.floor_char_boundary(limit)])
    }
}

pub const ERR_OWNER_CANCELLED: &str = "cancelled by the owner";
pub const ERR_HARNESS_PROVIDER_NIL: &str = "harness provider is nil";
pub const ERR_HARNESS_ENVIRONMENT_NIL: &str = "harness environment is nil";
pub use ERR_OWNER_CANCELLED as ErrOwnerCancelled;

struct DenyRule {
    description: String,
    re: Regex,
}

pub fn new(
    p: Box<dyn Provider>,
    env: Box<dyn Environment>,
    cfg: Config,
    sinks: Vec<Box<dyn Sink>>,
) -> Agent {
    new_shared(Arc::from(p), env, cfg, sinks)
}

/// [`new`] with a provider that is shared with a subagent.
pub fn new_shared(
    p: Arc<dyn Provider>,
    env: Box<dyn Environment>,
    cfg: Config,
    sinks: Vec<Box<dyn Sink>>,
) -> Agent {
    let rewind = Rewind::new(&cfg.cwd);
    Agent {
        p,
        env,
        cfg,
        sinks,
        deny: Vec::new(),
        msgs: Vec::new(),
        steps: Vec::new(),
        next_n: 0,
        ran: false,
        result: ResultSummary::default(),
        started: Instant::now(),
        cancel_reason: String::new(),
        hold: Arc::new(AtomicBool::new(false)),
        continuation: None,
        fatal: None,
        mcp: McpTools::default(),
        start_notes: Vec::new(),
        skills: Vec::new(),
        hooks: HookSet::default(),
        memory: None,
        rewind,
        path_rules: None,
        agents: Vec::new(),
        fetch: FetchPolicy::default(),
        web_search: WebSearch::default(),
        procs: Procs::default(),
        permissions: Rules::new(),
        current_subjects: Vec::new(),
        ui: None,
        stream: None,
        state: LoopState::default(),
        ended: false,
    }
}

pub use new as New;

impl Agent {
    pub fn run(&mut self) -> anyhow::Result<ResultSummary> {
        self.run_with_cancel(&CancelToken::new())
    }

    /// Offers a connected MCP server's tools to the model as
    /// `mcp__<server>__<tool>`; call before `run`.
    pub fn add_mcp_server(&mut self, server: McpServerTools) {
        self.mcp.add(server);
    }

    /// Leaves a note the run emits as one of its first steps.
    pub fn note_at_start(&mut self, text: impl Into<String>) {
        self.start_notes.push(text.into());
    }

    /// Offers these skills through the `skill` tool; call before `run`.
    pub fn set_skills(&mut self, skills: Vec<Skill>) {
        self.skills = skills;
    }

    /// The custom agents the model can run a helper as (`subagent` with
    /// `agent`); each brings its own instructions and tool list.
    pub fn set_agents(&mut self, agents: Vec<AgentDef>) {
        self.agents = agents;
    }

    pub fn agents(&self) -> &[AgentDef] {
        &self.agents
    }

    /// Runs these hooks around the run and its tool calls; call before `run`.
    /// Gives the run its memory files: the `memory` tool is offered (not
    /// in a read-only run) and `/memory` in a session reads them.
    pub fn set_memory(&mut self, memory: Memory) {
        self.memory = Some(memory);
    }

    /// The run's memory files, if the owner keeps memory on.
    pub fn memory(&self) -> Option<&Memory> {
        self.memory.as_ref()
    }

    /// Whether the `memory` tool is offered: memory is on and the run may
    /// leave something behind (a read-only run may not).
    pub fn offers_memory(&self) -> bool {
        self.memory.is_some() && !self.cfg.read_only
    }

    /// Hands the model the instruction files that name paths (see
    /// [`crate::instructions`]) as it first touches a file they apply to.
    pub fn set_path_rules(&mut self, rules: Option<PathRules>) {
        self.path_rules = rules;
    }

    /// The instruction files that speak of a file `call` read or changed
    /// and were not given yet: each becomes a note step and is appended
    /// to `text`, the model's view of the call's result.
    fn with_path_instructions(&mut self, call: &NativeCall, text: String) -> String {
        if self.path_rules.is_none() {
            return text;
        }
        let rels: Vec<String> = call
            .files()
            .iter()
            .filter_map(|p| self.fs().resolve_action_path(p, true).ok().map(|r| r.rel))
            .collect();
        let Some(rules) = self.path_rules.as_mut() else {
            return text;
        };
        let attached: Vec<(String, crate::instructions::Attached)> = rels
            .iter()
            .flat_map(|rel| rules.attach(rel).into_iter().map(move |a| (rel.clone(), a)))
            .collect();
        let mut text = text;
        for (rel, a) in attached {
            self.emit(Step {
                kind: StepKind::Note,
                content: format!(
                    "instructions for {rel} from {} ({}; {} bytes)",
                    a.source,
                    a.reason,
                    a.body.len()
                ),
                command: a.source.clone(),
                ..Step::default()
            });
            text.push_str(&format!(
                "\n\n[Instructions from {} ({}). They apply to {rel} and the other paths they name; follow them there.]\n{}",
                a.source, a.reason, a.body
            ));
        }
        text
    }

    /// Where a session keeps its rewind records (`<home>/harness/<id>.rewind`);
    /// without one they live in memory only. Call before the first turn.
    pub fn set_rewind_dir(&mut self, dir: PathBuf) {
        self.rewind.set_dir(dir);
    }

    /// Adds the rewind records an earlier session saved under `dir`, so
    /// its turns can still be rewound after `resume_messages`; the count
    /// loaded. Call after `resume_messages` and before the first turn.
    pub fn load_rewind(&mut self, dir: &Path) -> std::io::Result<usize> {
        let n = self.rewind.load(dir)?;
        // Before `start` the carried-over messages sit at msgs[i]; the
        // system prompt will push them to i + 1.
        let starts: Vec<(usize, String)> = self
            .msgs
            .iter()
            .enumerate()
            .filter(|(_, m)| is_turn_start(m))
            .map(|(i, m)| (i + 1, m.content.clone()))
            .collect();
        self.rewind.match_messages(&starts);
        Ok(n)
    }

    /// The session's turns as `/rewind` lists them.
    pub fn rewind_turns(&self) -> Vec<String> {
        self.rewind.describe()
    }

    /// `/rewind N`: puts the files the turns from `to` (1-based) on
    /// changed back to how they were, drops those turns' messages, or
    /// both; the turn's records go with them. Not during a turn.
    pub fn rewind(
        &mut self,
        to: usize,
        what: rewind::What,
    ) -> std::result::Result<rewind::Report, String> {
        if to == 0 {
            return Err("turns count from 1; /rewind lists them".to_string());
        }
        // Before `start` the system prompt is not in place yet, so
        // carried-over messages sit one below where the records say.
        let offset = usize::from(!self.ran);
        let report = self.rewind.rewind(to - 1, what, self.msgs.len() + offset)?;
        if let Some(index) = report.truncate_to {
            self.msgs.truncate(index.saturating_sub(offset));
            // The compaction and pruning bookkeeping speaks of a
            // conversation that is gone.
            self.state.last_prompt_tokens = 0;
        }
        // The step is the record of it; a chat shows the step.
        self.emit(Step {
            kind: StepKind::Note,
            content: report.describe(to, what),
            ..Step::default()
        });
        Ok(report)
    }

    pub fn set_hooks(&mut self, hooks: HookSet) {
        self.hooks = hooks;
    }

    /// Offers the `fetch` tool for the hosts of `policy`; call before `run`.
    pub fn set_fetch_policy(&mut self, policy: FetchPolicy) {
        self.fetch = policy;
    }

    /// Offers the `web_search` tool through `search`'s engine; call before
    /// `run`. A search that is off offers nothing.
    pub fn set_web_search(&mut self, search: WebSearch) {
        self.web_search = search;
    }

    pub fn offers_web_search(&self) -> bool {
        !self.web_search.is_off()
    }

    /// Sets the permission rules from the flags and settings; call before
    /// `run`. A tool every call of which is denied is not offered at all.
    pub fn set_permissions(&mut self, rules: Rules) {
        self.permissions = rules;
    }

    /// The permission rules in force.
    pub fn permissions(&self) -> &Rules {
        &self.permissions
    }

    /// Adds a rule for the rest of the session (`/permissions allow` in a
    /// session, `a` at an approval).
    pub fn add_permission(&mut self, rule: Rule) {
        self.permissions.push(rule);
    }

    /// Drops the rules added during the session; how many went.
    pub fn clear_session_permissions(&mut self) -> usize {
        self.permissions.clear_session()
    }

    /// Puts a person at the keyboard: the calls that change things are
    /// shown to `ui` before they run and the `question` tool is offered.
    /// Call before the first turn.
    pub fn set_ui(&mut self, ui: Arc<Mutex<dyn Ui>>) {
        self.ui = Some(ui);
    }

    /// Sends the text of each reply to `f` as it streams in from a
    /// provider that streams; the Assistant step still carries it whole.
    /// When the stream breaks part way the reply is asked for again (up
    /// to [`STREAM_RETRIES`] times) and `f` hears
    /// [`StreamSink::restart`] first.
    pub fn set_stream(&mut self, f: Box<dyn StreamSink>) {
        self.stream = Some(f);
    }

    /// Switches the session between looking and acting (`/plan` in a
    /// session): read-only offers no `edit` or `write` and refuses the
    /// commands that change things, from the next model call.
    pub fn set_read_only(&mut self, on: bool) {
        self.cfg.read_only = on;
    }

    /// What the session runs with, one line per item (`/env` in a
    /// session): model, window, budgets, tools, skills, hooks, MCP
    /// servers, fetch hosts and deny rules.
    pub fn environment(&self) -> Vec<String> {
        let cfg = &self.cfg;
        let mut out = vec![
            format!("model: {} ({})", cfg.model, cfg.provider),
            format!(
                "context window: {}; compact at {}%; keep {} observations",
                if cfg.context_window > 0 {
                    cfg.context_window.to_string()
                } else {
                    "unknown".to_string()
                },
                cfg.compact_at,
                cfg.keep_observations
            ),
            format!(
                "budgets: {} model calls and {} min per turn, {} s per step, cost cap {}",
                cfg.max_steps,
                cfg.wall_time.as_secs() / 60,
                cfg.step_timeout.as_secs(),
                if cfg.max_cost > 0.0 {
                    format!("{:.2}", cfg.max_cost)
                } else {
                    "none".to_string()
                }
            ),
            format!("working directory: {}", cfg.cwd),
            format!(
                "mode: {}{}",
                if cfg.read_only {
                    "read-only (plan)"
                } else {
                    "read and write"
                },
                if cfg.reasoning.is_empty() {
                    String::new()
                } else {
                    format!("; reasoning {}", cfg.reasoning)
                }
            ),
            format!("tools: {}", self.tool_names().join(", ")),
        ];
        out.push(if cfg.instructions.trim().is_empty() {
            "instructions: none loaded".to_string()
        } else {
            let files: Vec<&str> = cfg
                .instructions
                .lines()
                .filter_map(|l| l.strip_prefix("--- ").and_then(|l| l.strip_suffix(" ---")))
                .collect();
            format!(
                "instructions: {} bytes from {}",
                cfg.instructions.len(),
                if files.is_empty() {
                    "the brief".to_string()
                } else {
                    files.join(", ")
                }
            )
        });
        out.push(match &self.path_rules {
            Some(r) if !r.is_empty() => format!("path rules: {}", r.describe().join("; ")),
            Some(_) => {
                "path rules: none (an AGENTS.md below the root is read as files there are touched)"
                    .to_string()
            }
            None => "path rules: off".to_string(),
        });
        out.push(if self.skills.is_empty() {
            "skills: none".to_string()
        } else {
            format!(
                "skills: {}",
                self.skills
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
        out.push(if self.agents.is_empty() {
            "agents: none".to_string()
        } else {
            format!(
                "agents: {}",
                self.agents
                    .iter()
                    .map(|a| format!("{} ({})", a.name, a.source))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
        out.push(match &self.memory {
            Some(m) => m.describe(),
            None => "memory: off".to_string(),
        });
        out.push(if self.hooks.is_empty() {
            "hooks: none".to_string()
        } else {
            format!(
                "hooks: {} from {}",
                self.hooks.hooks.len(),
                self.hooks.files.join(", ")
            )
        });
        out.push(if self.mcp.servers.is_empty() {
            "mcp servers: none".to_string()
        } else {
            format!(
                "mcp servers: {}",
                self.mcp
                    .servers
                    .iter()
                    .map(|s| format!("{} ({} tools)", s.server, s.tools.len()))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
        out.push(if self.fetch.is_empty() {
            "fetch: off".to_string()
        } else {
            format!("fetch: {}", self.fetch.patterns.join(", "))
        });
        out.push(format!("web search: {}", self.web_search.describe()));
        out.push(format!(
            "deny rules: {}; subagents: {}",
            cfg.deny.len(),
            if self.offers_subagents() { "on" } else { "off" }
        ));
        out.push(if self.permissions.is_empty() {
            "permissions: none (changes are shown before they run)".to_string()
        } else {
            format!("permissions: {}", self.permissions.describe().join("; "))
        });
        if self.offers_background() {
            let running = self.procs.unfinished();
            out.push(match (running, self.procs.len()) {
                (_, 0) => {
                    "background processes: none (bash background:true starts one)".to_string()
                }
                (r, n) => format!("background processes: {r} running, {n} listed (/procs)"),
            });
        }
        out
    }

    fn offers_subagents(&self) -> bool {
        self.cfg.subagents && self.cfg.depth == 0
    }

    fn offers_question(&self) -> bool {
        self.ui.is_some() && self.cfg.depth == 0
    }

    /// Whether `bash` takes `background: true` and `process` is offered:
    /// the environment must be able to keep a process running, and a
    /// read-only run runs its commands to completion only (a change a
    /// background process made later could not be put back).
    pub fn offers_background(&self) -> bool {
        self.env.offers_background() && !self.cfg.read_only
    }

    /// The background processes of this run (`/procs` in a session).
    pub fn procs(&mut self) -> &mut Procs {
        &mut self.procs
    }

    /// The tools the next model call offers.
    pub fn tools(&self) -> Vec<Tool> {
        let mut out = native_tools_with(self.cfg.read_only, self.offers_background());
        if self.offers_background() {
            out.push(process_tool());
        }
        if self.offers_memory() {
            out.push(memory_tool());
        }
        if !self.skills.is_empty() {
            out.push(skill_tool(&self.skills));
        }
        if !self.fetch.is_empty() {
            out.push(fetch_tool(&self.fetch));
        }
        if self.offers_web_search() {
            out.push(web_search_tool(&self.web_search));
        }
        if self.offers_subagents() {
            out.push(subagent_tool(&self.agents));
        }
        if self.offers_question() {
            out.push(question_tool());
        }
        out.extend(self.mcp.advertised());
        out.retain(|t| !self.permissions.withdraws(&t.name));
        out
    }

    fn tool_names(&self) -> Vec<String> {
        let native = if self.cfg.read_only {
            READ_ONLY_TOOL_NAMES
        } else {
            NATIVE_TOOL_NAMES
        };
        let mut out: Vec<String> = native.iter().map(|s| (*s).to_string()).collect();
        if self.offers_background() {
            out.push(PROCESS_TOOL.to_string());
        }
        if self.offers_memory() {
            out.push(MEMORY_TOOL.to_string());
        }
        if !self.skills.is_empty() {
            out.push(SKILL_TOOL.to_string());
        }
        if !self.fetch.is_empty() {
            out.push(FETCH_TOOL.to_string());
        }
        if self.offers_web_search() {
            out.push(WEB_SEARCH_TOOL.to_string());
        }
        if self.offers_subagents() {
            out.push(SUBAGENT_TOOL.to_string());
        }
        if self.offers_question() {
            out.push(QUESTION_TOOL.to_string());
        }
        if self.mcp.deferred() {
            out.push(MCP_SEARCH_TOOL.to_string());
        } else {
            out.extend(self.mcp.table.keys().cloned());
        }
        out.retain(|n| !self.permissions.withdraws(n));
        out
    }

    pub fn run_with_cancel(&mut self, cancel: &CancelToken) -> anyhow::Result<ResultSummary> {
        if self.ran {
            return Err(anyhow!("harness agent can only run once"));
        }
        if self.cfg.task.trim().is_empty() {
            return Err(anyhow!("harness task is empty"));
        }
        self.start(cancel)?;
        let text = self
            .continuation
            .take()
            .unwrap_or_else(|| self.cfg.task.clone());
        self.push_user(text);
        Ok(match self.drive(cancel) {
            Drive::Answered(submission) => self.finish(EXIT_SUBMITTED, &submission, ""),
            Drive::Stopped { status, error } => self.finish(status, "", &error),
        })
    }

    /// One turn of an interactive session: `text` is the user's message.
    /// The loop runs until the model replies without a tool call (the
    /// answer), a budget or the user stops it, or the working directory
    /// is gone. The first turn sets the session up (prompt, hooks, notes).
    pub fn turn(&mut self, cancel: &CancelToken, text: &str) -> anyhow::Result<Turn> {
        self.turn_with_images(cancel, text, Vec::new())
    }

    /// [`turn`](Self::turn) with images on the user's message: they ride
    /// the message to the model (`Message.images`, which each provider maps
    /// to its own shape or ignores) and stay in the trajectory with it.
    pub fn turn_with_images(
        &mut self,
        cancel: &CancelToken,
        text: &str,
        images: Vec<ImageContent>,
    ) -> anyhow::Result<Turn> {
        if !self.cfg.interactive {
            return Err(anyhow!(
                "turn needs an interactive agent (Config.interactive)"
            ));
        }
        if self.ended {
            return Err(anyhow!("the session has ended"));
        }
        if text.trim().is_empty() {
            return Err(anyhow!("turn text is empty"));
        }
        if !self.ran {
            if self.cfg.task.trim().is_empty() {
                // Compaction summarises against the task; the first message
                // is the nearest thing a session has.
                self.cfg.task = text.to_string();
            }
            self.start(cancel)?;
        }
        let text = match self.user_prompt_hooks(cancel, text) {
            Ok(text) => text,
            Err(reason) => return Ok(Turn::Blocked(reason)),
        };
        self.state.turn_calls_start = self.result.calls;
        self.state.turn_started = Some(Instant::now());
        self.state.budget_notice_sent = false;
        self.state.initiator = "user".to_string();
        self.state.counted_format_errors = 0;
        self.state.used_format_retries = 0;
        self.rewind.begin_turn(&text, self.msgs.len());
        self.push_user_with_images(text, images);
        let drove = self.drive(cancel);
        if let Err(e) = self.rewind.end_turn() {
            self.emit(Step {
                kind: StepKind::Note,
                content: format!("rewind records not saved: {e}"),
                ..Step::default()
            });
        }
        Ok(match drove {
            Drive::Answered(reply) => Turn::Reply(reply),
            Drive::Stopped { status, error } if status == EXIT_SANDBOX_LOST => {
                Turn::Ended(self.finish(status, "", &error))
            }
            Drive::Stopped { status, error } => {
                let content = if error.is_empty() {
                    format!("turn stopped: {status}")
                } else {
                    format!("turn stopped: {status}: {error}")
                };
                self.emit(Step {
                    kind: StepKind::Note,
                    content,
                    ..Step::default()
                });
                Turn::Stopped {
                    status: status.to_string(),
                    error,
                }
            }
        })
    }

    /// Ends an interactive session: the end hooks run, the environment
    /// closes and the `exit` step is emitted with `status`
    /// ([`EXIT_ENDED`] when the user left). A session that never started
    /// (no turn) ends without hooks.
    pub fn end(&mut self, status: &str) -> ResultSummary {
        if self.ended {
            return self.result.clone();
        }
        if !self.ran {
            self.ended = true;
            self.result.exit_status = status.to_string();
            return self.result.clone();
        }
        self.finish(status, "", "")
    }

    /// Whether a turn is still possible.
    pub fn is_ended(&self) -> bool {
        self.ended
    }

    /// Adds the whole conversation of an earlier session before the first
    /// turn: everything after its system prompt, which is rendered anew.
    /// A second call appends.
    pub fn resume_messages(&mut self, msgs: Vec<Message>) -> anyhow::Result<()> {
        if self.ran {
            return Err(anyhow!("resume_messages must come before the first turn"));
        }
        let msgs = trim_dangling_tool_calls(msgs);
        let first_user = msgs
            .iter()
            .find(|m| matches!(m.role, Role::User))
            .map(|m| m.content.clone());
        if self.cfg.task.trim().is_empty()
            && let Some(task) = first_user
        {
            self.cfg.task = task;
        }
        self.msgs
            .extend(msgs.into_iter().filter(|m| !matches!(m.role, Role::System)));
        Ok(())
    }

    /// Carries a held job run on: the earlier session's conversation comes
    /// first (its first user message is the task, which compaction and the
    /// prompt keep), and the run opens with `continuation` instead of
    /// repeating the task. Before the first turn only.
    pub fn resume_run(&mut self, msgs: Vec<Message>, continuation: &str) -> anyhow::Result<()> {
        if self.ran {
            return Err(anyhow!("resume_run must come before the first turn"));
        }
        if continuation.trim().is_empty() {
            return Err(anyhow!("resume_run needs a continuation message"));
        }
        if !msgs.iter().any(|m| matches!(m.role, Role::User)) {
            return Err(anyhow!("the earlier session has no task to carry on"));
        }
        self.cfg.task.clear();
        self.resume_messages(msgs)?;
        self.continuation = Some(continuation.to_string());
        Ok(())
    }

    /// The flag a watcher raises, together with the cancel token, when the
    /// runner asks this run to hold: the run then ends `held` (exit code
    /// [`HELD_EXIT_CODE`]) rather than `cancelled`.
    pub fn hold_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.hold)
    }

    /// Whether the run was asked to hold.
    fn holding(&self) -> bool {
        self.hold.load(Ordering::SeqCst)
    }

    /// The model's view of an observation; a command the run's stop cut
    /// short says so instead of reading as the command's own timeout.
    fn observation_text(&self, cancel: &CancelToken, obs: &Observation) -> String {
        if obs.timed_out && cancel.is_cancelled() {
            format_interrupted_observation(obs, self.cfg.output_limit, self.holding())
        } else {
            format_observation(obs, self.cfg.output_limit)
        }
    }

    /// Replaces the model for the next calls (`/model` in a session).
    pub fn set_model(&mut self, model: &str, context_window: i32) {
        self.cfg.model = model.to_string();
        if context_window > 0 {
            self.cfg.context_window = context_window;
        }
    }

    /// Compacts the conversation now (`/compact` in a session): Some(true)
    /// when it was summarised, Some(false) when the model gave no summary,
    /// None when it is too short to compact.
    pub fn compact_now(&mut self, cancel: &CancelToken) -> Option<bool> {
        let out = self.compact(cancel, "manual");
        if out == Some(true) {
            self.state.calls_at_compaction = self.result.calls;
            self.state.last_prompt_tokens = 0;
        }
        out
    }

    /// The run so far: calls, steps, usage.
    pub fn result(&self) -> ResultSummary {
        self.result.clone()
    }

    fn push_user(&mut self, content: String) {
        self.push_user_with_images(content, Vec::new());
    }

    fn push_user_with_images(&mut self, content: String, images: Vec<ImageContent>) {
        self.msgs.push(Message {
            role: Role::User,
            content,
            tool_calls: Vec::new(),
            tool_call_id: String::new(),
            name: String::new(),
            images,
        });
    }

    /// Model calls made in the current turn (the whole run of a job).
    fn calls_this_turn(&self) -> i32 {
        self.result.calls - self.state.turn_calls_start
    }

    /// Model calls left before the step budget of this turn (or job).
    fn calls_remaining(&self) -> i32 {
        self.cfg.max_steps - self.calls_this_turn()
    }

    /// Time spent against the wall-time budget: the turn's in an
    /// interactive session, the run's otherwise.
    fn clock_elapsed(&self) -> Duration {
        match self.state.turn_started {
            Some(t) if self.cfg.interactive => t.elapsed(),
            _ => self.started.elapsed(),
        }
    }

    /// Sets the run up once: defaults, the system prompt, the deny rules,
    /// the start notes and hooks, and the system message.
    fn start(&mut self, cancel: &CancelToken) -> anyhow::Result<()> {
        self.ran = true;
        self.cfg = apply_defaults(self.cfg.clone());
        if self.cfg.model.trim().is_empty() {
            return Err(anyhow!("harness model is empty"));
        }
        self.started = Instant::now();
        if self.cfg.system_prompt.is_empty() {
            let tmpl = if !self.cfg.system_prompt_template.is_empty() {
                &self.cfg.system_prompt_template
            } else if self.cfg.interactive {
                CHAT_PROMPT_TEMPLATE
            } else {
                DEFAULT_PROMPT_TEMPLATE
            };
            self.cfg.system_prompt = render_prompt(tmpl, prompt_data(&self.env.info(), &self.cfg))?;
        }
        if self.cfg.read_only {
            // Appended after rendering so an owner's own template gets it too.
            self.cfg.system_prompt.push_str("\n\n");
            self.cfg.system_prompt.push_str(READ_ONLY_PROMPT);
            self.start_notes.insert(0, READ_ONLY_START_NOTE.to_string());
        }
        self.compile_deny()?;
        for note in std::mem::take(&mut self.start_notes) {
            self.emit(Step {
                kind: StepKind::Note,
                content: note,
                ..Step::default()
            });
        }
        self.run_start_hooks(cancel);
        let resumed = std::mem::take(&mut self.msgs);
        // Tools the earlier session ran count as evidence: a resumed run
        // that reports straight away is not an evidence-free give-up.
        let carried_tool_calls = resumed.iter().any(|m| !m.tool_calls.is_empty());
        self.msgs = vec![Message {
            role: Role::System,
            content: self.cfg.system_prompt.clone(),
            tool_calls: Vec::new(),
            tool_call_id: String::new(),
            name: String::new(),
            ..Default::default()
        }];
        if !resumed.is_empty() {
            let note = format!(
                "resumed an earlier session: {} messages carried over",
                resumed.len()
            );
            self.emit(Step {
                kind: StepKind::Note,
                content: note.clone(),
                ..Step::default()
            });
            self.msgs.extend(resumed);
            // The model hears it too: files may have changed in between.
            self.push_user(format!(
                "Note: {note}. Time has passed since; the checkout may have changed, so read a file again before relying on what you saw of it earlier."
            ));
        }
        self.state = LoopState {
            initiator: "user".to_string(),
            keep_observations: self.cfg.keep_observations,
            tool_call_seen: carried_tool_calls,
            ..LoopState::default()
        };
        Ok(())
    }

    /// The loop: model call, tool calls, until an answer or a stop.
    fn drive(&mut self, cancel: &CancelToken) -> Drive {
        loop {
            if let Some(status) = self.limit_status(cancel) {
                let msg = self.limit_error(status);
                return Drive::Stopped { status, error: msg };
            }
            if self.result.calls > 0
                && should_compact(
                    self.cfg.compact_at,
                    self.cfg.context_window,
                    self.state.last_prompt_tokens,
                )
                && self.result.calls - self.state.calls_at_compaction >= COMPACT_MIN_GAP_CALLS
                && let Some(changed) = self.compact(cancel, "auto")
            {
                self.state.calls_at_compaction = self.result.calls;
                if changed {
                    self.state.last_prompt_tokens = 0;
                }
            }
            if self.result.calls > 0 {
                let mut level = 1;
                if self.cfg.context_window > 0
                    && self.state.last_prompt_tokens
                        > (0.8 * f64::from(self.cfg.context_window)) as i32
                {
                    level = 2;
                    if self.state.last_prune_level == 2 && self.state.keep_observations > 1 {
                        self.state.keep_observations = 1.max(self.state.keep_observations / 2);
                    }
                }
                let (next, changed) =
                    prune_history(&self.msgs, self.state.keep_observations, level);
                self.msgs = next;
                let key = format!("{level}/{}", self.state.keep_observations);
                if changed > 0 && !self.state.emitted_prune_notes.contains(&key) {
                    self.emit(Step {
                        kind: StepKind::Note,
                        content: format!(
                            "collapsed {changed} older observations to fit the context window (level {level}, keep {})",
                            self.state.keep_observations
                        ),
                        ..Step::default()
                    });
                    self.state.emitted_prune_notes.insert(key);
                }
                self.state.last_prune_level = level;
            }
            if !self.state.budget_notice_sent
                && let Some(notice) = self.budget_notice()
            {
                self.push_user(notice.clone());
                self.emit(Step {
                    kind: StepKind::Note,
                    content: notice,
                    ..Step::default()
                });
                self.state.budget_notice_sent = true;
            }
            let req = Request {
                model: self.cfg.model.clone(),
                messages: clone_messages(&self.msgs),
                tools: self.tools(),
                initiator: self.state.initiator.clone(),
                reasoning: self.cfg.reasoning.clone(),
                ..Request::default()
            };
            let mut stream_retries = 0u32;
            let asked_at = Instant::now();
            let resp = loop {
                let attempt = req.clone();
                let resp = match self.stream.as_mut() {
                    Some(f) => self
                        .p
                        .chat_stream(cancel, attempt, &mut |text| f.text(text)),
                    None => self.p.chat(cancel, attempt),
                };
                self.result.calls += 1;
                match resp {
                    Err(err)
                        if is_stream_interrupted(&err)
                            && stream_retries < STREAM_RETRIES
                            && !cancel.is_cancelled() =>
                    {
                        stream_retries += 1;
                        if let Some(f) = self.stream.as_mut() {
                            f.restart();
                        }
                        self.emit(Step {
                            kind: StepKind::Note,
                            content: format!(
                                "the reply stopped part way ({}); asking again ({stream_retries} of {STREAM_RETRIES})",
                                one_line(&err.to_string(), 200)
                            ),
                            ..Step::default()
                        });
                        std::thread::sleep(Duration::from_secs(u64::from(stream_retries)));
                    }
                    other => break other,
                }
            };
            let resp = match resp {
                Ok(r) => r,
                Err(err) => {
                    if !self.cfg.wall_time.is_zero() && self.clock_elapsed() >= self.cfg.wall_time {
                        let msg = self.limit_error(EXIT_TIME_LIMIT);
                        return Drive::Stopped {
                            status: EXIT_TIME_LIMIT,
                            error: msg,
                        };
                    }
                    if cancel.is_cancelled() {
                        let status = if self.holding() {
                            self.cancel_reason = HELD_REASON.to_string();
                            EXIT_HELD
                        } else {
                            self.cancel_reason = if self.cfg.cancel_reason.is_empty() {
                                "interrupted: context canceled".to_string()
                            } else {
                                self.cfg.cancel_reason.clone()
                            };
                            EXIT_CANCELLED
                        };
                        let msg = self.limit_error(status);
                        self.emit(Step {
                            kind: StepKind::Error,
                            content: msg.clone(),
                            ..Step::default()
                        });
                        return Drive::Stopped { status, error: msg };
                    }
                    let status = if pulse_provider::is_not_logged_in(&err) {
                        EXIT_NOT_LOGGED_IN
                    } else {
                        EXIT_PROVIDER_ERROR
                    };
                    let msg = one_line(&err.to_string(), 400);
                    self.emit(Step {
                        kind: StepKind::Error,
                        content: msg.clone(),
                        ..Step::default()
                    });
                    return Drive::Stopped { status, error: msg };
                }
            };
            self.state.initiator = "agent".to_string();
            self.result.usage.add(resp.usage);
            self.state.last_prompt_tokens = resp.usage.prompt_tokens;
            let mut msg = resp
                .message
                .unwrap_or_else(|| Message::new(Role::Assistant));
            msg.role = Role::Assistant;
            self.msgs.push(msg.clone());
            let model = if resp.model.is_empty() {
                self.cfg.model.clone()
            } else {
                resp.model
            };
            self.emit(Step {
                kind: StepKind::Assistant,
                content: msg.content.clone(),
                // How long the model took to answer: the "thought for 12s"
                // between one tool call and the next.
                duration_ms: asked_at.elapsed().as_millis() as i64,
                usage: resp.usage,
                model,
                finish_reason: Some(resp.finish_reason),
                choices: (resp.choices > 0).then_some(i64::from(resp.choices)),
                ..Step::default()
            });
            let free_format_retry = self.state.used_format_retries < self.cfg.format_retries.max(0);
            let (clean, submitted, submission, prose_only) =
                self.handle_assistant(cancel, msg, free_format_retry);
            if let Some((status, msg)) = self.fatal.take() {
                return Drive::Stopped { status, error: msg };
            }
            if submitted {
                return Drive::Answered(submission);
            }
            if clean {
                self.state.counted_format_errors = 0;
                self.state.used_format_retries = 0;
            } else {
                self.result.format_errors += 1;
                if prose_only && free_format_retry {
                    self.state.used_format_retries += 1;
                    self.result.calls = self.result.calls.saturating_sub(1);
                    continue;
                }
                self.state.counted_format_errors += 1;
                if self.state.counted_format_errors >= self.cfg.max_format_errors {
                    return Drive::Stopped {
                        status: EXIT_FORMAT_ERROR,
                        error: String::new(),
                    };
                }
            }
        }
    }

    pub fn messages(&self) -> Vec<Message> {
        clone_messages(&self.msgs)
    }

    pub fn trajectory(&self) -> Trajectory {
        Trajectory {
            trajectory_format: "pulse-harness-1".to_string(),
            info: TrajectoryInfo {
                model_stats: ModelStats {
                    api_calls: self.result.calls,
                    prompt_tokens: self.result.usage.prompt_tokens,
                    completion_tokens: self.result.usage.completion_tokens,
                    cost: self.result.usage.cost,
                },
                exit_status: self.result.exit_status.clone(),
                submission: self.result.submission.clone(),
                config: TrajectoryConfig {
                    model: self.cfg.model.clone(),
                    provider: self.cfg.provider.clone(),
                    max_steps: self.cfg.max_steps,
                    step_timeout_seconds: self.cfg.step_timeout.as_secs() as i64,
                    cwd: self.cfg.cwd.clone(),
                    interactive: self.cfg.interactive,
                },
            },
            messages: self.messages(),
            steps: self.steps.clone(),
        }
    }

    fn compile_deny(&mut self) -> anyhow::Result<()> {
        let mut patterns: Vec<String> = DEFAULT_DENY.iter().map(|s| (*s).to_string()).collect();
        patterns.extend(self.cfg.deny.clone());
        self.deny.clear();
        for p in patterns {
            let re = Regex::new(&p).map_err(|e| anyhow!("compile deny regexp {:?}: {e}", p))?;
            self.deny.push(DenyRule {
                description: deny_description(&p),
                re,
            });
        }
        Ok(())
    }

    fn limit_status(&mut self, cancel: &CancelToken) -> Option<&'static str> {
        if !self.cfg.cancel_file.is_empty() && Path::new(&self.cfg.cancel_file).exists() {
            self.cancel_reason = ERR_OWNER_CANCELLED.to_string();
            return Some(EXIT_CANCELLED);
        }
        if self.holding() {
            self.cancel_reason = HELD_REASON.to_string();
            return Some(EXIT_HELD);
        }
        if cancel.is_cancelled() {
            self.cancel_reason = if self.cfg.cancel_reason.is_empty() {
                "interrupted: context canceled".to_string()
            } else {
                self.cfg.cancel_reason.clone()
            };
            return Some(EXIT_CANCELLED);
        }
        if self.calls_this_turn() >= self.cfg.max_steps {
            return Some(EXIT_STEP_LIMIT);
        }
        if !self.cfg.wall_time.is_zero() && self.clock_elapsed() >= self.cfg.wall_time {
            return Some(EXIT_TIME_LIMIT);
        }
        if self.cfg.max_cost > 0.0 && self.result.usage.cost >= self.cfg.max_cost {
            return Some(EXIT_COST_LIMIT);
        }
        None
    }

    fn budget_notice(&self) -> Option<String> {
        if self.cfg.max_steps < 10 {
            return None;
        }
        let reserve = 5.min(self.cfg.max_steps / 10);
        let remaining = self.calls_remaining();
        // `<=`, not `==`: a subagent's calls can carry the count past the
        // exact mark in one step.
        if remaining > reserve {
            return None;
        }
        if self.cfg.interactive {
            return Some(format!(
                "Budget notice: {remaining} model calls remain for this turn of {}. Finish the step in hand if it is small, then reply to the user with where things stand and what is left; they can say continue. Do not start new work.",
                self.cfg.max_steps
            ));
        }
        let close = if self.cfg.read_only {
            "Stop exploring. Reply with no command and your final report starting with SUMMARY: (use SUMMARY: blocked: or SUMMARY: failed: if unfinished). Do not start new work."
        } else {
            "Stop exploring. If you changed files, run the targeted check and commit finished work now, then reply with no command and your final report starting with SUMMARY: (use SUMMARY: blocked: or SUMMARY: failed: if unfinished). Do not start new work."
        };
        Some(format!(
            "Budget notice: {remaining} model calls remain of {}. {close}",
            self.cfg.max_steps
        ))
    }

    fn limit_error(&self, status: &str) -> String {
        match status {
            EXIT_STEP_LIMIT if self.cfg.interactive => format!(
                "stopped at the step budget for this turn: {} model calls (say continue to go on)",
                self.cfg.max_steps
            ),
            EXIT_STEP_LIMIT => format!(
                "stopped at the step budget: {} model calls",
                self.cfg.max_steps
            ),
            EXIT_TIME_LIMIT => format!(
                "stopped at the time budget: {}m",
                (self.cfg.wall_time.as_secs() + 30) / 60
            ),
            EXIT_COST_LIMIT => format!(
                "stopped at the cost budget: {:.2} of {} (provider units)",
                self.result.usage.cost,
                trim_float(self.cfg.max_cost)
            ),
            EXIT_CANCELLED | EXIT_HELD => self.cancel_reason.clone(),
            _ => String::new(),
        }
    }

    fn handle_assistant(
        &mut self,
        cancel: &CancelToken,
        msg: Message,
        free_format_retry: bool,
    ) -> (bool, bool, String, bool) {
        if !msg.tool_calls.is_empty() {
            self.state.tool_call_seen = true;
            return self.handle_tool_calls(cancel, msg.tool_calls);
        }
        let actions = action_blocks(&msg.content);
        if actions.len() == 1 {
            self.state.tool_call_seen = true;
            let (tool_name, args) = match &actions[0] {
                ActionBlock::Command(command) => ("bash", json!({ "command": command })),
                ActionBlock::Read(spec) => ("read", json!({ "spec": spec })),
                ActionBlock::Grep(spec) => ("grep", json!({ "spec": spec })),
                ActionBlock::Edit(spec) => ("edit", json!({ "spec": spec })),
            };
            let args = args.to_string();
            let (obs_text, submit, sub) =
                if let Some(blocked) = self.gates(cancel, tool_name, &args) {
                    (blocked, false, String::new())
                } else {
                    let (text, submit, sub) = match &actions[0] {
                        ActionBlock::Command(command) => {
                            let (text, _, submit, sub) = self.run_command(cancel, command);
                            (text, submit, sub)
                        }
                        ActionBlock::Read(spec) => self.run_native_action(NativeAction::Read, spec),
                        ActionBlock::Grep(spec) => self.run_native_action(NativeAction::Grep, spec),
                        ActionBlock::Edit(spec) => self.run_native_action(NativeAction::Edit, spec),
                    };
                    let text = if submit {
                        text
                    } else {
                        self.with_post_tool_hooks(cancel, tool_name, &args, text)
                    };
                    (text, submit, sub)
                };
            self.msgs.push(Message {
                role: Role::User,
                content: obs_text,
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
                ..Default::default()
            });
            if submit {
                return (true, true, sub, false);
            }
            return (true, false, String::new(), false);
        }
        if actions.len() > 1 {
            let text =
                "write exactly one fenced bash/read/grep/edit block per message (or call several tools in one reply), or your final report with no command"
                    .to_string();
            self.msgs.push(Message {
                role: Role::User,
                content: text.clone(),
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
                ..Default::default()
            });
            self.emit(Step {
                kind: StepKind::Error,
                content: text,
                ..Step::default()
            });
            return (false, false, String::new(), false);
        }
        if is_report(&msg.content) {
            // A report on the first turn, before any tool ran, is a give-up:
            // six runs in 30h ended at step 1 with "I ran no commands". Send
            // it back once; the second evidence-free report is accepted.
            if !self.cfg.interactive
                && self.cfg.depth == 0
                && !self.state.tool_call_seen
                && !self.state.evidence_free_report_bounced
            {
                self.state.evidence_free_report_bounced = true;
                let text = "Your report says nothing was run. This is the first turn: run at least one command (for example `pulse goal`, `git log -5 --stat`, or the check the brief names) before reporting, or report again if you truly cannot proceed.".to_string();
                self.msgs.push(Message {
                    role: Role::User,
                    content: text.clone(),
                    tool_calls: Vec::new(),
                    tool_call_id: String::new(),
                    name: String::new(),
                    ..Default::default()
                });
                self.emit(Step {
                    kind: StepKind::FormatRetry,
                    content: text,
                    ..Step::default()
                });
                return (true, false, String::new(), false);
            }
            return (true, true, msg.content, false);
        }
        if self.cfg.interactive && !msg.content.trim().is_empty() {
            // A reply without a tool call is the answer to the user.
            return (true, true, msg.content, false);
        }
        let text = if self.cfg.interactive {
            "Your reply was empty. Either call a tool to keep working, or reply to the user in plain text.".to_string()
        } else {
            "Your message had no command and no report. Either call a tool (bash, read, grep, glob, edit, write, plan; or write one ```bash block) to keep working, or finish with your report: a `SUMMARY:` line plus any `FINDING:`/`SUGGESTION:`/`QUESTION:`/`VOTE:` lines.".to_string()
        };
        self.msgs.push(Message {
            role: Role::User,
            content: text.clone(),
            tool_calls: Vec::new(),
            tool_call_id: String::new(),
            name: String::new(),
            ..Default::default()
        });
        self.emit(Step {
            kind: if free_format_retry {
                StepKind::FormatRetry
            } else {
                StepKind::Error
            },
            content: text,
            ..Step::default()
        });
        (false, false, String::new(), true)
    }

    /// Runs the typed tool calls of one reply. Calls that only read (read,
    /// grep, glob, fetch) run at the same time when there are at least two
    /// of them; the rest run one after another, in order. The tool results
    /// go back in the order the model made the calls.
    fn handle_tool_calls(
        &mut self,
        cancel: &CancelToken,
        calls: Vec<ToolCall>,
    ) -> (bool, bool, String, bool) {
        let mut batch: Vec<Option<ParallelCall>> =
            calls.iter().map(|c| self.parallel_call(c)).collect();
        if batch.iter().flatten().count() < 2 {
            batch.iter_mut().for_each(|b| *b = None);
        }
        // Hooks and the call steps first, one by one and in order; a call a
        // hook blocks leaves the batch with its refusal as the result.
        let mut blocked: Vec<Option<String>> = vec![None; calls.len()];
        for (i, call) in calls.iter().enumerate() {
            if batch[i].is_none() {
                continue;
            }
            if let Some(text) = self.gates(cancel, &call.name, &call.arguments) {
                blocked[i] = Some(text);
                batch[i] = None;
                continue;
            }
            let p = batch[i].as_ref().expect("parallel call");
            self.result.steps += 1;
            self.emit(Step {
                kind: p.step_kind(),
                content: p.spec(),
                command: p.spec(),
                ..Step::default()
            });
        }
        let mut observations = self.run_parallel(&batch);
        let mut outcomes: Vec<Option<ToolOutcome>> = (0..calls.len()).map(|_| None).collect();
        for (i, call) in calls.iter().enumerate() {
            if let Some(text) = blocked[i].take() {
                outcomes[i] = Some(ToolOutcome::clean(text));
                continue;
            }
            let (Some(p), Some(obs)) = (&batch[i], observations[i].take()) else {
                continue;
            };
            if let Some(err) = p.error_step(&obs) {
                self.emit(Step {
                    kind: StepKind::Error,
                    content: err,
                    command: p.spec(),
                    ..Step::default()
                });
            }
            self.emit_observation(&p.observation_command(), &obs);
            let mut text = self.observation_text(cancel, &obs);
            if let ParallelCall::Native(native) = p
                && obs.exit_code == 0
            {
                text = self.with_path_instructions(native, text);
            }
            let text = self.with_post_tool_hooks(cancel, &call.name, &call.arguments, text);
            outcomes[i] = Some(ToolOutcome::clean(text));
        }
        let mut all_clean = true;
        for (i, call) in calls.iter().enumerate() {
            let outcome = match outcomes[i].take() {
                Some(o) => o,
                None => self.dispatch_tool_call(cancel, call),
            };
            self.msgs.push(tool_message(call, &outcome.text));
            if !outcome.clean {
                all_clean = false;
            }
            if outcome.submitted {
                return (true, true, outcome.submission, false);
            }
            if self.fatal.is_some() {
                break;
            }
        }
        (all_clean, false, String::new(), false)
    }

    /// The call as something that can run beside the other calls of its
    /// reply, or None when it must run in order (bash, edit, write, plan,
    /// skill, subagent, MCP, or a call whose arguments do not parse).
    fn parallel_call(&self, call: &ToolCall) -> Option<ParallelCall> {
        match call.name.as_str() {
            name if PARALLEL_TOOL_NAMES.contains(&name) => {
                NativeCall::from_args(name, &call.arguments)
                    .ok()
                    .filter(NativeCall::parallel_safe)
                    .map(ParallelCall::Native)
            }
            FETCH_TOOL if !self.fetch.is_empty() => {
                let args: Value = serde_json::from_str(&call.arguments).ok()?;
                let url = args.get("url")?.as_str()?.trim();
                (!url.is_empty()).then(|| ParallelCall::Fetch(url.to_string()))
            }
            WEB_SEARCH_TOOL if self.offers_web_search() => {
                let (query, count) = crate::search::parse_arguments(&call.arguments).ok()?;
                Some(ParallelCall::Search { query, count })
            }
            _ => None,
        }
    }

    /// Runs the batch's calls on separate threads, at most
    /// [`PARALLEL_TOOL_LIMIT`] at a time; the result sits at the index of
    /// its call.
    fn run_parallel(&self, batch: &[Option<ParallelCall>]) -> Vec<Option<Observation>> {
        let fs = self.fs();
        let policy = &self.fetch;
        let search = &self.web_search;
        let mut out: Vec<Option<Observation>> = (0..batch.len()).map(|_| None).collect();
        let pending: Vec<(usize, &ParallelCall)> = batch
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.as_ref().map(|p| (i, p)))
            .collect();
        for chunk in pending.chunks(PARALLEL_TOOL_LIMIT) {
            let results: Vec<(usize, Observation)> = std::thread::scope(|scope| {
                let handles: Vec<_> = chunk
                    .iter()
                    .map(|(i, p)| {
                        let i = *i;
                        scope.spawn(move || (i, p.execute(fs, policy, search)))
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| {
                        h.join().unwrap_or_else(|_| {
                            (
                                usize::MAX,
                                action_error("tool thread panicked".into(), Instant::now(), 1),
                            )
                        })
                    })
                    .collect()
            });
            for (i, obs) in results {
                if i != usize::MAX {
                    out[i] = Some(obs);
                }
            }
        }
        // A call whose thread panicked still gets an answer.
        for (i, p) in &pending {
            if out[*i].is_none() {
                out[*i] = Some(action_error(
                    format!("{}: the tool failed unexpectedly", p.name()),
                    Instant::now(),
                    1,
                ));
            }
        }
        out
    }

    /// Runs one typed tool call, with the owner's hooks around it, and
    /// returns what the model gets back.
    fn dispatch_tool_call(&mut self, cancel: &CancelToken, call: &ToolCall) -> ToolOutcome {
        if let Some(blocked) = self.gates(cancel, &call.name, &call.arguments) {
            return ToolOutcome::clean(blocked);
        }
        let mut outcome = self.dispatch_tool_call_inner(cancel, call);
        if outcome.clean && !outcome.submitted {
            let text = std::mem::take(&mut outcome.text);
            outcome.text = self.with_post_tool_hooks(cancel, &call.name, &call.arguments, text);
        }
        outcome
    }

    fn dispatch_tool_call_inner(&mut self, cancel: &CancelToken, call: &ToolCall) -> ToolOutcome {
        match call.name.as_str() {
            "bash" => {
                let Some(command) = parse_command(&call.arguments) else {
                    return self.tool_error(
                        "tool arguments must be a JSON object with a string command".to_string(),
                    );
                };
                let background = match parse_background(&call.arguments) {
                    Ok(b) => b,
                    Err(e) => return self.tool_error(format!("bash: {e}")),
                };
                if background.is_some() && !self.offers_background() {
                    return self.tool_error(if self.cfg.read_only {
                        "bash: a read-only run runs its commands to completion; background is not offered".to_string()
                    } else {
                        "bash: background is not offered in this environment".to_string()
                    });
                }
                let title = parse_title(&call.arguments);
                let (text, _obs, submitted, submission) =
                    self.run_command_opts(cancel, &command, background, &title);
                ToolOutcome {
                    text,
                    clean: true,
                    submitted,
                    submission,
                }
            }
            PROCESS_TOOL if self.offers_background() => {
                self.run_process_tool(cancel, &call.arguments)
            }
            name if NATIVE_TOOL_NAMES.contains(&name) => {
                match NativeCall::from_args(name, &call.arguments) {
                    Ok(native) => ToolOutcome::clean(self.run_native_call(&native)),
                    Err(e) => self.tool_error(format!("{name}: {e}")),
                }
            }
            MEMORY_TOOL if self.offers_memory() => self.run_memory_tool(&call.arguments),
            SKILL_TOOL if !self.skills.is_empty() => self.run_skill_tool(&call.arguments),
            FETCH_TOOL if !self.fetch.is_empty() => self.run_fetch_tool(&call.arguments),
            WEB_SEARCH_TOOL if self.offers_web_search() => {
                self.run_web_search_tool(&call.arguments)
            }
            SUBAGENT_TOOL if self.offers_subagents() => self.run_subagent(cancel, &call.arguments),
            QUESTION_TOOL if self.offers_question() => self.run_question_tool(&call.arguments),
            MCP_SEARCH_TOOL if self.mcp.deferred() => {
                let query = serde_json::from_str::<Value>(&call.arguments)
                    .ok()
                    .and_then(|v| v.get("query").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_default();
                self.result.steps += 1;
                self.emit(Step {
                    kind: StepKind::Mcp,
                    content: format!("{MCP_SEARCH_TOOL} {query}"),
                    command: MCP_SEARCH_TOOL.to_string(),
                    ..Step::default()
                });
                let start = Instant::now();
                let text = self.mcp.search(&query);
                let obs = Observation {
                    output: text,
                    duration: start.elapsed(),
                    ..Observation::default()
                };
                self.emit_observation(MCP_SEARCH_TOOL, &obs);
                ToolOutcome::clean(format_observation(&obs, self.cfg.output_limit))
            }
            name if self.mcp.has(name) => self.run_mcp_tool(name, &call.arguments),
            name => self.tool_error(format!(
                "unknown tool {name}; available tools: {}",
                self.tool_names().join(", ")
            )),
        }
    }

    /// The `question` tool: the user's answer, read through the session's
    /// [`Ui`].
    fn run_question_tool(&mut self, arguments: &str) -> ToolOutcome {
        let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
        let question = args
            .get("question")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        if question.is_empty() {
            return self.tool_error(
                "question: arguments must be a JSON object with a string question".to_string(),
            );
        }
        let choices: Vec<String> = args
            .get("choices")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        self.result.steps += 1;
        let spec = if choices.is_empty() {
            question.clone()
        } else {
            format!("{question}\n{}", choices.join(" | "))
        };
        self.emit(Step {
            kind: StepKind::Note,
            content: format!("question: {spec}"),
            command: QUESTION_TOOL.to_string(),
            ..Step::default()
        });
        let start = Instant::now();
        let answer = self.ui.as_ref().and_then(|ui| {
            ui.lock()
                .unwrap_or_else(|e| e.into_inner())
                .ask(&question, &choices)
        });
        let obs = match answer {
            Some(a) => Observation {
                output: format!("The user answered: {a}"),
                duration: start.elapsed(),
                ..Observation::default()
            },
            None => Observation {
                output: "The user did not answer (input closed). Decide yourself and go on."
                    .to_string(),
                exit_code: 1,
                duration: start.elapsed(),
                ..Observation::default()
            },
        };
        self.emit_observation(&format!("{QUESTION_TOOL} tool"), &obs);
        ToolOutcome::clean(format_observation(&obs, self.cfg.output_limit))
    }

    /// Shows a call that changes things to the session's [`Ui`] when there
    /// is one. Some(observation) is the declined call's result; the call
    /// does not run.
    fn approval(
        &mut self,
        tool: &str,
        summary: &str,
        detail: &str,
        reason: &str,
    ) -> Option<Observation> {
        let ui = self.ui.clone()?;
        let subjects = if self.current_subjects.is_empty() {
            vec![summary.to_string()]
        } else {
            self.current_subjects.clone()
        };
        if self.permissions.allows_all(tool, &subjects) {
            return None;
        }
        let always = permissions::always_pattern(tool, &subjects[0]);
        let req = ApprovalRequest {
            tool: tool.to_string(),
            summary: summary.to_string(),
            detail: detail.to_string(),
            reason: reason.to_string(),
            always: always.clone(),
        };
        let start = Instant::now();
        let decision = ui.lock().unwrap_or_else(|e| e.into_inner()).approve(&req);
        match decision {
            Approval::Allow => None,
            Approval::Always => {
                if let Ok(rule) = Rule::parse(true, &always, permissions::Source::Session) {
                    self.emit(Step {
                        kind: StepKind::Note,
                        content: format!(
                            "permission: allow {} for the rest of the session",
                            rule.pattern()
                        ),
                        command: summary.to_string(),
                        ..Step::default()
                    });
                    self.permissions.push(rule);
                }
                None
            }
            Approval::Deny(why) => {
                let why = why.trim();
                let output = if why.is_empty() {
                    format!(
                        "{tool}: declined by the user. Do not do the same thing another way; ask them or move on."
                    )
                } else {
                    format!(
                        "{tool}: declined by the user: {why}. Follow that instead of doing the same thing another way."
                    )
                };
                self.emit(Step {
                    kind: StepKind::Note,
                    content: output.clone(),
                    command: summary.to_string(),
                    ..Step::default()
                });
                Some(Observation {
                    output,
                    exit_code: 126,
                    duration: start.elapsed(),
                    ..Observation::default()
                })
            }
        }
    }

    /// The `skill` tool: the instructions of one skill, or one of its files.
    /// The `memory` tool: remember, forget or list what this project (or
    /// the user) keeps across runs.
    fn run_memory_tool(&mut self, arguments: &str) -> ToolOutcome {
        let action = match MemoryAction::parse(arguments) {
            Ok(a) => a,
            Err(e) => return self.tool_error(format!("memory: {e}")),
        };
        let Some(memory) = self.memory.clone() else {
            return self.tool_error("memory is off".to_string());
        };
        self.result.steps += 1;
        let spec = action.spec();
        self.emit(Step {
            kind: StepKind::Memory,
            content: spec.clone(),
            command: spec,
            ..Step::default()
        });
        let start = Instant::now();
        let outcome = match &action {
            MemoryAction::Remember { scope, text } => memory.remember(*scope, text).map(|c| {
                format!(
                    "remembered for the {} ({} in {}); it reaches the prompt of later runs here",
                    c.scope.as_str(),
                    plural(c.count, "memory", "memories"),
                    memory.shown_path(c.scope)
                )
            }),
            MemoryAction::Forget { scope, text } => memory.forget(*scope, text).map(|c| {
                format!(
                    "forgotten from the {} ({} left in {})",
                    c.scope.as_str(),
                    plural(c.count, "memory", "memories"),
                    memory.shown_path(c.scope)
                )
            }),
            MemoryAction::List => Ok(memory.list()),
        };
        let obs = match outcome {
            Ok(text) => Observation {
                output: text,
                duration: start.elapsed(),
                ..Observation::default()
            },
            Err(text) => action_error(text, start, 1),
        };
        self.emit_observation("memory tool", &obs);
        ToolOutcome::clean(format_observation(&obs, self.cfg.output_limit))
    }

    fn run_skill_tool(&mut self, arguments: &str) -> ToolOutcome {
        let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        if name.is_empty() {
            return self.tool_error(
                "skill: arguments must be a JSON object with a string name".to_string(),
            );
        }
        let file = args
            .get("file")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        self.result.steps += 1;
        let spec = if file.is_empty() {
            name.clone()
        } else {
            format!("{name} {file}")
        };
        self.emit(Step {
            kind: StepKind::Skill,
            content: spec.clone(),
            command: spec,
            ..Step::default()
        });
        let start = Instant::now();
        let obs = match load_skill(&self.skills, &name, &file) {
            Ok(text) => Observation {
                output: text,
                duration: start.elapsed(),
                ..Observation::default()
            },
            Err(text) => action_error(text, start, 1),
        };
        self.emit_observation("skill tool", &obs);
        // A skill is read whole: its body is already capped by the loader.
        let limit = self
            .cfg
            .output_limit
            .max(crate::skills::SKILL_FILE_LIMIT + 1024);
        ToolOutcome::clean(format_observation(&obs, limit))
    }

    /// The `fetch` tool: one web page as text, for the allowed hosts.
    fn run_fetch_tool(&mut self, arguments: &str) -> ToolOutcome {
        let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
        let url = args
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        if url.is_empty() {
            return self.tool_error(
                "fetch: arguments must be a JSON object with a string url".to_string(),
            );
        }
        self.result.steps += 1;
        self.emit(Step {
            kind: StepKind::Fetch,
            content: url.clone(),
            command: url.clone(),
            ..Step::default()
        });
        let start = Instant::now();
        let r = fetch(&url, &self.fetch, FETCH_BYTE_LIMIT, FETCH_TIMEOUT);
        let obs = Observation {
            output: r.output,
            exit_code: r.exit_code,
            duration: start.elapsed(),
            ..Observation::default()
        };
        if r.exit_code == crate::fetch::FETCH_REFUSED_EXIT {
            self.emit(Step {
                kind: StepKind::Error,
                content: obs.output.clone(),
                command: url.clone(),
                ..Step::default()
            });
        }
        self.emit_observation("fetch tool", &obs);
        ToolOutcome::clean(format_observation(&obs, self.cfg.output_limit))
    }

    fn run_web_search_tool(&mut self, arguments: &str) -> ToolOutcome {
        let (query, count) = match crate::search::parse_arguments(arguments) {
            Ok(q) => q,
            Err(e) => return self.tool_error(e),
        };
        self.result.steps += 1;
        self.emit(Step {
            kind: StepKind::Search,
            content: query.clone(),
            command: query.clone(),
            ..Step::default()
        });
        let start = Instant::now();
        let r = self.web_search.search(&query, count, SEARCH_TIMEOUT);
        let obs = Observation {
            output: r.output,
            exit_code: r.exit_code,
            duration: start.elapsed(),
            ..Observation::default()
        };
        self.emit_observation("web_search tool", &obs);
        ToolOutcome::clean(format_observation(&obs, self.cfg.output_limit))
    }

    /// The `subagent` tool: a helper run on a sub-task, sharing this run's
    /// provider, budget, clone, skills, hooks and fetch policy. Its steps
    /// reach this run's sinks one level deeper; its report comes back as
    /// the tool result.
    fn run_subagent(&mut self, cancel: &CancelToken, arguments: &str) -> ToolOutcome {
        let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
        let task = args
            .get("task")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        if task.is_empty() {
            return self.tool_error(
                "subagent: arguments must be a JSON object with a string task".to_string(),
            );
        }
        let wanted = args
            .get("agent")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        let custom = if wanted.is_empty() {
            None
        } else {
            match self.agents.iter().find(|a| a.name == wanted).or_else(|| {
                self.agents
                    .iter()
                    .find(|a| a.name.eq_ignore_ascii_case(wanted))
            }) {
                Some(a) => Some(a.clone()),
                None => {
                    let known: Vec<&str> = self.agents.iter().map(|a| a.name.as_str()).collect();
                    return self.tool_error(if known.is_empty() {
                        format!("subagent: there is no custom agent {wanted:?}; leave agent out")
                    } else {
                        format!(
                            "subagent: there is no custom agent {wanted:?}; the agents are {}",
                            known.join(", ")
                        )
                    });
                }
            }
        };
        let requested = args
            .get("max_steps")
            .and_then(Value::as_i64)
            .map_or(SUBAGENT_DEFAULT_STEPS, |n| {
                n.clamp(1, i64::from(i32::MAX)) as i32
            });
        self.result.steps += 1;
        let start = Instant::now();
        let remaining = self.calls_remaining();
        let budget = requested.min(remaining - SUBAGENT_RESERVE);
        let time_left = (!self.cfg.wall_time.is_zero()).then(|| {
            self.cfg
                .wall_time
                .saturating_sub(self.clock_elapsed())
                .saturating_sub(SUBAGENT_TIME_RESERVE)
        });
        let cost_left =
            (self.cfg.max_cost > 0.0).then_some(self.cfg.max_cost - self.result.usage.cost);
        let refusal = if budget < 1 {
            Some(format!(
                "subagent: only {remaining} model calls remain in this run; do the work yourself and finish"
            ))
        } else if time_left.is_some_and(|t| t < Duration::from_secs(30)) {
            Some("subagent: too little of the run's time budget remains; do the work yourself and finish".to_string())
        } else if cost_left.is_some_and(|c| c <= 0.0) {
            Some("subagent: the run's cost budget is spent; finish now".to_string())
        } else {
            None
        };
        self.emit(Step {
            kind: StepKind::Subagent,
            content: task.clone(),
            command: match &custom {
                Some(a) => format!("{SUBAGENT_TOOL} {} ({} calls)", a.name, budget.max(0)),
                None => format!("{SUBAGENT_TOOL} ({} calls)", budget.max(0)),
            },
            ..Step::default()
        });
        if let Some(text) = refusal {
            let obs = action_error(text, start, 1);
            self.emit_observation(SUBAGENT_TOOL, &obs);
            return ToolOutcome::clean(format_observation(&obs, self.cfg.output_limit));
        }
        let Some(env) = self.env.fork() else {
            let obs = action_error(
                "subagent: this environment cannot run a helper; do the work yourself".to_string(),
                start,
                1,
            );
            self.emit_observation(SUBAGENT_TOOL, &obs);
            return ToolOutcome::clean(format_observation(&obs, self.cfg.output_limit));
        };
        let mut cfg = self.cfg.clone();
        cfg.task = match &custom {
            Some(a) if !a.body.is_empty() => format!(
                "You are the {} helper agent of a larger run, working in the same clone. Your standing instructions follow; then the sub-task. Do only the sub-task. Then reply with no tool call and a report that starts with SUMMARY: and carries everything the main agent needs: findings, exact file paths, commands and their results. The main agent does not see your conversation, only that report. Do not commit unless the sub-task says so.\n\n--- {} ---\n{}\n--- end of {} ---\n\nSub-task:\n{task}",
                a.name, a.source, a.body, a.name
            ),
            _ => format!(
                "You are a helper agent of a larger run, working in the same clone. Do only the sub-task below. Then reply with no tool call and a report that starts with SUMMARY: and carries everything the main agent needs: findings, exact file paths, commands and their results. The main agent does not see your conversation, only that report. Do not commit unless the sub-task says so.\n\nSub-task:\n{task}"
            ),
        };
        cfg.max_steps = budget;
        cfg.depth = self.cfg.depth.saturating_add(1);
        cfg.subagents = false;
        cfg.system_prompt = String::new();
        cfg.cancel_reason = String::new();
        if let Some(t) = time_left {
            cfg.wall_time = t;
        }
        if let Some(c) = cost_left {
            cfg.max_cost = c;
        }
        self.result.subagents += 1;
        let depth = cfg.depth;
        let mut child = new_shared(self.p.clone(), env, cfg, Vec::new());
        child.skills = self.skills.clone();
        child.hooks = self.hooks.clone();
        child.memory = self.memory.clone();
        child.fetch = self.fetch.clone();
        child.web_search = self.web_search.clone();
        child.permissions = self.permissions.clone();
        if let Some(a) = &custom {
            // The agent's tool list holds as deny rules, so a tool outside
            // it is neither offered nor run.
            let offered = self.tool_names();
            child
                .permissions
                .extend(withheld_tools(a, &offered).into_iter().map(|kind| Rule {
                    allow: false,
                    kind,
                    filter: None,
                    source: permissions::Source::Agent,
                }));
        }
        child.path_rules = self.path_rules.clone();
        child.ui = self.ui.clone();
        child.next_n = self.next_n;
        // The child's steps go to this run's sinks, one level deeper, and
        // the sinks come back when it is done.
        let shared = Arc::new(Mutex::new(std::mem::take(&mut self.sinks)));
        child.sinks = vec![Box::new(NestedSink {
            sinks: shared.clone(),
            depth,
        })];
        let run = child.run_with_cancel(cancel);
        child.sinks.clear();
        self.sinks = match Arc::try_unwrap(shared) {
            Ok(m) => m.into_inner().unwrap_or_else(|e| e.into_inner()),
            Err(shared) => std::mem::take(&mut *shared.lock().unwrap_or_else(|e| e.into_inner())),
        };
        self.next_n = child.next_n;
        let child_steps = std::mem::take(&mut child.steps);
        let last_child_message = child_steps
            .iter()
            .rev()
            .find(|s| s.kind == StepKind::Assistant && !s.content.trim().is_empty())
            .map(|s| s.content.trim().to_string());
        self.steps.extend(child_steps);
        let obs = match run {
            Ok(r) => {
                self.result.usage.add(r.usage);
                self.result.calls += r.calls;
                self.result.steps += r.steps;
                if r.exit_status == EXIT_SANDBOX_LOST && self.fatal.is_none() {
                    self.fatal = Some((EXIT_SANDBOX_LOST, r.error.clone()));
                }
                let report = if !r.submission.trim().is_empty() {
                    r.submission.trim().to_string()
                } else if !r.error.is_empty() {
                    format!("(no report; {})", r.error)
                } else {
                    last_child_message
                        .map(|m| format!("(no report; last message) {m}"))
                        .unwrap_or_else(|| "(no report)".to_string())
                };
                let report = if report.len() > SUBAGENT_REPORT_LIMIT {
                    format!(
                        "{}\n[... report cut at {SUBAGENT_REPORT_LIMIT} characters]",
                        &report[..report.floor_char_boundary(SUBAGENT_REPORT_LIMIT)]
                    )
                } else {
                    report
                };
                Observation {
                    output: format!(
                        "subagent {}: {} model calls, {} tool steps, {}s\n\n{report}",
                        r.exit_status,
                        r.calls,
                        r.steps,
                        start.elapsed().as_secs()
                    ),
                    exit_code: i32::from(r.exit_status != EXIT_SUBMITTED),
                    duration: start.elapsed(),
                    ..Observation::default()
                }
            }
            Err(e) => action_error(format!("subagent could not start: {e}"), start, 1),
        };
        self.emit_observation(SUBAGENT_TOOL, &obs);
        ToolOutcome::clean(format_observation(&obs, self.cfg.output_limit))
    }

    /// Runs the `pre_tool` hooks for a call. Some(text) is the observation
    /// of a call a hook blocked (exit [`HOOK_BLOCK_EXIT`]); the call does
    /// not run. Any other failure is noted and the call goes ahead.
    /// What stands between the model and a tool: the permission rules,
    /// then the owner's pre-tool hooks. Some(text) is the refusal the model
    /// gets; the call does not run. Remembers what the call is about for
    /// the allow rules at approval time.
    fn gates(&mut self, cancel: &CancelToken, tool: &str, arguments: &str) -> Option<String> {
        self.current_subjects = subjects_of(tool, arguments, &self.cfg.cwd);
        if let Some(rule) = self
            .permissions
            .denies_any(tool, &self.current_subjects)
            .map(Rule::pattern)
        {
            let obs = Observation {
                output: format!(
                    "{tool}: refused by permission rule `deny {rule}`. The user forbids this for the session; do not do the same thing another way - ask them or move on."
                ),
                exit_code: 126,
                ..Observation::default()
            };
            self.emit(Step {
                kind: StepKind::Error,
                content: obs.output.clone(),
                command: tool.to_string(),
                ..Step::default()
            });
            self.emit_observation(&format!("{tool} tool"), &obs);
            return Some(format_observation(&obs, self.cfg.output_limit));
        }
        self.pre_tool_hooks(cancel, tool, arguments)
    }

    fn pre_tool_hooks(
        &mut self,
        cancel: &CancelToken,
        tool: &str,
        arguments: &str,
    ) -> Option<String> {
        let hooks: Vec<Hook> = self
            .hooks
            .matching(HookEvent::PreTool, tool)
            .into_iter()
            .cloned()
            .collect();
        if hooks.is_empty() {
            return None;
        }
        let payload = tool_payload(HookEvent::PreTool, tool, arguments, None, &self.cfg.cwd);
        for hook in hooks {
            let obs = self.run_hook(cancel, &hook, tool, &payload);
            if obs.exit_code == HOOK_BLOCK_EXIT {
                let reason = cut_bytes(obs.output.trim(), HOOK_OUTPUT_LIMIT);
                let blocked = Observation {
                    output: format!(
                        "blocked by hook `{}`{}{}",
                        hook.command,
                        if reason.is_empty() { "" } else { ": " },
                        reason
                    ),
                    exit_code: 126,
                    duration: obs.duration,
                    ..Observation::default()
                };
                self.emit(Step {
                    kind: StepKind::Error,
                    content: blocked.output.clone(),
                    command: tool.to_string(),
                    ..Step::default()
                });
                self.emit_observation(&format!("{tool} tool"), &blocked);
                return Some(format_observation(&blocked, self.cfg.output_limit));
            }
            if obs.exit_code != 0 {
                self.emit(Step {
                    kind: StepKind::Note,
                    content: format!(
                        "pre_tool hook `{}` failed (exit {}); the call goes ahead: {}",
                        hook.command,
                        obs.exit_code,
                        one_line(&obs.output, 300)
                    ),
                    ..Step::default()
                });
            }
        }
        None
    }

    /// Runs the `post_tool` hooks for a call and appends what they say to
    /// the tool's result.
    fn with_post_tool_hooks(
        &mut self,
        cancel: &CancelToken,
        tool: &str,
        arguments: &str,
        text: String,
    ) -> String {
        let hooks: Vec<Hook> = self
            .hooks
            .matching(HookEvent::PostTool, tool)
            .into_iter()
            .cloned()
            .collect();
        if hooks.is_empty() {
            return text;
        }
        let (_, output, ok) = split_observation(&text);
        let output = if ok { output } else { text.clone() };
        let exit_code = observation_exit_code(&text).unwrap_or(0);
        let payload = tool_payload(
            HookEvent::PostTool,
            tool,
            arguments,
            Some((&output, exit_code)),
            &self.cfg.cwd,
        );
        let mut text = text;
        for hook in hooks {
            let obs = self.run_hook(cancel, &hook, tool, &payload);
            let said = obs.output.trim();
            if obs.exit_code == 0 && said.is_empty() {
                continue;
            }
            text.push_str(&format!(
                "\n[hook `{}`{}] {}",
                hook.command,
                if obs.exit_code == 0 {
                    String::new()
                } else {
                    format!(" exit {}", obs.exit_code)
                },
                cut_bytes(said, HOOK_OUTPUT_LIMIT)
            ));
        }
        text
    }

    /// Runs the `run_start` hooks; what they print joins the system prompt.
    fn run_start_hooks(&mut self, cancel: &CancelToken) {
        let hooks: Vec<Hook> = self
            .hooks
            .matching(HookEvent::RunStart, "")
            .into_iter()
            .cloned()
            .collect();
        if hooks.is_empty() {
            return;
        }
        let payload = run_payload(
            HookEvent::RunStart,
            &self.cfg.cwd,
            json!({
                "model": self.cfg.model,
                "task": cut_bytes(&self.cfg.task, 4000),
                "depth": self.cfg.depth,
            }),
        );
        let mut said: Vec<String> = Vec::new();
        for hook in hooks {
            let obs = self.run_hook(cancel, &hook, "", &payload);
            if obs.exit_code == 0 {
                let out = obs.output.trim();
                if !out.is_empty() {
                    said.push(cut_bytes(out, HOOK_OUTPUT_LIMIT));
                }
            } else {
                self.emit(Step {
                    kind: StepKind::Note,
                    content: format!(
                        "run_start hook `{}` failed (exit {}): {}",
                        hook.command,
                        obs.exit_code,
                        one_line(&obs.output, 300)
                    ),
                    ..Step::default()
                });
            }
        }
        if said.is_empty() {
            return;
        }
        if !self.cfg.system_prompt.ends_with('\n') {
            self.cfg.system_prompt.push('\n');
        }
        self.cfg
            .system_prompt
            .push_str(&format!("Hook output at run start:\n{}\n", said.join("\n")));
        self.emit(Step {
            kind: StepKind::Note,
            content: format!(
                "added {} characters of run_start hook output to the system prompt",
                said.iter().map(String::len).sum::<usize>()
            ),
            ..Step::default()
        });
    }

    /// Runs the `run_end` hooks with the outcome; what they say is
    /// recorded, nothing more.
    fn run_end_hooks(&mut self, cancel: &CancelToken, status: &str, submission: &str) {
        let hooks: Vec<Hook> = self
            .hooks
            .matching(HookEvent::RunEnd, "")
            .into_iter()
            .cloned()
            .collect();
        if hooks.is_empty() {
            return;
        }
        let payload = run_payload(
            HookEvent::RunEnd,
            &self.cfg.cwd,
            json!({
                "model": self.cfg.model,
                "exit_status": status,
                "calls": self.result.calls,
                "steps": self.result.steps,
                "submission": cut_bytes(submission, 4000),
                "depth": self.cfg.depth,
            }),
        );
        for hook in hooks {
            self.run_hook(cancel, &hook, "", &payload);
        }
    }

    /// Runs the `user_prompt` hooks on a session message before the model
    /// sees it. Ok(text) is the message to send, with what the hooks
    /// printed carried along; Err(reason) when one exited
    /// [`HOOK_BLOCK_EXIT`] and the message must not go.
    fn user_prompt_hooks(
        &mut self,
        cancel: &CancelToken,
        text: &str,
    ) -> std::result::Result<String, String> {
        let hooks: Vec<Hook> = self
            .hooks
            .matching(HookEvent::UserPrompt, "")
            .into_iter()
            .cloned()
            .collect();
        if hooks.is_empty() {
            return Ok(text.to_string());
        }
        let payload = run_payload(
            HookEvent::UserPrompt,
            &self.cfg.cwd,
            json!({
                "model": self.cfg.model,
                "prompt": cut_bytes(text, HOOK_PAYLOAD_LIMIT),
                "turn": self.rewind.turns().len() + 1,
                "messages": self.msgs.len(),
            }),
        );
        let mut said: Vec<String> = Vec::new();
        for hook in hooks {
            let obs = self.run_hook(cancel, &hook, "", &payload);
            let out = obs.output.trim();
            if obs.exit_code == HOOK_BLOCK_EXIT {
                let reason = format!(
                    "user_prompt hook `{}` kept the message back{}",
                    hook.command,
                    if out.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", cut_bytes(out, HOOK_OUTPUT_LIMIT))
                    }
                );
                self.emit(Step {
                    kind: StepKind::Note,
                    content: reason.clone(),
                    ..Step::default()
                });
                return Err(reason);
            }
            if obs.exit_code == 0 {
                if !out.is_empty() {
                    said.push(format!(
                        "--- user_prompt hook `{}` ---\n{}\n--- end of hook output ---",
                        hook.command,
                        cut_bytes(out, HOOK_OUTPUT_LIMIT)
                    ));
                }
            } else {
                self.emit(Step {
                    kind: StepKind::Note,
                    content: format!(
                        "user_prompt hook `{}` failed (exit {}): {}",
                        hook.command,
                        obs.exit_code,
                        one_line(&obs.output, 300)
                    ),
                    ..Step::default()
                });
            }
        }
        if said.is_empty() {
            Ok(text.to_string())
        } else {
            Ok(format!("{text}\n\n{}", said.join("\n\n")))
        }
    }

    /// Runs the `pre_compact` hooks. Ok(asks) is what they want the
    /// summary to keep (empty for nothing); Err(reason) when one exited
    /// [`HOOK_BLOCK_EXIT`] and the compaction is declined.
    fn pre_compact_hooks(
        &mut self,
        cancel: &CancelToken,
        trigger: &str,
        compacting: usize,
    ) -> std::result::Result<String, String> {
        let hooks: Vec<Hook> = self
            .hooks
            .matching(HookEvent::PreCompact, "")
            .into_iter()
            .cloned()
            .collect();
        if hooks.is_empty() {
            return Ok(String::new());
        }
        let payload = run_payload(
            HookEvent::PreCompact,
            &self.cfg.cwd,
            json!({
                "model": self.cfg.model,
                "trigger": trigger,
                "messages": self.msgs.len(),
                "compacting": compacting,
                "prompt_tokens": self.state.last_prompt_tokens,
                "context_window": self.cfg.context_window,
            }),
        );
        let mut asks: Vec<String> = Vec::new();
        for hook in hooks {
            let obs = self.run_hook(cancel, &hook, "", &payload);
            let out = obs.output.trim();
            if obs.exit_code == HOOK_BLOCK_EXIT {
                return Err(format!(
                    "pre_compact hook `{}` declined the compaction{}",
                    hook.command,
                    if out.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", one_line(out, 300))
                    }
                ));
            }
            if obs.exit_code == 0 {
                if !out.is_empty() {
                    asks.push(cut_bytes(out, HOOK_OUTPUT_LIMIT));
                }
            } else {
                self.emit(Step {
                    kind: StepKind::Note,
                    content: format!(
                        "pre_compact hook `{}` failed (exit {}): {}",
                        hook.command,
                        obs.exit_code,
                        one_line(&obs.output, 300)
                    ),
                    ..Step::default()
                });
            }
        }
        Ok(asks.join("\n"))
    }

    /// Runs one hook in the run's environment and records it as a step.
    fn run_hook(
        &mut self,
        cancel: &CancelToken,
        hook: &Hook,
        tool: &str,
        payload: &Value,
    ) -> Observation {
        self.emit(Step {
            kind: StepKind::Hook,
            content: hook.command.clone(),
            command: if tool.is_empty() {
                hook.event.to_string()
            } else {
                format!("{} {tool}", hook.event)
            },
            ..Step::default()
        });
        let line = hook_command(hook, tool, payload);
        let obs = self.env.execute(cancel, &line, hook.timeout);
        self.emit_observation(&format!("hook {}", hook.event), &obs);
        obs
    }

    /// Replaces the older part of the conversation with a summary the
    /// model writes. None when the conversation is too short to compact;
    /// Some(true) when it changed.
    fn compact(&mut self, cancel: &CancelToken, trigger: &str) -> Option<bool> {
        let cut = compaction_cut(&self.msgs, COMPACT_KEEP_RECENT)?;
        let asks = match self.pre_compact_hooks(cancel, trigger, cut - 2) {
            Ok(asks) => asks,
            Err(reason) => {
                self.emit(Step {
                    kind: StepKind::Note,
                    content: reason,
                    ..Step::default()
                });
                return Some(false);
            }
        };
        let transcript = render_transcript(&self.msgs, cut);
        let req = summary_request(&self.cfg.model, &self.cfg.task, &transcript, &asks);
        let start = Instant::now();
        let resp = match self.p.chat(cancel, req) {
            Ok(r) => r,
            Err(e) => {
                self.emit(Step {
                    kind: StepKind::Note,
                    content: format!(
                        "context compaction failed; going on with the pruned history: {}",
                        one_line(&e.to_string(), 300)
                    ),
                    ..Step::default()
                });
                return Some(false);
            }
        };
        self.result.usage.add(resp.usage);
        let summary = resp
            .message
            .map(|m| m.content.trim().to_string())
            .unwrap_or_default();
        if summary.is_empty() {
            self.emit(Step {
                kind: StepKind::Note,
                content: "context compaction failed: the model returned no summary".to_string(),
                ..Step::default()
            });
            return Some(false);
        }
        let before = self.msgs.len();
        let replaced = cut - 2;
        self.msgs = apply_compaction(&self.msgs, cut, &summary);
        self.rewind.on_compaction(cut);
        self.result.compactions += 1;
        self.emit(Step {
            kind: StepKind::Note,
            content: format!(
                "compacted {replaced} older messages into a {}-character summary ({before} messages became {}); the system prompt, the task and the last {} messages are unchanged",
                summary.len(),
                self.msgs.len(),
                before - cut
            ),
            usage: resp.usage,
            duration_ms: start.elapsed().as_millis() as i64,
            ..Step::default()
        });
        Some(true)
    }

    fn tool_error(&mut self, text: String) -> ToolOutcome {
        self.emit(Step {
            kind: StepKind::Error,
            content: text.clone(),
            ..Step::default()
        });
        ToolOutcome {
            text,
            clean: false,
            submitted: false,
            submission: String::new(),
        }
    }

    fn run_mcp_tool(&mut self, name: &str, arguments: &str) -> ToolOutcome {
        let args: Value = match serde_json::from_str(arguments) {
            Ok(Value::Null) => json!({}),
            Ok(v) if v.is_object() => v,
            Ok(_) => return self.tool_error(format!("{name}: arguments must be a JSON object")),
            Err(e) if arguments.trim().is_empty() => {
                let _ = e;
                json!({})
            }
            Err(e) => return self.tool_error(format!("{name}: arguments are not JSON: {e}")),
        };
        self.result.steps += 1;
        self.emit(Step {
            kind: StepKind::Mcp,
            content: format!("{name} {}", compact_json(&args, 2000)),
            command: name.to_string(),
            ..Step::default()
        });
        let start = Instant::now();
        if self.ui.is_some()
            && let Some(declined) = self.approval(
                name,
                name,
                &compact_json(&args, 2000),
                "an MCP tool may do anything its server allows",
            )
        {
            self.emit_observation(name, &declined);
            return ToolOutcome::clean(format_observation(&declined, self.cfg.output_limit));
        }
        let obs = match self.mcp.call(name, args) {
            Ok(r) => Observation {
                output: r.text,
                exit_code: i32::from(r.is_error),
                duration: start.elapsed(),
                ..Observation::default()
            },
            Err(e) => Observation {
                output: format!("{name}: {}", one_line(&e.to_string(), 2000)),
                exit_code: 1,
                duration: start.elapsed(),
                ..Observation::default()
            },
        };
        self.emit_observation(name, &obs);
        ToolOutcome::clean(format_observation(&obs, self.cfg.output_limit))
    }

    fn run_command(
        &mut self,
        cancel: &CancelToken,
        command: &str,
    ) -> (String, Observation, bool, String) {
        self.run_command_opts(cancel, command, None, "")
    }

    /// Runs a shell command through the policy, read-only and approval
    /// gates; `background` (the initial wait) starts it as a background
    /// process instead of waiting for it; `title` is the agent's own
    /// sentence about what the command does.
    fn run_command_opts(
        &mut self,
        cancel: &CancelToken,
        command: &str,
        background: Option<Duration>,
        title: &str,
    ) -> (String, Observation, bool, String) {
        self.result.steps += 1;
        let shown = match background {
            Some(_) => format!("{command} &"),
            None => command.to_string(),
        };
        self.emit(Step {
            kind: StepKind::Command,
            content: shown.clone(),
            command: shown,
            title: title.to_string(),
            ..Step::default()
        });
        if let Some(rule) = self.denied(command) {
            let description = rule.description.clone();
            if self.ui.is_none() {
                let obs = Observation {
                    output: format!(
                        "refused by policy: {description}. Never push, never add remotes, never call gh, never touch credentials (job rule 3)."
                    ),
                    exit_code: 126,
                    ..Observation::default()
                };
                self.emit(Step {
                    kind: StepKind::Error,
                    content: obs.output.clone(),
                    command: command.to_string(),
                    ..Step::default()
                });
                self.emit_observation(command, &obs);
                return (
                    format_observation(&obs, self.cfg.output_limit),
                    obs,
                    false,
                    String::new(),
                );
            }
            // With a person at the keyboard the policy line is theirs to
            // cross: a push or a gh call they asked for goes through them.
            if let Some(obs) = self.approval(
                "bash",
                command,
                "",
                &format!("crosses the job policy line: {description}"),
            ) {
                self.emit_observation(command, &obs);
                return (
                    format_observation(&obs, self.cfg.output_limit),
                    obs,
                    false,
                    String::new(),
                );
            }
        } else if self.ui.is_some()
            && !self.cfg.read_only
            && let Some(reason) = crate::readonly::read_only_refusal(command)
            && let Some(obs) = self.approval(
                "bash",
                command,
                "",
                &reason.replace(" (a read-only run lands nothing)", ""),
            )
        {
            self.emit_observation(command, &obs);
            return (
                format_observation(&obs, self.cfg.output_limit),
                obs,
                false,
                String::new(),
            );
        }
        if self.cfg.read_only
            && let Some(reason) = crate::readonly::read_only_refusal(command)
        {
            let obs = Observation {
                output: format!(
                    "refused: this is a read-only run and {reason}. {READ_ONLY_REMINDER}"
                ),
                exit_code: 126,
                ..Observation::default()
            };
            self.emit(Step {
                kind: StepKind::Error,
                content: obs.output.clone(),
                command: command.to_string(),
                ..Step::default()
            });
            self.emit_observation(command, &obs);
            return (
                format_observation(&obs, self.cfg.output_limit),
                obs,
                false,
                String::new(),
            );
        }
        if let Some(wait) = background {
            let obs = self.start_background(cancel, command, wait);
            self.emit_observation(&format!("{command} &"), &obs);
            return (
                format_observation(&obs, self.cfg.output_limit),
                obs,
                false,
                String::new(),
            );
        }
        let gate = self.verify_gate_for(command);
        let timeout = match &gate {
            Some(_) => self.commit_deadline(),
            None => self.cfg.step_timeout,
        };
        let snapshot = if self.cfg.read_only {
            crate::readonly::TreeSnapshot::take(Path::new(if self.cfg.cwd.is_empty() {
                "."
            } else {
                &self.cfg.cwd
            }))
        } else {
            None
        };
        let mut obs = self.env.execute(cancel, command, timeout);
        if let Some(snapshot) = snapshot {
            let reverted = snapshot.restore();
            if !reverted.is_empty() {
                let note = format!(
                    "[read-only run: the command changed the clone ({}); the change was put back. {READ_ONLY_REMINDER}]",
                    reverted.join(", ")
                );
                if !obs.output.is_empty() && !obs.output.ends_with('\n') {
                    obs.output.push('\n');
                }
                obs.output.push_str(&note);
                self.emit(Step {
                    kind: StepKind::Error,
                    content: note,
                    command: command.to_string(),
                    ..Step::default()
                });
            }
        }
        self.emit_observation(command, &obs);
        if obs.exit_code == 127 && self.sandbox_lost() {
            let msg = format!(
                "the run's working directory {} no longer exists; every command would fail the same way (`{}`), so the run ends here instead of spending its budget on it",
                self.cfg.cwd,
                one_line(&obs.output, 120)
            );
            self.emit(Step {
                kind: StepKind::Error,
                content: msg.clone(),
                command: command.to_string(),
                ..Step::default()
            });
            self.fatal = Some((EXIT_SANDBOX_LOST, msg));
        }
        let formatted = match gate {
            _ if obs.timed_out && cancel.is_cancelled() => self.observation_text(cancel, &obs),
            Some(gate) if obs.timed_out => {
                format_commit_timeout(&obs, self.cfg.output_limit, &gate)
            }
            _ => format_observation(&obs, self.cfg.output_limit),
        };
        if obs.exit_code == 0
            && !obs.timed_out
            && let Some(sub) = submission_from_observation(&obs.output)
        {
            return (formatted, obs, true, sub);
        }
        (formatted, obs, false, String::new())
    }

    /// Starts `command` in the background and gives it `wait` to end on
    /// its own: a command that does comes back whole, like a foreground
    /// one; the rest stays running under an id the `process` tool takes.
    fn start_background(
        &mut self,
        cancel: &CancelToken,
        command: &str,
        wait: Duration,
    ) -> Observation {
        let start = Instant::now();
        let running = self.procs.running();
        if running >= BACKGROUND_LIMIT {
            let ids = self.procs.ids().join(", ");
            return Observation {
                output: format!(
                    "{running} background processes are running already ({ids}); stop one with the process tool before starting another."
                ),
                exit_code: 1,
                duration: start.elapsed(),
                ..Observation::default()
            };
        }
        let proc = match self.env.spawn(command) {
            Ok(p) => p,
            Err(e) => {
                return Observation {
                    output: format!("could not start the command: {e}"),
                    exit_code: 127,
                    duration: start.elapsed(),
                    ..Observation::default()
                };
            }
        };
        let id = self.procs.add(command, proc);
        let wait = wait.min(self.step_cap());
        let m = self.procs.get(&id).expect("just added");
        let ended = m.wait(cancel, wait);
        let pid = m.pid();
        let (mut output, dropped) = m.take_output();
        if dropped > 0 {
            output = format!("{}\n{output}", dropped_marker(dropped));
        }
        if ended {
            let code = m.exit().unwrap_or(-1);
            let elapsed = m.elapsed();
            self.procs.remove(&id);
            let line = format!(
                "[{id} ended on its own with exit {code} after {}; nothing is left running]",
                fmt_dur(elapsed)
            );
            return Observation {
                output: join_lines(&output, &line),
                exit_code: code,
                duration: start.elapsed(),
                ..Observation::default()
            };
        }
        let line = format!(
            "[{id} is running in the background (pid {pid}) after {}. process {{\"action\":\"output\",\"id\":\"{id}\"}} reads what it writes next, \"wait\" waits for it to end, \"stop\" ends it. Stop it when you are done with it.]",
            fmt_dur(wait)
        );
        Observation {
            output: join_lines(&line, &output),
            exit_code: 0,
            duration: start.elapsed(),
            ..Observation::default()
        }
    }

    /// The most a background wait may block: the step timeout, or
    /// [`BACKGROUND_WAIT_LIMIT`] when the run has none.
    fn step_cap(&self) -> Duration {
        if self.cfg.step_timeout.is_zero() {
            BACKGROUND_WAIT_LIMIT
        } else {
            self.cfg.step_timeout.min(BACKGROUND_WAIT_LIMIT)
        }
    }

    /// The `process` tool: output, wait, stop or list of the background
    /// processes.
    fn run_process_tool(&mut self, cancel: &CancelToken, arguments: &str) -> ToolOutcome {
        let action = match ProcessAction::parse(arguments) {
            Ok(a) => a,
            Err(e) => return self.tool_error(format!("{PROCESS_TOOL}: {e}")),
        };
        self.result.steps += 1;
        let spec = action.spec();
        self.emit(Step {
            kind: StepKind::Process,
            content: spec.clone(),
            command: spec,
            ..Step::default()
        });
        let start = Instant::now();
        let obs = self.process_action(cancel, &action, start);
        self.emit_observation(&format!("{PROCESS_TOOL} tool"), &obs);
        ToolOutcome::clean(format_observation(&obs, self.cfg.output_limit))
    }

    fn process_action(
        &mut self,
        cancel: &CancelToken,
        action: &ProcessAction,
        start: Instant,
    ) -> Observation {
        let unknown = |procs: &mut Procs, id: &str, start: Instant| Observation {
            output: if procs.is_empty() {
                format!(
                    "{id}: no such background process; none is running (bash background:true starts one)"
                )
            } else {
                format!(
                    "{id}: no such background process; the list has {}",
                    procs.ids().join(", ")
                )
            },
            exit_code: 1,
            duration: start.elapsed(),
            ..Observation::default()
        };
        match action {
            ProcessAction::List => {
                let lines = self.procs.describe();
                Observation {
                    output: if lines.is_empty() {
                        "no background processes".to_string()
                    } else {
                        lines.join("\n")
                    },
                    exit_code: 0,
                    duration: start.elapsed(),
                    ..Observation::default()
                }
            }
            ProcessAction::Output { id } | ProcessAction::Wait { id, .. } => {
                let timeout = match action {
                    ProcessAction::Wait { timeout, .. } => {
                        Some(timeout.unwrap_or(PROCESS_WAIT_DEFAULT).min(self.step_cap()))
                    }
                    _ => None,
                };
                let Some(m) = self.procs.get(id) else {
                    return unknown(&mut self.procs, id, start);
                };
                let mut waited = String::new();
                if let Some(t) = timeout
                    && !m.wait(cancel, t)
                {
                    waited = if cancel.is_cancelled() {
                        " (the wait was interrupted)".to_string()
                    } else {
                        format!(" (still running after waiting {})", fmt_dur(t))
                    };
                }
                let state = m.state();
                let (output, dropped) = m.take_output();
                let code = m.exit();
                let head = format!(
                    "[{id} `{}`: {state}{waited}; {}]",
                    one_line(&m.command, 100),
                    if output.is_empty() {
                        "no new output".to_string()
                    } else {
                        format!("{} new bytes", output.len())
                    }
                );
                let mut text = head;
                if dropped > 0 {
                    text = join_lines(&text, &dropped_marker(dropped));
                }
                Observation {
                    output: join_lines(&text, &output),
                    exit_code: code.unwrap_or(0),
                    duration: start.elapsed(),
                    ..Observation::default()
                }
            }
            ProcessAction::Stop { id } => {
                let Some(m) = self.procs.get(id) else {
                    return unknown(&mut self.procs, id, start);
                };
                let was_running = m.stop();
                let (output, dropped) = m.take_output();
                let code = m.exit().unwrap_or(-1);
                let elapsed = fmt_dur(m.elapsed());
                let command = one_line(&m.command, 100);
                self.procs.remove(id);
                let head = if was_running {
                    format!(
                        "[{id} `{command}`: stopped after {elapsed} (exit {code}); {}]",
                        if output.is_empty() {
                            "no unread output"
                        } else {
                            "its last output follows"
                        }
                    )
                } else {
                    format!(
                        "[{id} `{command}`: had already ended with exit {code} after {elapsed}; removed from the list{}]",
                        if output.is_empty() {
                            ""
                        } else {
                            "; its last output follows"
                        }
                    )
                };
                let mut text = head;
                if dropped > 0 {
                    text = join_lines(&text, &dropped_marker(dropped));
                }
                Observation {
                    output: join_lines(&text, &output),
                    exit_code: 0,
                    duration: start.elapsed(),
                    ..Observation::default()
                }
            }
        }
    }

    fn denied(&self, command: &str) -> Option<&DenyRule> {
        self.deny.iter().find(|r| r.re.is_match(command))
    }

    /// True when the configured working directory existed at start and is
    /// gone now. Exit 127 alone is not enough: `command not found` uses it too.
    fn sandbox_lost(&self) -> bool {
        !self.cfg.cwd.is_empty() && !Path::new(&self.cfg.cwd).is_dir()
    }

    /// A fenced ```read/```grep/```edit block: the spec is parsed, then run
    /// like the typed tool of the same name.
    fn run_native_action(&mut self, action: NativeAction, spec: &str) -> (String, bool, String) {
        self.result.steps += 1;
        self.emit(Step {
            kind: action.step_kind(),
            content: spec.to_string(),
            command: spec.to_string(),
            ..Step::default()
        });
        let start = Instant::now();
        let (obs, call) = match NativeCall::from_fenced(action, spec) {
            Ok(call) => (self.execute_native(&call, spec), Some(call)),
            Err(e) => (action_error(e, start, 1), None),
        };
        let command = format!("{} action", action.name());
        self.emit_observation(&command, &obs);
        let mut formatted = format_observation(&obs, self.cfg.output_limit);
        if let Some(call) = call
            && obs.exit_code == 0
        {
            formatted = self.with_path_instructions(&call, formatted);
        }
        (formatted, false, String::new())
    }

    /// A typed tool call (read, grep, glob, edit, write, plan).
    fn run_native_call(&mut self, call: &NativeCall) -> String {
        self.result.steps += 1;
        let spec = call.spec();
        self.emit(Step {
            kind: call.step_kind(),
            content: spec.clone(),
            command: spec.clone(),
            ..Step::default()
        });
        let obs = self.execute_native(call, &spec);
        let command = format!("{} tool", call.name());
        self.emit_observation(&command, &obs);
        let text = format_observation(&obs, self.cfg.output_limit);
        if obs.exit_code == 0 {
            self.with_path_instructions(call, text)
        } else {
            text
        }
    }

    /// Runs a parsed native call; in a read-only run `edit` and `write`
    /// are refused instead, and in a session the user sees them first.
    fn execute_native(&mut self, call: &NativeCall, spec: &str) -> Observation {
        if self.cfg.read_only && call.writes() {
            let obs = Observation {
                output: format!(
                    "{}: refused: this is a read-only run; {READ_ONLY_REMINDER}",
                    call.name()
                ),
                exit_code: 126,
                ..Observation::default()
            };
            self.emit(Step {
                kind: StepKind::Error,
                content: obs.output.clone(),
                command: spec.to_string(),
                ..Step::default()
            });
            return obs;
        }
        if call.writes()
            && self.ui.is_some()
            && let Some((summary, detail, reason)) = call.approval_preview(&self.cfg.cwd)
            && let Some(obs) = self.approval(call.name(), &summary, &detail, &reason)
        {
            return obs;
        }
        if call.writes()
            && let Some(path) = call.path()
            && let Ok(resolved) = self.fs().resolve_action_path(path, true)
        {
            self.rewind.capture(&resolved.rel);
        }
        self.fs().execute(call)
    }

    /// The file side of the native tools, borrowed from the agent.
    fn fs(&self) -> NativeFs<'_> {
        NativeFs {
            cwd: &self.cfg.cwd,
            deny: &self.deny,
        }
    }

    /// The verify gate a command would trigger: `Some(gate)` when the command
    /// runs a git subcommand that fires the pre-commit hook and the clone's
    /// hook carries [`VERIFY_GATE_HOOK_MARKER`].
    fn verify_gate_for(&self, command: &str) -> Option<VerifyGate> {
        if !runs_git_commit(command) {
            return None;
        }
        let dir = if self.cfg.cwd.is_empty() {
            std::env::current_dir().ok()?
        } else {
            std::path::PathBuf::from(&self.cfg.cwd)
        };
        verify_gate_in(&dir)
    }

    /// Deadline for a gated commit: `commit_timeout`, never below
    /// `step_timeout`, capped by what remains of the wall clock (with a
    /// small reserve so the model can still report).
    fn commit_deadline(&self) -> Duration {
        let mut d = self.cfg.commit_timeout.max(self.cfg.step_timeout);
        if !self.cfg.wall_time.is_zero() {
            let left = self
                .cfg
                .wall_time
                .saturating_sub(self.clock_elapsed())
                .saturating_sub(Duration::from_secs(60));
            d = d.min(left.max(self.cfg.step_timeout));
        }
        d
    }

    fn emit_observation(&mut self, command: &str, obs: &Observation) {
        self.emit(Step {
            kind: StepKind::Observation,
            content: stored_observation(&obs.output, self.cfg.output_limit),
            command: command.to_string(),
            exit_code: Some(obs.exit_code),
            duration_ms: obs.duration.as_millis() as i64,
            ..Step::default()
        });
    }

    fn finish(&mut self, status: &str, submission: &str, err_text: &str) -> ResultSummary {
        self.ended = true;
        self.result.exit_status = status.to_string();
        self.result.submission = submission.to_string();
        self.result.error = err_text.to_string();
        // A fresh token: the end hooks run even when the run was cancelled.
        self.run_end_hooks(&CancelToken::new(), status, submission);
        let stopped = self.procs.stop_all();
        if !stopped.is_empty() {
            self.emit(Step {
                kind: StepKind::Note,
                content: format!(
                    "stopped {} background process(es) still running: {}",
                    stopped.len(),
                    stopped.join("; ")
                ),
                ..Step::default()
            });
        }
        let stragglers = self.env.close();
        if !stragglers.is_empty() {
            self.emit(Step {
                kind: StepKind::Note,
                content: format!(
                    "killed {} process(es) the run left under its clone: {}",
                    stragglers.len(),
                    pulse_cli::procs::describe_killed(&stragglers)
                ),
                ..Step::default()
            });
        }
        self.result.elapsed = self.started.elapsed();
        self.emit(Step {
            kind: StepKind::Exit,
            content: status.to_string(),
            ..Step::default()
        });
        self.result.clone()
    }

    pub(crate) fn emit(&mut self, mut s: Step) {
        s.n = self.next_n;
        self.next_n += 1;
        s.at = now_rfc3339();
        s.depth = self.cfg.depth;
        self.steps.push(s.clone());
        for sink in &mut self.sinks {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                sink.step(s.clone());
            }));
        }
    }
}

/// One call of a reply that can run beside the others: a native read-side
/// call or a fetch. It carries everything its thread needs.
enum ParallelCall {
    Native(NativeCall),
    Fetch(String),
    Search { query: String, count: usize },
}

impl ParallelCall {
    fn name(&self) -> &'static str {
        match self {
            Self::Native(c) => c.name(),
            Self::Fetch(_) => FETCH_TOOL,
            Self::Search { .. } => WEB_SEARCH_TOOL,
        }
    }

    fn step_kind(&self) -> StepKind {
        match self {
            Self::Native(c) => c.step_kind(),
            Self::Fetch(_) => StepKind::Fetch,
            Self::Search { .. } => StepKind::Search,
        }
    }

    fn spec(&self) -> String {
        match self {
            Self::Native(c) => c.spec(),
            Self::Fetch(url) => url.clone(),
            Self::Search { query, .. } => query.clone(),
        }
    }

    /// The `command` of the Observation step, as the sequential path
    /// records it.
    fn observation_command(&self) -> String {
        format!("{} tool", self.name())
    }

    /// The Error step the sequential path would emit for this result, if
    /// any (a fetch the policy refused).
    fn error_step(&self, obs: &Observation) -> Option<String> {
        match self {
            Self::Fetch(_) if obs.exit_code == crate::fetch::FETCH_REFUSED_EXIT => {
                Some(obs.output.clone())
            }
            _ => None,
        }
    }

    fn execute(&self, fs: NativeFs<'_>, policy: &FetchPolicy, search: &WebSearch) -> Observation {
        match self {
            Self::Native(c) => fs.execute(c),
            Self::Fetch(url) => {
                let start = Instant::now();
                let r = fetch(url, policy, FETCH_BYTE_LIMIT, FETCH_TIMEOUT);
                Observation {
                    output: r.output,
                    exit_code: r.exit_code,
                    duration: start.elapsed(),
                    ..Observation::default()
                }
            }
            Self::Search { query, count } => {
                let start = Instant::now();
                let r = search.search(query, *count, SEARCH_TIMEOUT);
                Observation {
                    output: r.output,
                    exit_code: r.exit_code,
                    duration: start.elapsed(),
                    ..Observation::default()
                }
            }
        }
    }
}

/// The file side of the native tools: the clone root and the deny rules,
/// and nothing of the agent's mutable state, so several calls of one reply
/// can run on separate threads at once.
#[derive(Clone, Copy)]
struct NativeFs<'a> {
    cwd: &'a str,
    deny: &'a [DenyRule],
}

impl NativeFs<'_> {
    fn execute(&self, call: &NativeCall) -> Observation {
        match call {
            NativeCall::Read(targets) => self.native_read(targets),
            NativeCall::Grep { pattern, scope } => self.native_grep(pattern, scope),
            NativeCall::Glob { pattern, scope } => self.native_glob(pattern, scope),
            NativeCall::Edit { path, blocks } => self.native_edit(path, blocks),
            NativeCall::Write {
                path,
                content,
                overwrite,
            } => self.native_write(path, content, *overwrite),
            NativeCall::Plan(items) => {
                let start = Instant::now();
                Observation {
                    output: render_plan(items),
                    duration: start.elapsed(),
                    ..Observation::default()
                }
            }
        }
    }

    fn denied(&self, text: &str) -> Option<&DenyRule> {
        self.deny.iter().find(|r| r.re.is_match(text))
    }

    fn native_read(&self, targets: &[ReadTarget]) -> Observation {
        let start = Instant::now();
        if targets.is_empty() {
            return action_error("read: missing path".into(), start, 1);
        }
        let mut out = String::new();
        for target in targets {
            let path = target.path.as_str();
            let resolved = match self.resolve_action_path(path, false) {
                Ok(p) => p,
                Err(e) => return action_error(e, start, 126),
            };
            if !resolved.path.exists() {
                return action_error(format!("read: {} does not exist", path), start, 1);
            }
            if resolved.path.is_dir() {
                return action_error(
                    format!("read: {} is a directory; use glob to list it", path),
                    start,
                    1,
                );
            }
            let data = match fs::read(&resolved.path) {
                Ok(d) => d,
                Err(e) => return action_error(format!("read {}: {e}", path), start, 1),
            };
            out.push_str(&format!("==> {} <==\n", resolved.rel));
            if data.contains(&0) {
                out.push_str(&format!("binary file, {} bytes\n", data.len()));
                continue;
            }
            let text = match String::from_utf8(data) {
                Ok(t) => t,
                Err(e) => {
                    out.push_str(&format!("binary file, {} bytes\n", e.as_bytes().len()));
                    continue;
                }
            };
            let total = text.lines().count();
            for (idx, line) in text.lines().enumerate() {
                let n = idx + 1;
                if let Some((start_line, end_line)) = target.range
                    && (n < start_line || n > end_line)
                {
                    continue;
                }
                out.push_str(&format!("{n:>6}\t{line}\n"));
            }
            if let Some((start_line, end_line)) = target.range
                && end_line < total
            {
                out.push_str(&format!("[lines {start_line}-{end_line} of {total}]\n"));
            }
        }
        Observation {
            output: out,
            duration: start.elapsed(),
            ..Observation::default()
        }
    }

    fn native_grep(&self, pattern: &str, scope: &str) -> Observation {
        let start = Instant::now();
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return action_error("grep: missing regex pattern".into(), start, 1);
        }
        let re = match Regex::new(pattern) {
            Ok(r) => r,
            Err(e) => return action_error(format!("grep: invalid regex: {e}"), start, 1),
        };
        let scope = scope.trim();
        let scope = if scope.is_empty() { "." } else { scope };
        let root = match self.resolve_action_path(".", false) {
            Ok(p) => p.path,
            Err(e) => return action_error(e, start, 126),
        };
        let mut files = Vec::new();
        if scope.contains('*') {
            collect_files(&root, &root, &mut files);
            files.retain(|p| {
                p.strip_prefix(&root)
                    .ok()
                    .and_then(|r| r.to_str())
                    .is_some_and(|r| glob_match(scope, r))
            });
        } else {
            let resolved = match self.resolve_action_path(scope, false) {
                Ok(p) => p,
                Err(e) => return action_error(e, start, 126),
            };
            if resolved.path.is_file() {
                files.push(resolved.path);
            } else {
                collect_files(&root, &resolved.path, &mut files);
            }
        }
        let mut out = String::new();
        for file in files {
            let rel = file
                .strip_prefix(&root)
                .ok()
                .and_then(|p| p.to_str())
                .unwrap_or("")
                .to_string();
            if self.denied(&rel).is_some() {
                continue;
            }
            let Ok(data) = fs::read_to_string(&file) else {
                continue;
            };
            for (idx, line) in data.lines().enumerate() {
                if re.is_match(line) {
                    out.push_str(&format!("{}:{}: {}\n", rel, idx + 1, line));
                }
            }
        }
        Observation {
            output: out,
            duration: start.elapsed(),
            ..Observation::default()
        }
    }

    fn native_glob(&self, pattern: &str, scope: &str) -> Observation {
        let start = Instant::now();
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return action_error("glob: missing pattern".into(), start, 1);
        }
        let scope = scope.trim();
        let scope = if scope.is_empty() { "." } else { scope };
        let root = match self.resolve_action_path(".", false) {
            Ok(p) => p.path,
            Err(e) => return action_error(e, start, 126),
        };
        let dir = match self.resolve_action_path(scope, false) {
            Ok(p) => p,
            Err(e) => return action_error(e, start, 126),
        };
        if !dir.path.is_dir() {
            return action_error(
                format!("glob: {} is not a directory inside the clone", scope),
                start,
                1,
            );
        }
        let base = fs::canonicalize(&dir.path).unwrap_or(dir.path.clone());
        let mut files = Vec::new();
        collect_files(&root, &base, &mut files);
        let mut matched: Vec<String> = files
            .iter()
            .filter_map(|p| {
                let under = p.strip_prefix(&base).ok()?.to_str()?;
                if !glob_match(pattern, under) {
                    return None;
                }
                let rel = p.strip_prefix(&root).ok()?.to_str()?.to_string();
                (self.denied(&rel).is_none()).then_some(rel)
            })
            .collect();
        matched.sort();
        let total = matched.len();
        matched.truncate(GLOB_RESULT_LIMIT);
        let mut out = matched.join("\n");
        if total == 0 {
            out = format!("no file under {scope} matches {pattern}");
        } else if total > GLOB_RESULT_LIMIT {
            out.push_str(&format!(
                "\n[... {} more files; narrow the pattern or the path ...]",
                total - GLOB_RESULT_LIMIT
            ));
        }
        out.push('\n');
        Observation {
            output: out,
            duration: start.elapsed(),
            ..Observation::default()
        }
    }

    fn native_edit(&self, path: &str, blocks: &[EditBlock]) -> Observation {
        let start = Instant::now();
        let path = path.trim();
        if path.is_empty() {
            return action_error("edit: missing path".into(), start, 1);
        }
        if blocks.is_empty() {
            return action_error("edit: missing SEARCH/REPLACE block".into(), start, 1);
        }
        let create = !Path::new(self.cwd).join(path).exists()
            && blocks.len() == 1
            && blocks[0].search.is_empty();
        let resolved = match self.resolve_action_path(path, create) {
            Ok(p) => p,
            Err(e) => return action_error(e, start, 126),
        };
        if create {
            if let Err(e) = create_parent(&resolved.path)
                .and_then(|()| fs::write(&resolved.path, &blocks[0].replace))
            {
                return action_error(format!("edit {}: {e}", resolved.rel), start, 1);
            }
            return Observation {
                output: format!("created {}\n", resolved.rel),
                duration: start.elapsed(),
                ..Observation::default()
            };
        }
        let mut text = match fs::read_to_string(&resolved.path) {
            Ok(t) => t,
            Err(e) => return action_error(format!("edit {}: {e}", resolved.rel), start, 1),
        };
        for block in blocks {
            if block.search.is_empty() {
                return action_error(
                    format!(
                        "edit {}: empty SEARCH only creates a new file",
                        resolved.rel
                    ),
                    start,
                    1,
                );
            }
            let count = text.matches(&block.search).count();
            if count != 1 {
                let nearest = nearest_line(&text, &block.search);
                return action_error(
                    format!(
                        "edit {}: SEARCH matched {count} times, want exactly 1 (nearest line {nearest})",
                        resolved.rel
                    ),
                    start,
                    1,
                );
            }
            text = text.replacen(&block.search, &block.replace, 1);
        }
        if let Err(e) = fs::write(&resolved.path, text) {
            return action_error(format!("edit {}: {e}", resolved.rel), start, 1);
        }
        Observation {
            output: format!("edited {}\n", resolved.rel),
            duration: start.elapsed(),
            ..Observation::default()
        }
    }

    fn native_write(&self, path: &str, content: &str, overwrite: bool) -> Observation {
        let start = Instant::now();
        let path = path.trim();
        if path.is_empty() {
            return action_error("write: missing path".into(), start, 1);
        }
        let resolved = match self.resolve_action_path(path, true) {
            Ok(p) => p,
            Err(e) => return action_error(e, start, 126),
        };
        if resolved.path.is_dir() {
            return action_error(format!("write {}: is a directory", resolved.rel), start, 1);
        }
        let existed = resolved.path.exists();
        if existed && !overwrite {
            return action_error(
                format!(
                    "write {}: file exists; use edit to change it, or write with overwrite true to replace it",
                    resolved.rel
                ),
                start,
                1,
            );
        }
        if let Err(e) =
            create_parent(&resolved.path).and_then(|()| fs::write(&resolved.path, content))
        {
            return action_error(format!("write {}: {e}", resolved.rel), start, 1);
        }
        Observation {
            output: format!(
                "{} {} ({} bytes)\n",
                if existed { "replaced" } else { "wrote" },
                resolved.rel,
                content.len()
            ),
            duration: start.elapsed(),
            ..Observation::default()
        }
    }

    fn resolve_action_path(
        &self,
        rel: &str,
        allow_create: bool,
    ) -> std::result::Result<ResolvedPath, String> {
        let rel = rel.trim();
        if rel.is_empty() {
            return Err("path is empty".into());
        }
        let rel_path = Path::new(rel);
        if rel_path.is_absolute()
            || rel_path.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(format!(
                "path {rel:?} must be relative and stay inside the clone"
            ));
        }
        if let Some(rule) = self.denied(rel) {
            return Err(format!(
                "refused by policy: {}. Native actions use the same deny rules as shell commands.",
                rule.description
            ));
        }
        let root = fs::canonicalize(if self.cwd.is_empty() { "." } else { self.cwd })
            .map_err(|e| format!("canonicalize clone root: {e}"))?;
        let path = root.join(rel_path);
        // A path to create is confined through its nearest existing
        // ancestor, so a new file in a new directory still resolves.
        let check = if allow_create {
            let mut anc = path.parent().unwrap_or(&root).to_path_buf();
            while !anc.exists() {
                match anc.parent() {
                    Some(p) if p.starts_with(&root) => anc = p.to_path_buf(),
                    _ => break,
                }
            }
            anc
        } else {
            path.clone()
        };
        let canon = fs::canonicalize(&check)
            .map_err(|e| format!("path {rel:?} cannot be resolved inside the clone: {e}"))?;
        if !canon.starts_with(&root) {
            return Err(format!("path {rel:?} escapes the clone root"));
        }
        Ok(ResolvedPath {
            rel: rel.to_string(),
            path,
        })
    }
}

/// The parent's sinks, as a subagent sees them: every step the child emits
/// reaches them marked with the child's depth.
struct NestedSink {
    sinks: Arc<Mutex<Vec<Box<dyn Sink>>>>,
    depth: u8,
}

impl Sink for NestedSink {
    fn step(&mut self, mut step: Step) {
        step.depth = step.depth.max(self.depth);
        let mut sinks = self.sinks.lock().unwrap_or_else(|e| e.into_inner());
        for sink in sinks.iter_mut() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                sink.step(step.clone());
            }));
        }
    }
}

/// The exit code of a formatted observation (`<returncode>N</returncode>`).
fn observation_exit_code(text: &str) -> Option<i32> {
    text.strip_prefix(RETURN_CODE_PREFIX)?
        .split("</returncode>")
        .next()?
        .trim()
        .parse()
        .ok()
}

fn tool_message(call: &ToolCall, content: &str) -> Message {
    Message {
        role: Role::Tool,
        content: content.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: call.id.clone(),
        name: call.name.clone(),
        ..Default::default()
    }
}

/// What one typed tool call gives back to the loop.
struct ToolOutcome {
    text: String,
    /// False when the call itself was malformed (unknown tool, arguments
    /// of the wrong shape): that counts as a format error.
    clean: bool,
    submitted: bool,
    submission: String,
}

impl ToolOutcome {
    fn clean(text: String) -> Self {
        Self {
            text,
            clean: true,
            submitted: false,
            submission: String::new(),
        }
    }
}

fn create_parent(path: &Path) -> std::io::Result<()> {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() && !p.exists() => fs::create_dir_all(p),
        _ => Ok(()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanItem {
    pub text: String,
    pub status: String,
}

/// The checklist the `plan` tool shows: `[ ]` todo, `[~]` doing, `[x]` done.
pub fn render_plan(items: &[PlanItem]) -> String {
    let done = items.iter().filter(|i| i.status == "done").count();
    let mut out = format!("plan: {done}/{} done\n", items.len());
    for item in items {
        let mark = match item.status.as_str() {
            "done" => "x",
            "doing" => "~",
            _ => " ",
        };
        out.push_str(&format!("- [{mark}] {}\n", one_line(&item.text, 200)));
    }
    out
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Trajectory {
    pub trajectory_format: String,
    pub info: TrajectoryInfo,
    pub messages: Vec<Message>,
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrajectoryInfo {
    pub model_stats: ModelStats,
    pub exit_status: String,
    pub submission: String,
    pub config: TrajectoryConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelStats {
    pub api_calls: i32,
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub cost: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrajectoryConfig {
    pub model: String,
    pub provider: String,
    pub max_steps: i32,
    pub step_timeout_seconds: i64,
    pub cwd: String,
    /// An interactive session (`pulse harness chat`), which `--continue`
    /// picks up again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interactive: bool,
}

pub fn write_trajectory(path: &str, t: &Trajectory) -> anyhow::Result<()> {
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent)?;
        chmod(parent, 0o700)?;
    }
    let mut data = pulse_store::gojson::to_vec_pretty(t)?;
    data.push(b'\n');
    let tmp = format!("{path}.tmp");
    fs::write(&tmp, data)?;
    chmod(Path::new(&tmp), 0o600)?;
    fs::rename(tmp, path)?;
    Ok(())
}

pub use write_trajectory as WriteTrajectory;

pub const DEFAULT_PROMPT_TEMPLATE: &str = concat!(
    "You are an autonomous software engineering agent working in {{.Info.cwd}} on {{.Info.os}}.\n",
    "Your tools: bash runs one non-interactive shell command; read shows a file with line numbers (optionally a line range); grep searches file contents with a regex; glob lists files by pattern; edit replaces exact text in one file or creates it; write creates a file; plan keeps the checklist the owner sees.\n",
    "Some runs also offer skill (loads a repository skill's instructions before work it covers), fetch (reads a web page as text, allowed hosts only), subagent (a bounded helper run on a self-contained sub-task that returns a report), memory (remembers a short fact about this project for later runs: a convention the files do not state, a gotcha, where things live; not secrets, not what the files already say) and mcp__ tools; use them when they are offered.\n",
    "Call tools with JSON arguments. Independent calls go in one reply, each with its own result: read, grep and glob calls of one reply run at the same time, the rest in order; a call that needs an earlier result waits for your next reply.\n",
    "Prefer read, grep, glob and edit over shell for the same job: they cost less and their errors are precise.\n",
    "If you cannot call tools, write exactly one fenced block per message; exactly one fenced block is executed.\n",
    "The working directory is already the clone root ({{.Info.cwd}}), so `cd <root> &&` is unnecessary.\n",
    "Example command reply: ```bash\ncargo test -p crate_name\n```.\n",
    "Example report reply: SUMMARY: done\nFINDING: tests pass.\n",
    "Prose-only replies are not executed.\n",
    "Shell command fences may use ```bash, ```sh, ```shell, ```zsh, ```console, or ```terminal.\n",
    "Native action fences do what the read, grep and edit tools do:\n",
    "```read\nsrc/lib.rs:1-80\n```\n",
    "```grep\nTODO\nsrc\n```\n",
    "```edit\nsrc/lib.rs\n<<<<<<< SEARCH\nold\n=======\nnew\n>>>>>>> REPLACE\n```\n",
    "Each shell command runs in a fresh non-interactive {{.Info.shell}} in the working directory.\n",
    "No state survives between commands: cd, exports, aliases, and shell variables are lost.\n",
    "Background children a command leaves (cmd &, nohup) are killed when the command ends. For a server, a watcher or a check that outlasts a command, call bash with background:true: it returns at once with an id, and the process tool reads its output, waits for it or stops it; stop what you started when you are done.\n",
    "Chain setup and use with &&, or use absolute paths.\n",
    "Commands time out after {{.StepTimeout}}.\n",
    "Keep outputs small with head, tail, grep, and focused test selectors.\n",
    "Output over {{.OutputLimit}} characters is cut in the middle.\n",
    "Older command outputs are shortened to their first and last lines as the conversation grows; rerun a command if you need its full output again.\n",
    "Never run interactive programs: editors, pagers, or anything that waits for a keyboard; a server or a watch mode goes in the background.\n",
    "To edit files, prefer the edit tool (or ```edit blocks); shell heredocs, sed -i, or python3 -c are still available when needed.\n",
    "For work of several steps, keep a plan: call plan with the whole checklist and update it as steps finish.\n",
    "Verify your work by running the project's own checks.\n",
    "On a command failure, inspect the cause, make a small safe correction and rerun the targeted check.\n",
    "If it still fails, try a different evidence-based approach or use the task's peer-help channel; do not repeat an unchanged failure or poll while waiting for help.\n",
    "Never bypass a policy refusal or approval boundary, weaken a check, or report unfinished work as completed.\n",
    "A budget notice arrives when few model calls remain; after it, stop exploring and finish with the targeted check, commit, and final SUMMARY report.\n",
    "An unfinished task is reported as SUMMARY: blocked: <cause and attempted fixes> or SUMMARY: failed: <cause and attempted fixes>.\n",
    "When done, or when you cannot continue, reply with no command and a plain-text final report.\n",
    "The report must include a SUMMARY: line and any FINDING:, SUGGESTION:, QUESTION:, or VOTE: lines the task asked for.\n",
    "Those key words must start their lines because a program reads the report.\n",
    "A message with neither a command nor a report wastes a call after one corrective retry.\n",
    "You have at most {{.MaxSteps}} model calls.\n",
    "{{.Instructions}}",
    "{{.Memory}}",
    "{{.Teach}}",
    "The task follows in the next message and its rules apply in full."
);

pub use DEFAULT_PROMPT_TEMPLATE as DefaultPromptTemplate;

/// The system prompt of an interactive session (`pulse harness chat`): the
/// same tools and shell rules as a job, with the user at the keyboard
/// instead of a task brief and a report format.
pub const CHAT_PROMPT_TEMPLATE: &str = concat!(
    "You are a software engineering agent working with the user in their checkout at {{.Info.cwd}} on {{.Info.os}}, in an interactive session: they write, you work and answer, they write again.\n",
    "Your tools: bash runs one non-interactive shell command; read shows a file with line numbers (optionally a line range); grep searches file contents with a regex; glob lists files by pattern; edit replaces exact text in one file or creates it; write creates a file; plan keeps the checklist the user sees; question asks the user one thing and waits for the answer.\n",
    "Some sessions also offer skill (loads a repository skill's instructions before work it covers), fetch (reads a web page as text, allowed hosts only), subagent (a bounded helper run on a self-contained sub-task that returns a report), memory (remembers a short fact about this project or the user's preferences for later sessions: a convention the files do not state, a gotcha, how they like things done; not secrets, not what the files already say; remember when the user says to, or when you learned something the next session would need) and mcp__ tools; use them when they are offered.\n",
    "Call tools with JSON arguments. Independent calls go in one reply, each with its own result: read, grep and glob calls of one reply run at the same time, the rest in order; a call that needs an earlier result waits for your next reply.\n",
    "Prefer read, grep, glob and edit over shell for the same job: they cost less and their errors are precise.\n",
    "A reply with no tool call ends your turn and is shown to the user; they answer in the next message. Reply in plain prose, short: what you did, what you found, what is next. There is no report format.\n",
    "The user sees every edit, write, and command that changes the checkout or leaves the machine (git commit, git push, gh, package installs, formatters that rewrite files, rm, and the like) before it runs and may decline it; a declined call comes back as `declined by the user` with their reason. Follow the reason; never do the same thing another way.\n",
    "Use question when a decision is the user's: which of several approaches, whether to go further than asked, a detail the request leaves out. Do not guess at scope; do not start work the user did not ask for; do not commit or push unless they ask.\n",
    "The working directory is {{.Info.cwd}}; it may have uncommitted work of the user's in it. Leave what you did not touch as it was, and never discard changes you did not make.\n",
    "Each shell command runs in a fresh non-interactive {{.Info.shell}} in the working directory.\n",
    "No state survives between commands: cd, exports, aliases, and shell variables are lost.\n",
    "Background children a command leaves (cmd &, nohup) are killed when the command ends. For a server, a watcher or a check that outlasts a command, call bash with background:true: it returns at once with an id, and the process tool reads its output, waits for it or stops it; stop what you started when you are done.\n",
    "Chain setup and use with &&, or use absolute paths.\n",
    "Commands time out after {{.StepTimeout}}.\n",
    "Keep outputs small with head, tail, grep, and focused test selectors.\n",
    "Output over {{.OutputLimit}} characters is cut in the middle.\n",
    "Older command outputs are shortened to their first and last lines as the conversation grows; rerun a command if you need its full output again.\n",
    "Never run interactive programs: editors, pagers, or anything that waits for a keyboard; a server or a watch mode goes in the background.\n",
    "To edit files, prefer the edit tool; shell heredocs, sed -i, or python3 -c are still available when needed.\n",
    "For work of several steps, keep a plan: call plan with the whole checklist and update it as steps finish.\n",
    "Verify your work by running the project's own checks.\n",
    "On a command failure, inspect the cause, make a small safe correction and rerun the targeted check; do not repeat an unchanged failure.\n",
    "Never bypass a policy refusal or approval boundary, weaken a check, or report unfinished work as done.\n",
    "Each turn has at most {{.MaxSteps}} model calls; a budget notice arrives when few remain, and then you wrap up and tell the user where things stand.\n",
    "{{.Instructions}}",
    "{{.Memory}}",
    "{{.Teach}}",
    "The user's first message follows."
);

/// Appended to the system prompt of a read-only run, after the template
/// renders, so an owner's own template gets it too.
pub const READ_ONLY_PROMPT: &str = concat!(
    "This is a read-only run: your findings are the whole deliverable. Look, test and report; change nothing in the clone and commit nothing.\n",
    "The edit and write tools are not offered. Commands that would change the clone (git commit, checkout, stash, formatters that rewrite files, `> file`, sed -i, npm install <pkg>, and the like) are refused, and a command that changes it anyway is put back and reported. Commands run to completion only: bash background:true and the process tool are not offered.\n",
    "Scratch files go under /tmp. Running the project's checks is fine. Describe a change you would make as a FINDING: or SUGGESTION: line instead of making it."
);

/// The Note step a read-only run starts with.
pub const READ_ONLY_START_NOTE: &str = "read-only run: edit and write are off, commands that would change the clone are refused, and a change that slips through is put back";

/// The closing clause of every read-only refusal.
const READ_ONLY_REMINDER: &str =
    "Look, test and report; change nothing and commit nothing. Scratch files go under /tmp.";

#[derive(Clone, Debug, Default)]
pub struct PromptData {
    pub cwd: String,
    pub model: String,
    pub provider: String,
    pub step_timeout: String,
    pub output_limit: usize,
    pub max_steps: i32,
    pub info: BTreeMap<String, String>,
    /// Repository instructions as [`load_instructions`] read them; the
    /// template's `{{.Instructions}}` renders them as a paragraph, and a
    /// template without the key gets the paragraph appended.
    pub instructions: String,
    /// What earlier runs remembered ([`Memory::prompt_section`]); the
    /// template's `{{.Memory}}` renders it as a paragraph after the
    /// instructions, and a template without the key gets it appended.
    pub memory: String,
    /// Teacher mode ([`Config::teach`]); the template's `{{.Teach}}` renders
    /// [`teach_paragraph`] there, and a template without the key gets it
    /// appended after the memory.
    pub teach: bool,
}

/// The teacher-mode paragraph of a job's system prompt.
pub const TEACH_PARAGRAPH: &str = concat!(
    "Teacher mode is on: a junior developer reads this run to learn from it, so teach as you work without slowing the work down. ",
    "Whenever you make a design or coding choice that a junior could learn from (a pattern, a data structure, where code lives, a test strategy, a trade-off, a fix and what caused the bug), say in one or two plain sentences what you chose, the alternative you passed over, and why - in the reply that makes the choice, not after. ",
    "In the final report, repeat the lessons worth keeping as LESSON: lines, one lesson per line, each a self-contained sentence or two that names the choice, the alternative and the reason (e.g. LESSON: validated the flag in the store's settings table rather than in the CLI parser, because every caller - CLI, hub and runner - reads settings through the store, so one check covers them all). ",
    "Three to seven lessons is plenty; skip what any developer already knows.\n"
);

/// The teacher-mode paragraph of an interactive session.
pub const TEACH_CHAT_PARAGRAPH: &str = concat!(
    "Teacher mode is on: the user is learning from this session, so teach as you work without slowing the work down. ",
    "Whenever you make a design or coding choice they could learn from (a pattern, a data structure, where code lives, a test strategy, a trade-off, a fix and what caused the bug), say in one or two plain sentences what you chose, the alternative you passed over, and why - in the reply that makes the choice. ",
    "Name the concept so they can look it up, point at the file and line you are talking about, and when you finish a piece of work, close with a short 'what to take away' of the lessons worth keeping. ",
    "Skip what any developer already knows, and never explain instead of doing the work.\n"
);

/// The system-prompt paragraph of teacher mode: empty when it is off.
pub fn teach_paragraph(teach: bool, interactive: bool) -> String {
    if !teach {
        return String::new();
    }
    if interactive {
        TEACH_CHAT_PARAGRAPH.to_string()
    } else {
        TEACH_PARAGRAPH.to_string()
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The system-prompt paragraph that carries repository instructions.
pub fn instructions_paragraph(instructions: &str) -> String {
    let text = instructions.trim();
    if text.is_empty() {
        return String::new();
    }
    format!(
        "Repository instructions follow; they apply in addition to the task, and the rules above still win where they differ.\n{text}\n"
    )
}

pub fn render_prompt(tmpl: &str, data: PromptData) -> anyhow::Result<String> {
    let data = normalize_prompt_data(data);
    let instructions = instructions_paragraph(&data.instructions);
    let memory = memory_paragraph(&data.memory);
    let teach = teach_paragraph(data.teach, tmpl_is_chat(tmpl));
    let has_instructions_key = tmpl.contains("{{.Instructions}}");
    let has_memory_key = tmpl.contains("{{.Memory}}");
    let has_teach_key = tmpl.contains("{{.Teach}}");
    // The instructions and the memory are substituted last and never
    // scanned for keys: a repository's AGENTS.md may well document
    // `{{.Something}}` itself.
    let render_part = |part: &str| -> anyhow::Result<String> {
        let mut out = part.to_string();
        let replacements = [
            ("{{.Cwd}}", data.cwd.as_str()),
            ("{{.Model}}", data.model.as_str()),
            ("{{.Provider}}", data.provider.as_str()),
            ("{{.StepTimeout}}", data.step_timeout.as_str()),
        ];
        for (k, v) in replacements {
            out = out.replace(k, v);
        }
        out = out.replace("{{.OutputLimit}}", &data.output_limit.to_string());
        out = out.replace("{{.MaxSteps}}", &data.max_steps.to_string());
        for (k, v) in &data.info {
            out = out.replace(&format!("{{{{.Info.{k}}}}}"), v);
        }
        if let Some(start) = out.find("{{.") {
            let tail = &out[start..];
            let end = tail.find("}}").map_or(tail.len(), |i| i + 2);
            return Err(anyhow!(
                "template: prompt:1: map has no entry for key {:?}",
                &tail[..end]
            ));
        }
        Ok(out)
    };
    let mut rendered = Vec::new();
    for part in tmpl.split("{{.Instructions}}") {
        let mut pieces = Vec::new();
        for piece in part.split("{{.Memory}}") {
            let mut bits = Vec::new();
            for bit in piece.split("{{.Teach}}") {
                bits.push(render_part(bit)?);
            }
            pieces.push(bits.join(&teach));
        }
        rendered.push(pieces.join(&memory));
    }
    let mut out = rendered.join(&instructions);
    for (has_key, paragraph) in [
        (has_instructions_key, &instructions),
        (has_memory_key, &memory),
        (has_teach_key, &teach),
    ] {
        if !has_key && !paragraph.is_empty() {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(paragraph.trim_end());
        }
    }
    Ok(out)
}

pub use render_prompt as RenderPrompt;

/// Whether a template is the interactive session's: the teacher-mode
/// paragraph speaks of "the user" there and of the final report in a job's.
fn tmpl_is_chat(tmpl: &str) -> bool {
    tmpl == CHAT_PROMPT_TEMPLATE || tmpl.contains("in an interactive session")
}

pub fn normalize_prompt_data(mut data: PromptData) -> PromptData {
    let mut cp = data.info.clone();
    if cp.get("cwd").is_none_or(String::is_empty) {
        cp.insert("cwd".into(), data.cwd.clone());
    }
    if cp.get("cwd").is_none_or(String::is_empty) {
        cp.insert("cwd".into(), ".".into());
    }
    if data.cwd.is_empty() {
        data.cwd = cp.get("cwd").cloned().unwrap_or_else(|| ".".into());
    }
    if cp.get("os").is_none_or(String::is_empty) {
        cp.insert("os".into(), "this system".into());
    }
    if cp.get("shell").is_none_or(String::is_empty) {
        cp.insert("shell".into(), "/bin/sh".into());
    }
    data.info = cp;
    data
}

pub fn prompt_data(info: &BTreeMap<String, String>, cfg: &Config) -> PromptData {
    let cfg = apply_defaults(cfg.clone());
    let mut cp = info.clone();
    let mut cwd = cp.get("cwd").cloned().unwrap_or_default();
    if cwd.is_empty() {
        cwd = cfg.cwd.clone();
    }
    if cwd.is_empty() {
        cwd = ".".to_string();
    }
    cp.insert("cwd".into(), cwd.clone());
    if cp.get("os").is_none_or(String::is_empty) {
        cp.insert("os".into(), "this system".into());
    }
    if cp.get("shell").is_none_or(String::is_empty) {
        cp.insert("shell".into(), "/bin/sh".into());
    }
    normalize_prompt_data(PromptData {
        cwd,
        model: cfg.model,
        provider: cfg.provider,
        step_timeout: pulse_cli::format_duration(cfg.step_timeout),
        output_limit: cfg.output_limit,
        max_steps: cfg.max_steps,
        info: cp,
        instructions: cfg.instructions,
        memory: cfg.memory,
        teach: cfg.teach,
    })
}

pub fn default_system_prompt(info: &BTreeMap<String, String>, cfg: Config) -> String {
    render_prompt(DEFAULT_PROMPT_TEMPLATE, prompt_data(info, &cfg)).unwrap_or_default()
}

pub use default_system_prompt as DefaultSystemPrompt;

pub fn apply_defaults(mut cfg: Config) -> Config {
    if cfg.max_steps == 0 {
        cfg.max_steps = DEFAULT_MAX_STEPS;
    }
    if cfg.step_timeout.is_zero() {
        cfg.step_timeout = DEFAULT_STEP_TIMEOUT;
    }
    if cfg.commit_timeout.is_zero() {
        cfg.commit_timeout = DEFAULT_COMMIT_TIMEOUT;
    }
    if cfg.output_limit == 0 {
        cfg.output_limit = DEFAULT_OUTPUT_LIMIT;
    }
    if cfg.keep_observations == 0 {
        cfg.keep_observations = DEFAULT_KEEP_OBSERVATIONS;
    }
    if cfg.max_format_errors == 0 {
        cfg.max_format_errors = DEFAULT_MAX_FORMAT_ERRORS;
    }
    if cfg.format_retries < 0 {
        cfg.format_retries = 0;
    }
    cfg
}

fn parse_command(args: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Args {
        command: String,
    }
    let obj: Args = serde_json::from_str(args).ok()?;
    if obj.command.is_empty() {
        None
    } else {
        Some(obj.command)
    }
}

/// The `description` of a `bash` call - one sentence on what the command
/// does, kept to a line; empty when the model gave none.
fn parse_title(args: &str) -> String {
    let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
    match v.get("description").and_then(Value::as_str) {
        Some(d) if !d.trim().is_empty() => one_line(d, TITLE_LIMIT),
        _ => String::new(),
    }
}

/// The `background`/`wait` of a `bash` call: Some(initial wait) when the
/// command is to run in the background.
fn parse_background(args: &str) -> std::result::Result<Option<Duration>, String> {
    let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
    let background = match v.get("background") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.trim(), "true" | "yes" | "1"),
        Some(_) => return Err("background must be true or false".to_string()),
    };
    if !background {
        return Ok(None);
    }
    let wait = match v.get("wait") {
        None | Some(Value::Null) => BACKGROUND_WAIT_DEFAULT,
        Some(w) => match w.as_f64() {
            Some(secs) if secs >= 0.0 => Duration::from_secs_f64(secs),
            _ => return Err("wait must be a non-negative number of seconds".to_string()),
        },
    };
    Ok(Some(wait.min(BACKGROUND_WAIT_LIMIT)))
}

/// `a` and `b` on separate lines, skipping an empty one.
fn join_lines(a: &str, b: &str) -> String {
    let a = a.trim_end_matches('\n');
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b.to_string(),
        (_, true) => a.to_string(),
        _ => format!("{a}\n{b}"),
    }
}

pub fn bash_blocks(text: &str) -> Vec<String> {
    action_blocks(text)
        .into_iter()
        .filter_map(|b| match b {
            ActionBlock::Command(command) => Some(command),
            _ => None,
        })
        .collect()
}

pub fn is_report(text: &str) -> bool {
    static RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(
            r"(?im)^\s*(?:#{1,6}\s*)?-?\s*`?(SUMMARY|QUESTION|FINDING|SUGGESTION|VOTE|DECIDE)`?:",
        )
        .unwrap()
    });
    static HEADING_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?im)^\s*#{1,6}\s*(?:final\s+)?report\s*$").unwrap()
    });
    text.contains("COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT")
        || RE.is_match(text)
        || HEADING_RE.is_match(text)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ActionBlock {
    Command(String),
    Read(String),
    Grep(String),
    Edit(String),
}

#[derive(Clone, Copy, Debug)]
enum NativeAction {
    Read,
    Grep,
    Edit,
}

impl NativeAction {
    fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Grep => "grep",
            Self::Edit => "edit",
        }
    }

    fn step_kind(self) -> StepKind {
        match self {
            Self::Read => StepKind::Read,
            Self::Grep => StepKind::Grep,
            Self::Edit => StepKind::Edit,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReadTarget {
    path: String,
    range: Option<(usize, usize)>,
}

/// One native tool call with its arguments parsed, whether it arrived as a
/// typed tool call or as a fenced block.
#[derive(Clone, Debug, PartialEq, Eq)]
enum NativeCall {
    Read(Vec<ReadTarget>),
    Grep {
        pattern: String,
        scope: String,
    },
    Glob {
        pattern: String,
        scope: String,
    },
    Edit {
        path: String,
        blocks: Vec<EditBlock>,
    },
    Write {
        path: String,
        content: String,
        overwrite: bool,
    },
    Plan(Vec<PlanItem>),
}

impl NativeCall {
    fn name(&self) -> &'static str {
        match self {
            Self::Read(_) => "read",
            Self::Grep { .. } => "grep",
            Self::Glob { .. } => "glob",
            Self::Edit { .. } => "edit",
            Self::Write { .. } => "write",
            Self::Plan(_) => "plan",
        }
    }

    fn step_kind(&self) -> StepKind {
        match self {
            Self::Read(_) => StepKind::Read,
            Self::Grep { .. } => StepKind::Grep,
            Self::Glob { .. } => StepKind::Glob,
            Self::Edit { .. } => StepKind::Edit,
            Self::Write { .. } => StepKind::Write,
            Self::Plan(_) => StepKind::Plan,
        }
    }

    /// Whether the call changes the clone.
    fn writes(&self) -> bool {
        matches!(self, Self::Edit { .. } | Self::Write { .. })
    }

    /// The one path an `edit` or `write` changes.
    fn path(&self) -> Option<&str> {
        match self {
            Self::Edit { path, .. } | Self::Write { path, .. } => Some(path.trim()),
            _ => None,
        }
    }

    /// The files the call reads or changes (none for a search or a plan).
    fn files(&self) -> Vec<&str> {
        match self {
            Self::Read(targets) => targets.iter().map(|t| t.path.trim()).collect(),
            Self::Edit { path, .. } | Self::Write { path, .. } => vec![path.trim()],
            _ => Vec::new(),
        }
    }

    /// What a permission rule's filter is matched against: the paths read
    /// or changed, the directory searched (`.` for the whole checkout).
    fn subjects(&self) -> Vec<String> {
        match self {
            Self::Read(targets) => targets.iter().map(|t| t.path.clone()).collect(),
            Self::Grep { scope, .. } | Self::Glob { scope, .. } => {
                vec![if scope.trim().is_empty() {
                    ".".to_string()
                } else {
                    scope.trim().to_string()
                }]
            }
            Self::Edit { path, .. } | Self::Write { path, .. } => vec![path.clone()],
            Self::Plan(_) => Vec::new(),
        }
    }

    /// Whether the call can run beside the other calls of its reply: it
    /// only reads, and reads nothing another call of the reply writes.
    fn parallel_safe(&self) -> bool {
        matches!(self, Self::Read(_) | Self::Grep { .. } | Self::Glob { .. })
    }

    /// What the user sees before an edit or a write runs: the path, a
    /// preview (the blocks as `-`/`+` lines, the head of a new file) and
    /// why it is shown. None for a call that changes nothing.
    fn approval_preview(&self, cwd: &str) -> Option<(String, String, String)> {
        const PREVIEW_LINES: usize = 40;
        match self {
            Self::Edit { path, blocks } => {
                let mut lines: Vec<String> = Vec::new();
                for (i, b) in blocks.iter().enumerate() {
                    if blocks.len() > 1 {
                        lines.push(format!("block {} of {}", i + 1, blocks.len()));
                    }
                    lines.extend(b.search.lines().map(|l| format!("- {l}")));
                    lines.extend(b.replace.lines().map(|l| format!("+ {l}")));
                }
                let cut = lines.len().saturating_sub(PREVIEW_LINES);
                lines.truncate(PREVIEW_LINES);
                if cut > 0 {
                    lines.push(format!("… {cut} more lines"));
                }
                Some((
                    path.clone(),
                    lines.join("\n"),
                    format!(
                        "edits {} ({} block{})",
                        path,
                        blocks.len(),
                        if blocks.len() == 1 { "" } else { "s" }
                    ),
                ))
            }
            Self::Write {
                path,
                content,
                overwrite,
            } => {
                let exists = Path::new(cwd).join(path).exists();
                let mut lines: Vec<String> = content
                    .lines()
                    .take(PREVIEW_LINES)
                    .map(str::to_string)
                    .collect();
                let total = content.lines().count();
                if total > PREVIEW_LINES {
                    lines.push(format!("… {} more lines", total - PREVIEW_LINES));
                }
                let reason = if exists && *overwrite {
                    format!("replaces the existing {path} ({} bytes)", content.len())
                } else if exists {
                    format!("writes {path}, which exists ({} bytes)", content.len())
                } else {
                    format!("creates {path} ({} bytes)", content.len())
                };
                Some((path.clone(), lines.join("\n"), reason))
            }
            _ => None,
        }
    }

    /// The call as a fenced-block spec: what the step records and the
    /// console shows, the same for both ways of calling.
    fn spec(&self) -> String {
        match self {
            Self::Read(targets) => targets
                .iter()
                .map(|t| match t.range {
                    Some((a, b)) => format!("{}:{a}-{b}", t.path),
                    None => t.path.clone(),
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Self::Grep { pattern, scope } | Self::Glob { pattern, scope } => {
                if scope.is_empty() || scope == "." {
                    pattern.clone()
                } else {
                    format!("{pattern}\n{scope}")
                }
            }
            Self::Edit { path, blocks } => {
                let mut out = path.clone();
                for b in blocks {
                    out.push_str(&format!(
                        "\n<<<<<<< SEARCH\n{}\n=======\n{}\n>>>>>>> REPLACE",
                        b.search, b.replace
                    ));
                }
                out
            }
            Self::Write { path, content, .. } => {
                format!("{path} ({} bytes)", content.len())
            }
            Self::Plan(items) => render_plan(items),
        }
    }

    fn from_fenced(action: NativeAction, spec: &str) -> std::result::Result<Self, String> {
        match action {
            NativeAction::Read => Ok(Self::Read(
                spec.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(|raw| {
                        let (path, range) = parse_read_target(raw);
                        ReadTarget {
                            path: path.to_string(),
                            range,
                        }
                    })
                    .collect(),
            )),
            NativeAction::Grep => {
                let mut lines = spec.lines();
                let pattern = lines.next().unwrap_or("").trim().to_string();
                let scope = lines.next().map(str::trim).unwrap_or("").to_string();
                Ok(Self::Grep { pattern, scope })
            }
            NativeAction::Edit => {
                let Some((path, rest)) = spec.split_once('\n') else {
                    return Err("edit: missing path and replacement block".into());
                };
                Ok(Self::Edit {
                    path: path.trim().to_string(),
                    blocks: parse_edit_blocks(rest)?,
                })
            }
        }
    }

    /// Parses a typed tool call's JSON arguments.
    fn from_args(name: &str, arguments: &str) -> std::result::Result<Self, String> {
        let args: Value = if arguments.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(arguments).map_err(|e| format!("arguments are not JSON: {e}"))?
        };
        if !args.is_object() {
            return Err("arguments must be a JSON object".into());
        }
        let str_arg = |key: &str| -> std::result::Result<String, String> {
            match args.get(key) {
                Some(Value::String(s)) => Ok(s.clone()),
                Some(Value::Null) | None => Err(format!("missing string argument {key}")),
                Some(other) => Err(format!(
                    "argument {key} must be a string, not {}",
                    json_kind(other)
                )),
            }
        };
        let opt_str = |key: &str| -> std::result::Result<String, String> {
            match args.get(key) {
                Some(Value::String(s)) => Ok(s.clone()),
                Some(Value::Null) | None => Ok(String::new()),
                Some(other) => Err(format!(
                    "argument {key} must be a string, not {}",
                    json_kind(other)
                )),
            }
        };
        let opt_line = |key: &str| -> std::result::Result<Option<usize>, String> {
            match args.get(key) {
                Some(Value::Null) | None => Ok(None),
                Some(Value::Number(n)) => n
                    .as_u64()
                    .map(|n| Some(n as usize))
                    .ok_or_else(|| format!("argument {key} must be a positive whole number")),
                Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
                Some(Value::String(s)) => s
                    .trim()
                    .parse::<usize>()
                    .map(Some)
                    .map_err(|_| format!("argument {key} must be a positive whole number")),
                Some(other) => Err(format!(
                    "argument {key} must be a whole number, not {}",
                    json_kind(other)
                )),
            }
        };
        match name {
            "read" => {
                let path = str_arg("path")?;
                let range = match (opt_line("start")?, opt_line("end")?) {
                    (None, None) => None,
                    (start, end) => {
                        let start = start.unwrap_or(1).max(1);
                        let end = end.unwrap_or(usize::MAX);
                        if end < start {
                            return Err(format!("end ({end}) is before start ({start})"));
                        }
                        Some((start, end))
                    }
                };
                Ok(Self::Read(vec![ReadTarget { path, range }]))
            }
            "grep" => Ok(Self::Grep {
                pattern: str_arg("pattern")?,
                scope: opt_str("path")?,
            }),
            "glob" => Ok(Self::Glob {
                pattern: str_arg("pattern")?,
                scope: opt_str("path")?,
            }),
            "edit" => {
                let path = str_arg("path")?;
                let mut blocks = Vec::new();
                if let Some(edits) = args.get("edits").filter(|v| !v.is_null()) {
                    let Some(list) = edits.as_array() else {
                        return Err("argument edits must be an array of {search, replace}".into());
                    };
                    for (i, e) in list.iter().enumerate() {
                        let search = e.get("search").and_then(Value::as_str);
                        let replace = e.get("replace").and_then(Value::as_str);
                        match (search, replace) {
                            (Some(s), Some(r)) => blocks.push(EditBlock {
                                search: s.to_string(),
                                replace: r.to_string(),
                            }),
                            _ => {
                                return Err(format!("edits[{i}] needs string search and replace"));
                            }
                        }
                    }
                }
                if blocks.is_empty() {
                    let search = opt_str("search")?;
                    let replace = match args.get("replace") {
                        Some(Value::String(s)) => s.clone(),
                        Some(Value::Null) | None => {
                            return Err("missing string argument replace".into());
                        }
                        Some(other) => {
                            return Err(format!(
                                "argument replace must be a string, not {}",
                                json_kind(other)
                            ));
                        }
                    };
                    blocks.push(EditBlock { search, replace });
                }
                Ok(Self::Edit { path, blocks })
            }
            "write" => {
                let path = str_arg("path")?;
                let content = str_arg("content")?;
                let overwrite = match args.get("overwrite") {
                    Some(Value::Bool(b)) => *b,
                    Some(Value::String(s)) => {
                        matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "yes" | "1")
                    }
                    Some(Value::Null) | None => false,
                    Some(other) => {
                        return Err(format!(
                            "argument overwrite must be a boolean, not {}",
                            json_kind(other)
                        ));
                    }
                };
                Ok(Self::Write {
                    path,
                    content,
                    overwrite,
                })
            }
            "plan" => {
                let Some(list) = args.get("items").and_then(Value::as_array) else {
                    return Err("missing array argument items".into());
                };
                let mut items = Vec::new();
                for (i, item) in list.iter().enumerate() {
                    let text = match item {
                        Value::String(s) => s.clone(),
                        Value::Object(_) => item
                            .get("text")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .ok_or_else(|| format!("items[{i}] needs a string text"))?,
                        _ => return Err(format!("items[{i}] must be an object with text")),
                    };
                    let status = item
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("todo")
                        .trim()
                        .to_ascii_lowercase();
                    let status = match status.as_str() {
                        "done" | "completed" | "complete" | "x" => "done",
                        "doing" | "in_progress" | "in-progress" | "active" | "started" | "wip"
                        | "~" => "doing",
                        "todo" | "pending" | "" | "open" => "todo",
                        other => {
                            return Err(format!(
                                "items[{i}] status {other:?} is not one of todo, doing, done"
                            ));
                        }
                    };
                    items.push(PlanItem {
                        text,
                        status: status.to_string(),
                    });
                }
                if items.is_empty() {
                    return Err("items is empty; list the steps".into());
                }
                Ok(Self::Plan(items))
            }
            other => Err(format!("{other} is not a native tool")),
        }
    }
}

fn json_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[derive(Clone, Debug)]
struct ResolvedPath {
    rel: String,
    path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EditBlock {
    search: String,
    replace: String,
}

fn action_blocks(text: &str) -> Vec<ActionBlock> {
    let mut out = Vec::new();
    out.extend(fenced_action_blocks(text));
    if out.is_empty()
        && let Some(command) = inline_code_command(text)
    {
        out.push(ActionBlock::Command(command));
    }
    out
}

fn fenced_action_blocks(text: &str) -> Vec<ActionBlock> {
    static RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?s)(```|~~~)[ \t]*([A-Za-z0-9_-]*)[^\n`~]*\n(.*?)\n(?:```|~~~)").unwrap()
    });
    RE.captures_iter(text)
        .filter_map(|c| {
            let info = c.get(2).map_or("", |m| m.as_str()).to_ascii_lowercase();
            let body = c.get(3).map_or("", |m| m.as_str()).trim().to_string();
            match info.as_str() {
                "bash" | "sh" | "shell" | "zsh" | "console" | "terminal" => {
                    Some(ActionBlock::Command(strip_prompt_prefix(&body)))
                }
                "read" => Some(ActionBlock::Read(body)),
                "grep" => Some(ActionBlock::Grep(body)),
                "edit" => Some(ActionBlock::Edit(body)),
                _ => None,
            }
        })
        .collect()
}

fn strip_prompt_prefix(command: &str) -> String {
    let mut lines: Vec<&str> = command.lines().collect();
    if let Some(first) = lines.first_mut() {
        let trimmed = first.trim_start();
        if let Some(rest) = trimmed.strip_prefix("$ ") {
            *first = rest;
        }
    }
    lines.join("\n").trim().to_string()
}

fn inline_code_command(text: &str) -> Option<String> {
    static RE: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"`([^`\n]+)`").unwrap());
    let mut matches = RE.captures_iter(text);
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    let whole = first.get(0)?;
    let outside = format!("{}{}", &text[..whole.start()], &text[whole.end()..]);
    if !outside.trim().is_empty() {
        return None;
    }
    let command = first.get(1)?.as_str().trim();
    (!command.is_empty()).then(|| command.to_string())
}

fn parse_read_target(raw: &str) -> (&str, Option<(usize, usize)>) {
    let Some((path, range)) = raw.rsplit_once(':') else {
        return (raw, None);
    };
    let Some((start, end)) = range.split_once('-') else {
        return (raw, None);
    };
    let Ok(start) = start.parse::<usize>() else {
        return (raw, None);
    };
    let Ok(end) = end.parse::<usize>() else {
        return (raw, None);
    };
    if start == 0 || end < start {
        return (raw, None);
    }
    (path, Some((start, end)))
}

fn parse_edit_blocks(text: &str) -> std::result::Result<Vec<EditBlock>, String> {
    let mut blocks = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<<<<<<< SEARCH") {
        rest = &rest[start + "<<<<<<< SEARCH".len()..];
        if let Some(stripped) = rest.strip_prefix('\n') {
            rest = stripped;
        }
        let Some((mid, mid_len)) = find_edit_marker(rest, "=======") else {
            return Err("edit: missing ======= marker".into());
        };
        let search = &rest[..mid];
        rest = &rest[mid + mid_len..];
        if let Some(stripped) = rest.strip_prefix('\n') {
            rest = stripped;
        }
        let Some((end, end_len)) = find_edit_marker(rest, ">>>>>>> REPLACE") else {
            return Err("edit: missing >>>>>>> REPLACE marker".into());
        };
        let replace = &rest[..end];
        blocks.push(EditBlock {
            search: search.to_string(),
            replace: replace.to_string(),
        });
        rest = &rest[end + end_len..];
    }

    fn find_edit_marker(text: &str, marker: &str) -> Option<(usize, usize)> {
        if text.starts_with(marker) {
            return Some((0, marker.len()));
        }
        text.find(&format!("\n{marker}"))
            .map(|idx| (idx, marker.len() + 1))
    }
    if blocks.is_empty() {
        Err("edit: missing SEARCH/REPLACE block".into())
    } else {
        Ok(blocks)
    }
}

fn nearest_line(text: &str, needle: &str) -> usize {
    let first = needle.lines().next().unwrap_or("").trim();
    if first.is_empty() {
        return 1;
    }
    text.lines()
        .enumerate()
        .find(|(_, line)| line.trim().contains(first))
        .map_or(1, |(idx, _)| idx + 1)
}

fn action_error(output: String, start: Instant, exit_code: i32) -> Observation {
    Observation {
        output,
        exit_code,
        duration: start.elapsed(),
        ..Observation::default()
    }
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // The repository's own object store is never what a search wants.
        if path.file_name().is_some_and(|n| n == ".git") {
            continue;
        }
        let Ok(canon) = fs::canonicalize(&path) else {
            continue;
        };
        if !canon.starts_with(root) {
            continue;
        }
        if canon.is_dir() {
            collect_files(root, &canon, out);
        } else if canon.is_file() {
            out.push(canon);
        }
    }
}

/// What a permission rule's filter is matched against for a call of
/// `tool` with these arguments: the command for `bash`, the paths for the
/// file tools (relative to `cwd`), the host for `fetch`, the arguments for
/// an MCP tool, the name for `skill`; nothing for the rest.
fn subjects_of(tool: &str, arguments: &str, cwd: &str) -> Vec<String> {
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let str_of = |key: &str| -> String {
        args.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    match tool {
        "bash" => vec![str_of("command")],
        name if NATIVE_TOOL_NAMES.contains(&name) => {
            let spec = str_of("spec");
            let call = if spec.is_empty() {
                NativeCall::from_args(name, arguments)
            } else {
                let action = match name {
                    "read" => NativeAction::Read,
                    "grep" => NativeAction::Grep,
                    _ => NativeAction::Edit,
                };
                NativeCall::from_fenced(action, &spec)
            };
            call.map(|c| {
                c.subjects()
                    .into_iter()
                    .map(|p| relative_subject(&p, cwd))
                    .collect()
            })
            .unwrap_or_default()
        }
        FETCH_TOOL => vec![url_host(&str_of("url"))],
        WEB_SEARCH_TOOL => vec![str_of("query")],
        SKILL_TOOL => vec![str_of("name")],
        MEMORY_TOOL => vec![
            MemoryAction::parse(arguments)
                .map(|a| a.subject())
                .unwrap_or_default(),
        ],
        name if name.starts_with(MCP_TOOL_PREFIX) => {
            vec![if args.is_object() {
                compact_json(&args, 2000)
            } else {
                String::new()
            }]
        }
        _ => Vec::new(),
    }
}

/// A path as a rule sees it: relative to the working directory when it
/// lies inside it, as given otherwise.
fn relative_subject(path: &str, cwd: &str) -> String {
    let p = Path::new(path);
    if p.is_absolute()
        && !cwd.is_empty()
        && let Ok(rel) = p.strip_prefix(cwd)
    {
        let rel = rel.to_string_lossy();
        return if rel.is_empty() {
            ".".to_string()
        } else {
            rel.into_owned()
        };
    }
    path.to_string()
}

fn glob_match(pattern: &str, text: &str) -> bool {
    fn inner(p: &[u8], t: &[u8]) -> bool {
        if p.is_empty() {
            return t.is_empty();
        }
        if p[0] == b'*' {
            return inner(&p[1..], t) || (!t.is_empty() && inner(p, &t[1..]));
        }
        !t.is_empty() && (p[0] == t[0]) && inner(&p[1..], &t[1..])
    }
    inner(pattern.as_bytes(), text.as_bytes())
}

pub fn deny_description(pattern: &str) -> String {
    match pattern {
        p if p == DEFAULT_DENY[0] => "git push is disabled".into(),
        p if p == DEFAULT_DENY[1] => "git remote add/set-url/remove is disabled".into(),
        p if p == DEFAULT_DENY[2] => "gh commands are disabled".into(),
        p if p == DEFAULT_DENY[3] => "credential and private paths are disabled".into(),
        _ => pattern.to_string(),
    }
}

/// What a commit step's pre-commit hook will run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VerifyGate {
    /// The verify command the hook runs, when it can be read from the hook.
    pub verify_cmd: String,
}

/// True when the command contains a git subcommand that fires the
/// pre-commit hook (`commit`, `merge`, `rebase`, `cherry-pick`, `am`).
pub fn runs_git_commit(command: &str) -> bool {
    static RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?:^|[\s;&|(])git\s+(?:-[-\w=./]+\s+|-C\s+\S+\s+)*(?:commit|merge|rebase|cherry-pick|am)(?:\s|$)")
            .expect("git commit regex")
    });
    RE.is_match(command)
}

/// Reads `<clone>/.git/hooks/pre-commit` (following a `.git` file for
/// worktrees) and returns the verify gate it carries, if any.
pub fn verify_gate_in(clone: &std::path::Path) -> Option<VerifyGate> {
    let dot_git = clone.join(".git");
    let git_dir = if dot_git.is_file() {
        let s = std::fs::read_to_string(&dot_git).ok()?;
        let rel = s.trim().strip_prefix("gitdir:")?.trim();
        let p = std::path::PathBuf::from(rel);
        let p = if p.is_absolute() { p } else { clone.join(p) };
        // A worktree's gitdir is `<main>/.git/worktrees/<name>`; hooks live
        // in the common dir `<main>/.git`.
        match p.parent().and_then(|w| w.parent()) {
            Some(common) if p.parent().is_some_and(|w| w.ends_with("worktrees")) => {
                common.to_path_buf()
            }
            _ => p,
        }
    } else {
        dot_git
    };
    let hook = std::fs::read_to_string(git_dir.join("hooks").join("pre-commit")).ok()?;
    if !hook.contains(VERIFY_GATE_HOOK_MARKER) {
        return None;
    }
    Some(VerifyGate {
        verify_cmd: verify_cmd_from_hook(&hook),
    })
}

/// The verify command line of the runner's hook: the last non-empty,
/// non-comment line before the `)` that closes the hook's subshell.
fn verify_cmd_from_hook(hook: &str) -> String {
    let mut last = "";
    for line in hook.lines() {
        let t = line.trim();
        if t == ")" {
            break;
        }
        if !t.is_empty() && !t.starts_with('#') && t != "(" {
            last = t;
        }
    }
    one_line(last, 160)
}

/// Observation text for a gated commit that hit its deadline: names the
/// verify gate and the lock wait instead of the generic "smaller scope" hint.
pub fn format_commit_timeout(obs: &Observation, limit: usize, gate: &VerifyGate) -> String {
    let mut out = truncate(&obs.output, limit);
    let secs = (obs.duration.as_secs_f64().round() as i64).max(1);
    let lock_wait = lock_wait_from_output(&obs.output);
    let which = if gate.verify_cmd.is_empty() {
        String::new()
    } else {
        format!(" ({})", gate.verify_cmd)
    };
    let line = format!(
        "The command timed out after {secs}s and was killed: the pre-commit verify gate{which} was still running after {secs}s (lock wait {lock_wait}s). Verify was in progress, not stuck; do not background, nohup or sleep around git commit (the harness kills that too). Check whether the commit landed with `git log -1`, and if not run the verify command once in the foreground to see what it reports, then commit again."
    );
    if out.is_empty() {
        out = line;
    } else {
        out.push('\n');
        out.push_str(&line);
    }
    format!("exit code: {}\n{}", obs.exit_code, out)
}

/// Seconds the hook spent waiting for the verify lock, read from its
/// `pulse: another verification holds ...` / `waited Ns` lines; 0 when
/// nothing was printed.
fn lock_wait_from_output(output: &str) -> u64 {
    static RE: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"after waiting (\d+)s").expect("lock wait regex"));
    let mut best = 0;
    for c in RE.captures_iter(output) {
        if let Ok(n) = c[1].parse::<u64>() {
            best = best.max(n);
        }
    }
    best
}

/// The observation of a command the run's stop cut short - the runner's
/// hold or a cancel, not the command's own timeout - so a model that
/// carries the run on later does not go looking for a smaller scope.
pub fn format_interrupted_observation(obs: &Observation, limit: usize, holding: bool) -> String {
    let mut out = truncate(&obs.output, limit);
    let secs = (obs.duration.as_secs_f64()).round().max(1.0) as i64;
    let line = if holding {
        format!(
            "The command was stopped after {secs}s: the runner asked this run to hold for a restart. It did not finish; run it again when you carry on."
        )
    } else {
        format!("The command was stopped after {secs}s because the run was cancelled.")
    };
    if out.is_empty() {
        out = line;
    } else {
        out.push('\n');
        out.push_str(&line);
    }
    format!(
        "<returncode>{}</returncode>\n<output>\n{}\n</output>",
        obs.exit_code, out
    )
}

pub fn format_observation(obs: &Observation, limit: usize) -> String {
    let mut out = truncate(&obs.output, limit);
    if obs.timed_out {
        let mut secs = (obs.duration.as_secs_f64()).round() as i64;
        if secs <= 0 {
            secs = 1;
        }
        let line = format!(
            "The command timed out after {secs}s and was killed; inspect partial output and retry only with a smaller scope or a different safe approach, within the run's remaining budget. Do not leave unsupervised background work."
        );
        if out.is_empty() {
            out = line;
        } else {
            out.push('\n');
            out.push_str(&line);
        }
    }
    format!(
        "<returncode>{}</returncode>\n<output>\n{}\n</output>",
        obs.exit_code, out
    )
}

/// The observation text kept in the step store: capped at the same
/// `output_limit` the model saw, with a one-line marker naming what was
/// dropped. One stray `grep -rn` used to leave a 24 MB row nobody read.
pub fn stored_observation(output: &str, limit: usize) -> String {
    if limit == 0 || output.len() <= limit {
        return output.to_string();
    }
    let kept = truncate(output, limit);
    format!(
        "[stored: {} bytes, kept first/last {} bytes]\n{kept}",
        output.len(),
        limit / 2
    )
}

pub fn truncate(s: &str, limit: usize) -> String {
    if limit == 0 || s.len() <= limit {
        return s.to_string();
    }
    let half = limit / 2;
    // Byte offsets clamped to char boundaries: tool output with multi-byte
    // text (box drawing, Arabic, emoji) at the cut used to panic the worker.
    let head_end = s.floor_char_boundary(half);
    let tail_start = s.ceil_char_boundary(s.len() - half);
    let omitted = tail_start - head_end;
    format!(
        "{}\n[... {omitted} characters omitted ...]\n{}",
        &s[..head_end],
        &s[tail_start..]
    )
}

pub fn submission_from_observation(output: &str) -> Option<String> {
    let output = output.trim_start_matches(['\r', '\n']);
    let (line, rest) = output.split_once('\n').unwrap_or((output, ""));
    if line.trim_end_matches('\r') != "COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT" {
        return None;
    }
    Some(rest.trim_start_matches(['\r', '\n']).to_string())
}

pub fn clone_messages(input: &[Message]) -> Vec<Message> {
    input.to_vec()
}

/// A carried-over conversation that a hold or a crash cut in the middle
/// of a tool round: an assistant message whose calls have no results
/// after it (and whatever follows) goes, so every provider accepts what
/// is left. Complete conversations come back unchanged.
pub(crate) fn trim_dangling_tool_calls(mut msgs: Vec<Message>) -> Vec<Message> {
    loop {
        let Some(i) = msgs
            .iter()
            .rposition(|m| matches!(m.role, Role::Assistant) && !m.tool_calls.is_empty())
        else {
            return msgs;
        };
        let answered = |call: &ToolCall| {
            msgs[i + 1..]
                .iter()
                .any(|m| matches!(m.role, Role::Tool) && m.tool_call_id == call.id)
        };
        if msgs[i].tool_calls.iter().all(answered) {
            return msgs;
        }
        msgs.truncate(i);
    }
}

pub fn one_line(s: &str, limit: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.len() > limit {
        s[..s.floor_char_boundary(limit)].to_string()
    } else {
        s
    }
}

pub(crate) fn now_rfc3339() -> String {
    if let Ok(s) = std::env::var("PULSE_NOW")
        && !s.is_empty()
    {
        return s;
    }
    chrono::DateTime::<chrono::Utc>::from(SystemTime::now())
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn trim_float(v: f64) -> String {
    let s = format!("{v}");
    s
}

/// Whether a provider error says the streamed reply broke part way
/// (`stream_interrupted`): the text shown so far is incomplete and the
/// same request can be made again.
fn is_stream_interrupted(err: &anyhow::Error) -> bool {
    pulse_provider::provider_error(err).is_some_and(|e| e.code == "stream_interrupted")
}

#[cfg(unix)]
fn chmod(path: &Path, mode: u32) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn chmod(_path: &Path, _mode: u32) -> anyhow::Result<()> {
    Ok(())
}

pub const PRUNE_MARKER: &str = "characters elided - rerun the command to see them";
pub const OUTPUT_ELIDED_LINE: &str = "[output elided - rerun the command to see it]";
pub const OUTPUT_OPEN_TAG: &str = "<output>";
pub const OUTPUT_CLOSE_TAG: &str = "</output>";
pub const RETURN_CODE_PREFIX: &str = "<returncode>";

pub fn prune_history(msgs: &[Message], keep: usize, level: i32) -> (Vec<Message>, usize) {
    let mut out = clone_messages(msgs);
    let keep = keep.max(1);
    if level < 1 {
        return (out, 0);
    }
    let observations: Vec<usize> = out
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            if i >= 2 && is_observation_message(i, m) {
                Some(i)
            } else {
                None
            }
        })
        .collect();
    let Some(collapse_through) = observations.len().checked_sub(keep) else {
        return (out, 0);
    };
    if collapse_through == 0 {
        return (out, 0);
    }
    let mut changed = 0;
    for idx in observations.into_iter().take(collapse_through) {
        let old = out[idx].content.clone();
        let next = collapse_observation(&old, level);
        if next != old {
            out[idx].content = next;
            changed += 1;
        }
    }
    (out, changed)
}

/// A user message that begins a turn: not an observation, a compaction
/// summary or a note the harness wrote.
pub fn is_turn_start(msg: &Message) -> bool {
    msg.role == Role::User
        && !msg.content.starts_with(RETURN_CODE_PREFIX)
        && !msg.content.starts_with(COMPACT_MARKER)
        && !msg.content.starts_with("Note: resumed an earlier session")
}

pub fn is_observation_message(index: usize, msg: &Message) -> bool {
    msg.role == Role::Tool
        || (msg.role == Role::User && index >= 2 && msg.content.starts_with(RETURN_CODE_PREFIX))
}

pub fn collapse_observation(content: &str, level: i32) -> String {
    if level == 1 && content.contains(PRUNE_MARKER) {
        return content.to_string();
    }
    let (header, output, ok) = split_observation(content);
    if !ok {
        if level >= 2 {
            return format!("{}\n{}", header_line(content), OUTPUT_ELIDED_LINE);
        }
        return content.to_string();
    }
    if level >= 2 {
        return format!("{header}\n{OUTPUT_OPEN_TAG}\n{OUTPUT_ELIDED_LINE}\n{OUTPUT_CLOSE_TAG}");
    }
    let lines: Vec<&str> = output.split('\n').collect();
    if lines.len() <= 4 {
        return content.to_string();
    }
    let first = take_first(&lines, 2);
    let last = take_last(&lines, 2);
    let kept = first
        .iter()
        .chain(last.iter())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let elided = output.len().saturating_sub(kept.len());
    if elided == 0 {
        return content.to_string();
    }
    let mut body = first;
    body.push(format!(
        "[{elided} characters elided - rerun the command to see them]"
    ));
    body.extend(last);
    format!(
        "{header}\n{OUTPUT_OPEN_TAG}\n{}\n{OUTPUT_CLOSE_TAG}",
        body.join("\n")
    )
}

pub fn split_observation(content: &str) -> (String, String, bool) {
    let lines: Vec<&str> = content.split('\n').collect();
    if lines.is_empty() || !lines[0].starts_with(RETURN_CODE_PREFIX) {
        return (header_line(content), String::new(), false);
    }
    let mut open = None;
    let mut close = None;
    for (i, line) in lines.iter().enumerate().skip(1) {
        if *line == OUTPUT_OPEN_TAG && open.is_none() {
            open = Some(i);
            continue;
        }
        if *line == OUTPUT_CLOSE_TAG {
            close = Some(i);
            break;
        }
    }
    let (Some(open), Some(close)) = (open, close) else {
        return (lines[0].to_string(), String::new(), false);
    };
    if close < open {
        return (lines[0].to_string(), String::new(), false);
    }
    (
        lines[0].to_string(),
        lines[open + 1..close].join("\n"),
        true,
    )
}

pub fn header_line(content: &str) -> String {
    content
        .split_once('\n')
        .map_or(content, |(h, _)| h)
        .to_string()
}

fn take_first(lines: &[&str], n: usize) -> Vec<String> {
    lines.iter().take(n).map(|s| (*s).to_string()).collect()
}

fn take_last(lines: &[&str], n: usize) -> Vec<String> {
    lines
        .iter()
        .skip(lines.len().saturating_sub(n))
        .map(|s| (*s).to_string())
        .collect()
}

pub(crate) fn shared_agent(agent: Agent) -> Arc<std::sync::Mutex<Agent>> {
    Arc::new(std::sync::Mutex::new(agent))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_provider::{Login, Model};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Condvar, Mutex};

    #[test]
    fn stored_observation_is_capped_at_output_limit() {
        // Under the limit the row holds the raw output, unmarked.
        assert_eq!(stored_observation("abcdef", 0), "abcdef");
        assert_eq!(stored_observation("abcdef", 6), "abcdef");
        // Over it the row keeps what the model saw plus a one-line marker,
        // never the full text (a 24 MB grep used to land in one row).
        let big = "x".repeat(100_000);
        let stored = stored_observation(&big, 1000);
        assert!(stored.starts_with("[stored: 100000 bytes, kept first/last 500 bytes]\n"));
        assert!(stored.contains("[... 99000 characters omitted ...]"));
        assert!(stored.len() < 1000 + 200, "len {}", stored.len());
    }

    #[test]
    fn truncate_cuts_on_char_boundaries() {
        assert_eq!(truncate("abcdef", 0), "abcdef");
        assert_eq!(truncate("abcdef", 6), "abcdef");
        assert_eq!(
            truncate("abcdefghij", 4),
            "ab\n[... 6 characters omitted ...]\nij"
        );
        // Multi-byte chars at either cut (the shapes seen in run logs: a
        // box-drawing table and Arabic JSON) are dropped whole, not split.
        let table = format!("{}─{}", "x".repeat(4), "y".repeat(4));
        // limit 10 puts both cuts inside the 3-byte '─'; limit 8 lands on
        // its edges. Either way the char is dropped whole.
        for limit in [8, 10] {
            assert_eq!(
                truncate(&table, limit),
                "xxxx\n[... 3 characters omitted ...]\nyyyy",
                "limit {limit}"
            );
        }
        let arabic = format!("{}ب{}ب{}", "a".repeat(3), "b".repeat(3), "c".repeat(3));
        let got = truncate(&arabic, 8);
        assert!(got.starts_with("aaa\n"), "{got:?}");
        assert!(got.ends_with("\nccc"), "{got:?}");
        for s in ["─".repeat(20), "عميل ".repeat(10), "a😀".repeat(10)] {
            for limit in 1..=12 {
                let out = truncate(&s, limit);
                assert!(out.contains("characters omitted"), "{s:?} {limit}");
            }
        }
    }

    #[test]
    fn one_line_cuts_on_char_boundaries() {
        assert_eq!(one_line(" a \n b  c ", 80), "a b c");
        assert_eq!(one_line("abcdef", 3), "abc");
        assert_eq!(one_line("ab—cd", 3), "ab");
        assert_eq!(one_line("ab—cd", 4), "ab");
        assert_eq!(one_line("ab—cd", 5), "ab—");
        assert_eq!(one_line("تغيّر هذا العميل", 5), "تغ");
    }

    struct FakeProvider {
        responses: Mutex<Vec<pulse_provider::Response>>,
        err: Option<anyhow::Error>,
        requests: Mutex<Vec<Request>>,
    }

    impl FakeProvider {
        fn new(responses: Vec<pulse_provider::Response>) -> Self {
            Self {
                responses: Mutex::new(responses),
                err: None,
                requests: Mutex::new(Vec::new()),
            }
        }
        fn with_err(err: anyhow::Error) -> Self {
            Self {
                responses: Mutex::new(Vec::new()),
                err: Some(err),
                requests: Mutex::new(Vec::new()),
            }
        }
        fn requests(&self) -> Vec<Request> {
            self.requests.lock().unwrap().clone()
        }
    }

    struct SharedFakeProvider(Arc<FakeProvider>);

    impl Provider for SharedFakeProvider {
        fn name(&self) -> &str {
            "fake"
        }
        fn label(&self) -> &str {
            "Fake"
        }
        fn chat(
            &self,
            _cancel: &CancelToken,
            req: Request,
        ) -> anyhow::Result<pulse_provider::Response> {
            self.0.requests.lock().unwrap().push(req);
            if let Some(err) = &self.0.err {
                if err.to_string() == "not logged in" {
                    return Err(pulse_provider::err_not_logged_in().into());
                }
                return Err(anyhow!("{err}"));
            }
            let mut responses = self.0.responses.lock().unwrap();
            if responses.is_empty() {
                return Err(anyhow!("no response"));
            }
            Ok(responses.remove(0))
        }
        fn models(&self, _cancel: &CancelToken) -> anyhow::Result<Vec<Model>> {
            Ok(Vec::new())
        }
        fn status(&self, _cancel: &CancelToken) -> anyhow::Result<Login> {
            Ok(Login::default())
        }
    }

    struct BudgetAwareProvider {
        requests: Mutex<Vec<Request>>,
    }

    impl BudgetAwareProvider {
        fn requests(&self) -> Vec<Request> {
            self.requests.lock().unwrap().clone()
        }
    }

    struct SharedBudgetProvider(Arc<BudgetAwareProvider>);

    impl Provider for SharedBudgetProvider {
        fn name(&self) -> &str {
            "budget-aware"
        }
        fn label(&self) -> &str {
            "Budget Aware"
        }
        fn chat(
            &self,
            _cancel: &CancelToken,
            req: Request,
        ) -> anyhow::Result<pulse_provider::Response> {
            self.0.requests.lock().unwrap().push(req.clone());
            if request_has_budget_notice(&req) {
                return Ok(resp(msg("SUMMARY: done after notice")));
            }
            Ok(resp(tool("true")))
        }
        fn models(&self, _cancel: &CancelToken) -> anyhow::Result<Vec<Model>> {
            Ok(Vec::new())
        }
        fn status(&self, _cancel: &CancelToken) -> anyhow::Result<Login> {
            Ok(Login::default())
        }
    }

    struct BlockingProvider {
        started: (Mutex<bool>, Condvar),
    }

    struct SharedBlockingProvider(Arc<BlockingProvider>);

    impl Provider for SharedBlockingProvider {
        fn name(&self) -> &str {
            "blocking"
        }
        fn label(&self) -> &str {
            "Blocking"
        }
        fn chat(
            &self,
            cancel: &CancelToken,
            _req: Request,
        ) -> anyhow::Result<pulse_provider::Response> {
            let (lock, cv) = &self.0.started;
            *lock.lock().unwrap() = true;
            cv.notify_all();
            while !cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(anyhow!("context canceled"))
        }
        fn models(&self, _cancel: &CancelToken) -> anyhow::Result<Vec<Model>> {
            Ok(Vec::new())
        }
        fn status(&self, _cancel: &CancelToken) -> anyhow::Result<Login> {
            Ok(Login::default())
        }
    }

    struct TestTrajectorySink {
        path: String,
        steps: Vec<Step>,
    }

    impl Sink for TestTrajectorySink {
        fn step(&mut self, step: Step) {
            self.steps.push(step);
            let traj = Trajectory {
                trajectory_format: "pulse-harness-1".into(),
                info: TrajectoryInfo {
                    model_stats: ModelStats {
                        api_calls: 0,
                        prompt_tokens: 0,
                        completion_tokens: 0,
                        cost: 0.0,
                    },
                    exit_status: String::new(),
                    submission: String::new(),
                    config: TrajectoryConfig {
                        model: String::new(),
                        provider: String::new(),
                        max_steps: 0,
                        step_timeout_seconds: 0,
                        cwd: String::new(),
                        interactive: false,
                    },
                },
                messages: Vec::new(),
                steps: self.steps.clone(),
            };
            super::write_trajectory(&self.path, &traj).unwrap();
        }
    }

    #[derive(Default)]
    struct FakeEnv {
        observations: Vec<Observation>,
        commands: Vec<String>,
        timeouts: Vec<Duration>,
        default_output: String,
    }

    impl Environment for Arc<Mutex<FakeEnv>> {
        fn execute(
            &mut self,
            _cancel: &CancelToken,
            command: &str,
            timeout: Duration,
        ) -> Observation {
            let mut e = self.lock().unwrap();
            e.commands.push(command.to_string());
            e.timeouts.push(timeout);
            if e.observations.is_empty() {
                Observation {
                    output: e.default_output.clone(),
                    exit_code: 0,
                    ..Observation::default()
                }
            } else {
                e.observations.remove(0)
            }
        }
        fn info(&self) -> BTreeMap<String, String> {
            BTreeMap::from([
                ("cwd".into(), "/work".into()),
                ("os".into(), "testos".into()),
                ("shell".into(), "/bin/sh".into()),
            ])
        }
        fn fork(&self) -> Option<Box<dyn Environment>> {
            Some(Box::new(self.clone()))
        }
    }

    fn msg(content: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: String::new(),
            name: String::new(),
            ..Default::default()
        }
    }

    fn tool(command: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "call-1".into(),
                name: "bash".into(),
                arguments: pulse_store::gojson::to_string(&json!({ "command": command })).unwrap(),
            }],
            tool_call_id: String::new(),
            name: String::new(),
            ..Default::default()
        }
    }

    fn resp(message: Message) -> pulse_provider::Response {
        pulse_provider::Response {
            message: Some(message),
            ..pulse_provider::Response::default()
        }
    }

    #[test]
    fn runs_git_commit_detects_hook_firing_subcommands() {
        for c in [
            "git commit -m x",
            "git add -A && git commit -am 'x'",
            "cd crates && git -C .. commit -q -m x",
            "git --no-pager merge --no-ff topic",
            "git rebase main",
            "git cherry-pick abc",
            "git am < p.patch",
            "(git commit -m x)",
        ] {
            assert!(runs_git_commit(c), "{c}");
        }
        for c in [
            "git status",
            "git log --oneline -3",
            "git diff --cached",
            "cargo test commit_timeout",
            "echo 'git commit' > note.txt",
            "git committer-check",
            "forgit commit",
        ] {
            assert!(!runs_git_commit(c), "{c}");
        }
    }

    #[test]
    fn verify_gate_in_reads_runner_hook_only() {
        let tmp = tempfile::tempdir().unwrap();
        let clone = tmp.path().join("clone");
        let hooks = clone.join(".git").join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        assert_eq!(verify_gate_in(&clone), None);
        std::fs::write(hooks.join("pre-commit"), "#!/bin/sh\ncargo fmt --check\n").unwrap();
        assert_eq!(
            verify_gate_in(&clone),
            None,
            "a foreign hook is not the gate"
        );
        std::fs::write(
            hooks.join("pre-commit"),
            concat!(
                "#!/bin/sh\n",
                "# pulse: verify-before-commit gate (auto-installed by runner)\n",
                "unset GIT_INDEX_FILE\n",
                "export CI=1\n",
                "(\n",
                "cd /tmp/clone\n",
                "env X=1 sh -c 'cargo test --workspace'\n",
                ")\n",
                "rc=$?\n",
            ),
        )
        .unwrap();
        let gate = verify_gate_in(&clone).expect("runner hook is the gate");
        assert_eq!(gate.verify_cmd, "env X=1 sh -c 'cargo test --workspace'");
        // A worktree's `.git` file points at `<main>/.git/worktrees/<n>`;
        // the hook lives in the main `.git/hooks`.
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        let wt_git = clone.join(".git").join("worktrees").join("wt");
        std::fs::create_dir_all(&wt_git).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", wt_git.display())).unwrap();
        assert_eq!(verify_gate_in(&wt), Some(gate));
    }

    #[test]
    fn format_commit_timeout_names_gate_and_lock_wait() {
        let obs = Observation {
            output: "pulse: another verification holds x.lock (pid 7); waiting for it to finish\npulse: verification ran 0s after waiting 390s for the lock\n".into(),
            exit_code: -1,
            duration: Duration::from_secs(1500),
            timed_out: true,
        };
        let gate = VerifyGate {
            verify_cmd: "cargo test".into(),
        };
        let text = format_commit_timeout(&obs, 20_000, &gate);
        assert!(text.contains("the pre-commit verify gate (cargo test) was still running after 1500s (lock wait 390s)"), "{text}");
        assert!(text.contains("do not background, nohup or sleep around git commit"));
        assert!(!text.contains("smaller scope"));
        assert_eq!(lock_wait_from_output("nothing here"), 0);
    }

    #[test]
    fn gated_commit_gets_commit_timeout_and_specific_message() {
        let tmp = tempfile::tempdir().unwrap();
        let hooks = tmp.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(
            hooks.join("pre-commit"),
            "#!/bin/sh\n# pulse: verify-before-commit gate (auto-installed by runner)\n(\ncargo test\n)\n",
        )
        .unwrap();
        let p = Arc::new(FakeProvider::new(vec![
            resp(tool("git status")),
            resp(tool("git commit -m x")),
            resp(msg("SUMMARY: done")),
        ]));
        let env = Arc::new(Mutex::new(FakeEnv {
            observations: vec![
                Observation {
                    output: "clean\n".into(),
                    exit_code: 0,
                    ..Observation::default()
                },
                Observation {
                    output: "pulse: verification ran 0s after waiting 120s for the lock\n".into(),
                    exit_code: -1,
                    duration: Duration::from_secs(1500),
                    timed_out: true,
                },
            ],
            ..FakeEnv::default()
        }));
        let mut cfg = base_config();
        cfg.cwd = tmp.path().display().to_string();
        cfg.step_timeout = Duration::from_secs(900);
        cfg.commit_timeout = Duration::from_secs(1500);
        cfg.wall_time = Duration::from_secs(3 * 3600);
        let (h, got) = run_agent(p, env.clone(), cfg).unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        let timeouts = env.lock().unwrap().timeouts.clone();
        assert_eq!(
            timeouts,
            vec![Duration::from_secs(900), Duration::from_secs(1500)],
            "plain commands keep step_timeout; the gated commit gets commit_timeout"
        );
        let msgs = h.messages();
        let tool_msgs: Vec<&Message> = msgs.iter().filter(|m| m.role == Role::Tool).collect();
        assert!(
            tool_msgs[1].content.contains(
                "pre-commit verify gate (cargo test) was still running after 1500s (lock wait 120s)"
            ),
            "{}",
            tool_msgs[1].content
        );
        assert!(!tool_msgs[1].content.contains("smaller scope"));
    }

    #[test]
    fn commit_timeout_is_capped_by_remaining_wall_clock_and_floored_by_step_timeout() {
        let tmp = tempfile::tempdir().unwrap();
        let hooks = tmp.path().join(".git").join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(
            hooks.join("pre-commit"),
            "# pulse: verify-before-commit gate (auto-installed by runner)\n",
        )
        .unwrap();
        let p = Arc::new(FakeProvider::new(vec![
            resp(tool("git commit -m x")),
            resp(msg("SUMMARY: done")),
        ]));
        let env = Arc::new(Mutex::new(FakeEnv::default()));
        let mut cfg = base_config();
        cfg.cwd = tmp.path().display().to_string();
        cfg.step_timeout = Duration::from_secs(300);
        cfg.commit_timeout = Duration::from_secs(1500);
        // 10 min of wall clock minus the 60 s reserve leaves 540 s.
        cfg.wall_time = Duration::from_secs(600);
        run_agent(p, env.clone(), cfg).unwrap();
        let t = env.lock().unwrap().timeouts[0];
        assert!(
            t <= Duration::from_secs(540) && t >= Duration::from_secs(300),
            "{t:?}"
        );

        let p = Arc::new(FakeProvider::new(vec![
            resp(tool("git commit -m x")),
            resp(msg("SUMMARY: done")),
        ]));
        let env = Arc::new(Mutex::new(FakeEnv::default()));
        let mut cfg = base_config();
        cfg.cwd = tmp.path().display().to_string();
        cfg.step_timeout = Duration::from_secs(900);
        cfg.commit_timeout = Duration::from_secs(60);
        run_agent(p, env.clone(), cfg).unwrap();
        assert_eq!(
            env.lock().unwrap().timeouts[0],
            Duration::from_secs(900),
            "never below step_timeout"
        );
    }

    fn base_config() -> Config {
        Config {
            model: "m".into(),
            task: "do it".into(),
            system_prompt: "system".into(),
            max_steps: 10,
            ..Config::default()
        }
    }

    fn request_has_budget_notice(req: &Request) -> bool {
        req.messages
            .iter()
            .any(|m| m.role == Role::User && m.content.starts_with("Budget notice:"))
    }

    fn request_chars_for_test(req: &Request) -> usize {
        req.messages
            .iter()
            .map(|m| {
                m.content.len()
                    + m.tool_calls
                        .iter()
                        .map(|call| call.arguments.len())
                        .sum::<usize>()
            })
            .sum()
    }

    fn run_agent(
        p: Arc<FakeProvider>,
        env: Arc<Mutex<FakeEnv>>,
        cfg: Config,
    ) -> anyhow::Result<(Agent, ResultSummary)> {
        let mut h = new(
            Box::new(SharedFakeProvider(p)),
            Box::new(env),
            cfg,
            Vec::new(),
        );
        let got = h.run()?;
        Ok((h, got))
    }

    #[test]
    fn assistant_step_records_finish_reason_and_choices() {
        let _serial = crate::test_helpers::serial();
        let p = Arc::new(FakeProvider::new(vec![
            resp(tool("true")),
            pulse_provider::Response {
                message: Some(msg("SUMMARY: done")),
                finish_reason: "stop".into(),
                choices: 2,
                ..pulse_provider::Response::default()
            },
        ]));
        let env = Arc::new(Mutex::new(FakeEnv::default()));
        let seen: Arc<Mutex<Vec<Step>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_steps = seen.clone();
        let mut h = new(
            Box::new(SharedFakeProvider(p)),
            Box::new(env),
            base_config(),
            vec![Box::new(SinkFunc(move |s: Step| {
                sink_steps.lock().unwrap().push(s)
            }))],
        );
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        let steps = seen.lock().unwrap();
        let assistant: Vec<&Step> = steps
            .iter()
            .filter(|s| s.kind == StepKind::Assistant)
            .collect();
        assert_eq!(assistant.len(), 2);
        assert_eq!(assistant[1].finish_reason.as_deref(), Some("stop"));
        assert_eq!(assistant[1].choices, Some(2));
        // Non-assistant steps carry neither field.
        assert!(
            steps
                .iter()
                .filter(|s| s.kind != StepKind::Assistant)
                .all(|s| s.finish_reason.is_none() && s.choices.is_none())
        );
        // Store rows round-trip both columns.
        pulse_testutil::neutralize_runner_environment();
        let tmp = tempfile::tempdir().unwrap();
        let st = pulse_store::Store::open(pulse_store::Options {
            home: tmp.path().display().to_string(),
            create: true,
            now: "2026-10-12T00:00:00Z".into(),
            ..pulse_store::Options::default()
        })
        .unwrap();
        st.begin_harness_session(pulse_store::HarnessSession {
            id: "hs-1".into(),
            ..Default::default()
        })
        .unwrap();
        st.append_harness_step(pulse_store::HarnessStep {
            session_id: "hs-1".into(),
            n: 1,
            at: "2026-10-12T00:00:00Z".into(),
            kind: "assistant".into(),
            finish_reason: Some("length".into()),
            choices: Some(1),
            ..Default::default()
        })
        .unwrap();
        let rows = st.harness_steps("hs-1", 0).unwrap();
        assert_eq!(rows[0].finish_reason.as_deref(), Some("length"));
        assert_eq!(rows[0].choices, Some(1));
    }

    #[test]
    fn tool_call_path_submits() {
        let _serial = crate::test_helpers::serial();
        let p = Arc::new(FakeProvider::new(vec![
            pulse_provider::Response {
                message: Some(tool("echo hi")),
                usage: Usage {
                    prompt_tokens: 1,
                    completion_tokens: 2,
                    cost: 0.5,
                },
                model: "m-resolved".into(),
                ..pulse_provider::Response::default()
            },
            pulse_provider::Response {
                message: Some(msg("SUMMARY: done\nFINDING: ok")),
                usage: Usage {
                    prompt_tokens: 3,
                    ..Usage::default()
                },
                ..pulse_provider::Response::default()
            },
        ]));
        let env = Arc::new(Mutex::new(FakeEnv {
            observations: vec![Observation {
                output: "hi\n".into(),
                exit_code: 0,
                duration: Duration::from_millis(1),
                ..Observation::default()
            }],
            ..FakeEnv::default()
        }));
        let (h, got) = run_agent(p.clone(), env, base_config()).unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        assert!(got.submission.contains("SUMMARY: done"));
        assert_eq!((got.steps, got.calls), (1, 2));
        let reqs = p.requests();
        assert_eq!(reqs[0].initiator, "user");
        assert_eq!(reqs[1].initiator, "agent");
        assert_eq!(h.messages()[3].role, Role::Tool);
        assert!(
            h.messages()[3]
                .content
                .contains("<returncode>0</returncode>")
        );
    }

    #[test]
    fn default_prompt_template_renders_like_default_system_prompt() {
        let _serial = crate::test_helpers::serial();
        let cfg = Config {
            cwd: "/repo".into(),
            model: "m".into(),
            provider: "fake".into(),
            max_steps: 2,
            step_timeout: Duration::from_secs(15),
            output_limit: 1234,
            ..Config::default()
        };
        let info = BTreeMap::from([
            ("cwd".into(), "/work".into()),
            ("os".into(), "testos".into()),
            ("shell".into(), "/bin/zsh".into()),
        ]);
        let rendered = render_prompt(DEFAULT_PROMPT_TEMPLATE, prompt_data(&info, &cfg)).unwrap();
        assert_eq!(rendered, default_system_prompt(&info, cfg.clone()));
        assert!(rendered.contains("Commands time out after 15s."));
        assert!(rendered.contains("Output over 1234 characters"));
        assert!(rendered.contains("already the clone root"));
        assert!(rendered.contains("exactly one fenced block"));
        let custom = render_prompt(
            "{{.Cwd}} {{.Provider}} {{.Model}} {{.Info.shell}}",
            prompt_data(&info, &cfg),
        )
        .unwrap();
        assert_eq!(custom, "/work fake m /bin/zsh");
    }

    #[test]
    fn lenient_action_parser_accepts_live_shapes() {
        let _serial = crate::test_helpers::serial();
        assert_eq!(
            bash_blocks("~~~shell\n$ cargo test -p pulse-harness\n~~~"),
            vec!["cargo test -p pulse-harness".to_string()]
        );
        assert_eq!(
            bash_blocks("```console\n$ rg harness README.md\n```"),
            vec!["rg harness README.md".to_string()]
        );
        assert_eq!(
            bash_blocks("`cargo fmt --all`"),
            vec!["cargo fmt --all".to_string()]
        );
        assert!(matches!(
            action_blocks("```read\nREADME.md:1-5\n```").as_slice(),
            [ActionBlock::Read(body)] if body == "README.md:1-5"
        ));
        assert!(matches!(
            action_blocks("```grep\nharness\nREADME.md\n```").as_slice(),
            [ActionBlock::Grep(body)] if body.contains("harness")
        ));
        assert!(matches!(
            action_blocks("```edit\nREADME.md\n<<<<<<< SEARCH\na\n=======\nb\n>>>>>>> REPLACE\n```")
                .as_slice(),
            [ActionBlock::Edit(body)] if body.contains("<<<<<<< SEARCH")
        ));
        assert!(is_report("### report\nsummary: done"));
    }

    #[test]
    fn prose_only_retry_is_recorded_without_consuming_step_budget_once() {
        let _serial = crate::test_helpers::serial();
        let mut cfg = base_config();
        cfg.max_steps = 1;
        cfg.format_retries = 1;
        cfg.max_format_errors = 2;
        let p = Arc::new(FakeProvider::new(vec![
            resp(msg("I will inspect first.")),
            resp(msg("Still planning.")),
        ]));
        let (h, got) = run_agent(p, Arc::new(Mutex::new(FakeEnv::default())), cfg).unwrap();
        assert_eq!(got.exit_status, EXIT_STEP_LIMIT);
        assert_eq!(got.calls, 1);
        assert_eq!(got.format_errors, 2);
        assert!(
            h.trajectory()
                .steps
                .iter()
                .any(|s| s.kind == StepKind::FormatRetry)
        );
        assert!(
            h.trajectory()
                .steps
                .iter()
                .any(|s| s.kind == StepKind::Error)
        );
    }

    #[test]
    fn first_turn_report_without_tool_calls_is_sent_back_once() {
        // Six runs in 30h ended at step 1 with "I ran no commands". The
        // first evidence-free report costs one more turn; the second one is
        // accepted so no loop forms.
        let _serial = crate::test_helpers::serial();
        let p = Arc::new(FakeProvider::new(vec![
            resp(msg("SUMMARY: I ran no commands and read no files.")),
            resp(msg("SUMMARY: still nothing to run.")),
        ]));
        let (h, got) =
            run_agent(p, Arc::new(Mutex::new(FakeEnv::default())), base_config()).unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        assert_eq!(got.calls, 2);
        assert_eq!(got.format_errors, 0);
        assert_eq!(
            h.trajectory()
                .steps
                .iter()
                .filter(|s| s.kind == StepKind::FormatRetry)
                .count(),
            1
        );
        let bounced = h
            .messages()
            .iter()
            .filter(|m| {
                m.role == Role::User && m.content.starts_with("Your report says nothing was run.")
            })
            .count();
        assert_eq!(bounced, 1);
        assert_eq!(got.submission, "SUMMARY: still nothing to run.");
    }

    #[test]
    fn report_after_a_tool_call_is_submitted_at_once() {
        let _serial = crate::test_helpers::serial();
        let p = Arc::new(FakeProvider::new(vec![
            resp(tool("git log -5 --stat")),
            resp(msg("SUMMARY: looked at the log.")),
        ]));
        let (h, got) =
            run_agent(p, Arc::new(Mutex::new(FakeEnv::default())), base_config()).unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        assert_eq!(got.calls, 2);
        assert!(
            !h.trajectory()
                .steps
                .iter()
                .any(|s| s.kind == StepKind::FormatRetry)
        );
    }

    #[test]
    fn narration_with_tool_call_runs_the_command_without_a_format_retry() {
        // The providers merge a reply that Copilot split into a text choice
        // and a tool-call choice; the harness must act on the call and not
        // treat the narration as a prose-only turn.
        let _serial = crate::test_helpers::serial();
        let mut both = tool("cargo test");
        both.content = "Lint passes. Running tests.".into();
        let p = Arc::new(FakeProvider::new(vec![
            resp(both),
            resp(msg("SUMMARY: tests pass")),
        ]));
        let env = Arc::new(Mutex::new(FakeEnv::default()));
        let (h, got) = run_agent(p, env.clone(), base_config()).unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        assert_eq!(got.format_errors, 0);
        assert_eq!(env.lock().unwrap().commands, vec!["cargo test".to_string()]);
        assert!(
            !h.trajectory()
                .steps
                .iter()
                .any(|s| matches!(s.kind, StepKind::FormatRetry | StepKind::Error))
        );
        assert!(h.messages().iter().any(|m| m.role == Role::Tool));
    }

    #[test]
    fn lost_working_directory_ends_the_run_at_once() {
        // The clone was swept under the run: /bin/sh cannot even start in
        // it. Nineteen sessions in a month kept asking the model for the
        // next command and got `os error 2` back every time.
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().display().to_string();
        drop(dir);
        let p = Arc::new(FakeProvider::new(vec![
            resp(tool("pwd")),
            resp(tool("ls")),
            resp(msg("SUMMARY: gave up")),
        ]));
        let env = Arc::new(Mutex::new(FakeEnv {
            observations: vec![Observation {
                output: "No such file or directory (os error 2)".into(),
                exit_code: 127,
                ..Observation::default()
            }],
            ..FakeEnv::default()
        }));
        let cfg = Config {
            cwd: cwd.clone(),
            ..base_config()
        };
        let (h, got) = run_agent(p, env.clone(), cfg).unwrap();
        assert_eq!(got.exit_status, EXIT_SANDBOX_LOST, "{got:?}");
        assert_eq!(got.calls, 1, "no second model call once the clone is gone");
        assert_eq!(env.lock().unwrap().commands, vec!["pwd".to_string()]);
        assert!(
            got.error.contains(&cwd) && got.error.contains("no longer exists"),
            "{}",
            got.error
        );
        let steps = h.trajectory().steps;
        assert!(
            steps
                .iter()
                .any(|s| s.kind == StepKind::Error && s.content.contains("no longer exists")),
            "{steps:?}"
        );
        assert_eq!(
            steps.last().map(|s| s.content.as_str()),
            Some(EXIT_SANDBOX_LOST)
        );
    }

    #[test]
    fn command_not_found_in_a_live_working_directory_is_not_a_lost_sandbox() {
        // Exit 127 is also what the shell returns for an unknown command;
        // only a missing directory ends the run.
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let p = Arc::new(FakeProvider::new(vec![
            resp(tool("nope")),
            resp(msg("SUMMARY: no such tool here")),
        ]));
        let env = Arc::new(Mutex::new(FakeEnv {
            observations: vec![Observation {
                output: "sh: nope: command not found".into(),
                exit_code: 127,
                ..Observation::default()
            }],
            ..FakeEnv::default()
        }));
        let cfg = Config {
            cwd: dir.path().display().to_string(),
            ..base_config()
        };
        let (_, got) = run_agent(p, env.clone(), cfg).unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED, "{got:?}");
        assert_eq!(got.calls, 2);
    }

    #[test]
    fn text_mode_path_and_two_block_format_error() {
        let _serial = crate::test_helpers::serial();
        let p = Arc::new(FakeProvider::new(vec![
            resp(msg("```bash\necho hi\n```")),
            resp(msg("SUMMARY: done")),
        ]));
        let env = Arc::new(Mutex::new(FakeEnv {
            observations: vec![Observation {
                output: "hi\n".into(),
                exit_code: 0,
                ..Observation::default()
            }],
            ..FakeEnv::default()
        }));
        let (h, got) = run_agent(p, env, base_config()).unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        assert_eq!(h.messages()[3].role, Role::User);
        assert!(h.messages()[3].content.contains("hi"));

        let mut cfg = base_config();
        cfg.max_format_errors = 1;
        let p = Arc::new(FakeProvider::new(vec![resp(msg(
            "```bash\na\n```\n```sh\nb\n```",
        ))]));
        let env = Arc::new(Mutex::new(FakeEnv::default()));
        let (h, got) = run_agent(p, env, cfg).unwrap();
        assert_eq!(got.exit_status, EXIT_FORMAT_ERROR);
        let last_user = &h.messages()[3];
        assert!(last_user.content.contains("write exactly one"));
    }

    #[test]
    fn format_errors_stop_after_three_plan_only_messages() {
        let _serial = crate::test_helpers::serial();
        let p = Arc::new(FakeProvider::new(vec![
            resp(msg("I will inspect first.")),
            resp(msg("Plan: run tests.")),
            resp(msg("Next I will edit.")),
        ]));
        let (h, got) =
            run_agent(p, Arc::new(Mutex::new(FakeEnv::default())), base_config()).unwrap();
        assert_eq!(got.exit_status, EXIT_FORMAT_ERROR);
        assert_eq!(got.calls, 3);
        assert_eq!(
            h.messages()
                .iter()
                .filter(|m| m.content.contains("no command and no report"))
                .count(),
            3
        );
    }

    #[test]
    fn native_read_grep_edit_blocks_run_on_temp_clone() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("lib.rs"),
            "fn main() {\n    println!(\"old\");\n}\n",
        )
        .unwrap();
        let cfg = Config {
            cwd: dir.path().display().to_string(),
            model: "m".into(),
            task: "do it".into(),
            system_prompt: "system".into(),
            max_steps: 10,
            ..Config::default()
        };
        let p = Arc::new(FakeProvider::new(vec![
            resp(msg("```read\nlib.rs:1-2\n```")),
            resp(msg("```grep\nprintln\nlib.rs\n```")),
            resp(msg(
                "```edit\nlib.rs\n<<<<<<< SEARCH\nprintln!(\"old\");\n=======\nprintln!(\"new\");\n>>>>>>> REPLACE\n```",
            )),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = new(
            Box::new(SharedFakeProvider(p)),
            Box::new(crate::localenv::new_local_env(
                dir.path().display().to_string(),
                None,
            )),
            cfg,
            Vec::new(),
        );
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        assert_eq!(got.steps, 3);
        assert!(
            fs::read_to_string(dir.path().join("lib.rs"))
                .unwrap()
                .contains("new")
        );
        let steps = h.trajectory().steps;
        assert!(steps.iter().any(|s| s.kind == StepKind::Read));
        assert!(steps.iter().any(|s| s.kind == StepKind::Grep));
        assert!(steps.iter().any(|s| s.kind == StepKind::Edit));
        assert!(
            h.messages()
                .iter()
                .any(|m| m.content.contains("     1\tfn main()"))
        );
    }

    #[test]
    fn native_edit_reports_zero_multi_and_create() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("dup.txt"), "same\nsame\n").unwrap();
        let mut cfg = base_config();
        cfg.cwd = dir.path().display().to_string();
        cfg.max_steps = 10;
        let p = Arc::new(FakeProvider::new(vec![
            resp(msg(
                "```edit\ndup.txt\n<<<<<<< SEARCH\nmissing\n=======\nnew\n>>>>>>> REPLACE\n```",
            )),
            resp(msg(
                "```edit\ndup.txt\n<<<<<<< SEARCH\nsame\n=======\nnew\n>>>>>>> REPLACE\n```",
            )),
            resp(msg(
                "```edit\ncreated.txt\n<<<<<<< SEARCH\n=======\nhello\n>>>>>>> REPLACE\n```",
            )),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = new(
            Box::new(SharedFakeProvider(p)),
            Box::new(crate::localenv::new_local_env(
                dir.path().display().to_string(),
                None,
            )),
            cfg,
            Vec::new(),
        );
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        let joined = h
            .messages()
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("matched 0 times"));
        assert!(joined.contains("matched 2 times"));
        assert_eq!(
            fs::read_to_string(dir.path().join("created.txt")).unwrap(),
            "hello"
        );
    }

    /// An assistant turn made of typed tool calls, each `(id, tool, args)`.
    fn calls(items: &[(&str, &str, Value)]) -> Message {
        Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: items
                .iter()
                .map(|(id, name, args)| ToolCall {
                    id: (*id).to_string(),
                    name: (*name).to_string(),
                    arguments: pulse_store::gojson::to_string(args).unwrap(),
                })
                .collect(),
            tool_call_id: String::new(),
            name: String::new(),
            ..Default::default()
        }
    }

    fn local_agent(dir: &Path, p: Arc<FakeProvider>) -> Agent {
        let mut cfg = base_config();
        cfg.cwd = dir.display().to_string();
        cfg.max_steps = 20;
        new(
            Box::new(SharedFakeProvider(p)),
            Box::new(crate::localenv::new_local_env(
                dir.display().to_string(),
                None,
            )),
            cfg,
            Vec::new(),
        )
    }

    fn tool_replies(h: &Agent) -> Vec<(String, String)> {
        h.messages()
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| (m.tool_call_id.clone(), m.content.clone()))
            .collect()
    }

    #[test]
    fn path_rules_come_with_the_first_file_they_cover_and_once_only() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let write = |rel: &str, text: &str| {
            let p = dir.path().join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        };
        write(
            ".github/instructions/ts.instructions.md",
            "---\napplyTo: \"src/**/*.ts\"\n---\nUse strict TypeScript.\n",
        );
        write("src/api/AGENTS.md", "Handlers return Result.\n");
        write("src/api/a.ts", "export const a = 1;\n");
        write("src/api/b.ts", "export const b = 2;\n");
        write("src/other.md", "nothing special\n");
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[
                ("r1", "read", json!({"path":"src/api/a.ts"})),
                ("r2", "read", json!({"path":"src/other.md"})),
            ])),
            resp(calls(&[(
                "e1",
                "edit",
                json!({"path":"src/api/b.ts","search":"= 2","replace":"= 3"}),
            )])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p);
        assert!(h.environment().iter().any(|l| l == "path rules: off"));
        let cwd = dir.path().to_str().unwrap();
        h.set_path_rules(Some(load_instructions(cwd, "").path_rules(cwd)));
        assert!(
            h.environment()
                .iter()
                .any(|l| l == "path rules: .github/instructions/ts.instructions.md (src/**/*.ts)"),
            "{:?}",
            h.environment()
        );
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED, "{got:?}");
        let replies: BTreeMap<String, String> = tool_replies(&h).into_iter().collect();
        let r1 = &replies["r1"];
        assert!(r1.contains("export const a = 1;"), "{r1}");
        assert!(
            r1.contains("[Instructions from src/api/AGENTS.md (for everything under src/api/). They apply to src/api/a.ts"),
            "{r1}"
        );
        assert!(r1.contains("Handlers return Result."), "{r1}");
        assert!(
            r1.contains("[Instructions from .github/instructions/ts.instructions.md (applies to src/**/*.ts)"),
            "{r1}"
        );
        assert!(r1.contains("Use strict TypeScript."), "{r1}");
        assert!(
            !replies["r2"].contains("[Instructions"),
            "a file no rule covers brings none: {}",
            replies["r2"]
        );
        assert!(
            !replies["e1"].contains("[Instructions"),
            "rules already given are not repeated: {}",
            replies["e1"]
        );
        let notes: Vec<String> = h
            .trajectory()
            .steps
            .iter()
            .filter(|s| s.kind == StepKind::Note)
            .map(|s| s.content.clone())
            .collect();
        assert_eq!(
            notes,
            [
                "instructions for src/api/a.ts from src/api/AGENTS.md (for everything under src/api/; 23 bytes)",
                "instructions for src/api/a.ts from .github/instructions/ts.instructions.md (applies to src/**/*.ts; 22 bytes)",
            ],
            "{notes:?}"
        );
    }

    #[test]
    fn custom_agents_hand_the_helper_their_instructions_and_tool_list() {
        let _serial = crate::test_helpers::serial();
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[(
                "c1",
                "subagent",
                json!({"task": "look at src", "agent": "reviewer", "max_steps": 5}),
            )])),
            resp(tool("rm -rf src")),
            resp(msg("SUMMARY: looked, nothing removed")),
            resp(calls(&[(
                "c2",
                "subagent",
                json!({"task": "anything", "agent": "nobody"}),
            )])),
            resp(msg("SUMMARY: done")),
        ]));
        let env = Arc::new(Mutex::new(FakeEnv::default()));
        let mut cfg = base_config();
        cfg.subagents = true;
        cfg.max_steps = 12;
        let mut h = new(
            Box::new(SharedFakeProvider(p.clone())),
            Box::new(env.clone()),
            cfg,
            Vec::new(),
        );
        h.set_permissions(rules(&[(false, "bash(rm *)")]));
        assert!(h.environment().iter().any(|l| l == "agents: none"));
        h.set_agents(vec![crate::agents::AgentDef {
            name: "reviewer".into(),
            description: "Reviews code.".into(),
            tools: Some(vec![
                "read".into(),
                "grep".into(),
                "glob".into(),
                "bash".into(),
            ]),
            denied: Vec::new(),
            body: "Report only; never edit.".into(),
            source: ".claude/agents/reviewer.md".into(),
        }]);
        assert!(
            h.environment()
                .iter()
                .any(|l| l == "agents: reviewer (.claude/agents/reviewer.md)"),
            "{:?}",
            h.environment()
        );
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED, "{got:?}");
        assert_eq!(got.subagents, 1);
        assert!(
            env.lock().unwrap().commands.is_empty(),
            "the inherited deny rule stopped the helper's rm"
        );
        let reqs = p.requests();
        assert_eq!(reqs.len(), 5);
        let sub = reqs[0]
            .tools
            .iter()
            .find(|t| t.name == SUBAGENT_TOOL)
            .unwrap();
        assert!(
            sub.description
                .contains("Custom agents (pass one as agent; it brings its own instructions and tools): reviewer: Reviews code."),
            "{}",
            sub.description
        );
        assert_eq!(
            sub.parameters["properties"]["agent"]["enum"],
            json!(["reviewer"])
        );
        let child = &reqs[1];
        let brief = &child.messages[1].content;
        assert!(
            brief.starts_with("You are the reviewer helper agent of a larger run"),
            "{brief}"
        );
        assert!(
            brief.contains(
                "--- .claude/agents/reviewer.md ---\nReport only; never edit.\n--- end of reviewer ---\n\nSub-task:\nlook at src"
            ),
            "{brief}"
        );
        let mut names: Vec<&str> = child.tools.iter().map(|t| t.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            ["bash", "glob", "grep", "read"],
            "tools outside the agent's list are not offered"
        );
        assert!(
            h.steps.iter().any(|s| s.depth == 1
                && s.kind == StepKind::Error
                && s.content
                    .contains("refused by permission rule `deny bash(rm *)`")),
            "the helper inherits the run's rules"
        );
        let sub_step = h
            .steps
            .iter()
            .find(|s| s.kind == StepKind::Subagent)
            .unwrap();
        assert_eq!(sub_step.command, "subagent reviewer (5 calls)");
        let replies: BTreeMap<String, String> = tool_replies(&h).into_iter().collect();
        assert!(
            replies["c1"].contains("SUMMARY: looked, nothing removed"),
            "{}",
            replies["c1"]
        );
        assert_eq!(
            replies["c2"],
            "subagent: there is no custom agent \"nobody\"; the agents are reviewer"
        );
    }

    #[test]
    fn user_prompt_and_pre_compact_hooks_speak_before_the_model_and_the_summary() {
        let _serial = crate::test_helpers::serial();
        let dir = chat_dir();
        let p = Arc::new(FakeProvider::new(vec![
            resp(msg("hi")),
            resp(tool("echo 1")),
            resp(msg("one done")),
            resp(tool("echo 2")),
            resp(msg("two done")),
            resp(msg("Summary: ran echo 1 and 2.")),
        ]));
        let steps = Arc::new(Mutex::new(Vec::new()));
        let mut h = chat_agent(dir.path(), p.clone(), None, steps);
        h.set_hooks(HookSet {
            hooks: vec![
                hook(HookEvent::UserPrompt, &[], "cat"),
                hook(
                    HookEvent::UserPrompt,
                    &[],
                    "if grep -q nope; then echo no nope please; exit 2; fi",
                ),
            ],
            ..HookSet::default()
        });
        let cancel = CancelToken::new();
        let first = h.turn(&cancel, "hello").unwrap();
        assert!(matches!(&first, Turn::Reply(t) if t == "hi"), "{first:?}");
        let reqs = p.requests();
        let sent = &reqs[0].messages.last().unwrap().content;
        assert!(
            sent.starts_with("hello\n\n--- user_prompt hook `cat` ---\n{")
                && sent.contains("\"event\":\"user_prompt\"")
                && sent.contains("\"prompt\":\"hello\"")
                && sent.contains("\"turn\":1")
                && sent.ends_with("}\n--- end of hook output ---"),
            "{sent}"
        );
        let before = h.messages().len();
        let blocked = h.turn(&cancel, "say nope").unwrap();
        assert!(
            matches!(&blocked, Turn::Blocked(r) if r == "user_prompt hook `if grep -q nope; then echo no nope please; exit 2; fi` kept the message back: no nope please"),
            "{blocked:?}"
        );
        assert_eq!(h.messages().len(), before, "nothing was sent");
        assert_eq!(p.requests().len(), 1);
        assert_eq!(h.rewind_turns().len(), 1, "a kept-back message is no turn");

        h.set_hooks(HookSet::default());
        h.turn(&cancel, "one").unwrap();
        h.turn(&cancel, "two").unwrap();
        assert_eq!(h.messages().len(), 11);
        h.set_hooks(HookSet {
            hooks: vec![hook(HookEvent::PreCompact, &[], "echo not now; exit 2")],
            ..HookSet::default()
        });
        assert_eq!(h.compact_now(&cancel), Some(false));
        assert_eq!(p.requests().len(), 5, "no summary was asked for");
        assert!(
            h.steps.iter().any(|s| s.kind == StepKind::Note
                && s.content
                    == "pre_compact hook `echo not now; exit 2` declined the compaction: not now"),
            "{:?}",
            h.steps.iter().map(|s| &s.content).collect::<Vec<_>>()
        );
        h.set_hooks(HookSet {
            hooks: vec![hook(HookEvent::PreCompact, &[], "cat")],
            ..HookSet::default()
        });
        assert_eq!(h.compact_now(&cancel), Some(true));
        assert_eq!(h.result().compactions, 1);
        let reqs = p.requests();
        let ask = &reqs[5].messages[1].content;
        assert!(
            ask.contains("The owner's pre_compact hook asks this of the summary; honour it:\n{")
                && ask.contains("\"trigger\":\"manual\"")
                && ask.contains("\"compacting\":2")
                && ask.contains("\"event\":\"pre_compact\""),
            "{ask}"
        );
        let hook_steps: Vec<String> = h
            .steps
            .iter()
            .filter(|s| s.kind == StepKind::Hook)
            .map(|s| s.command.clone())
            .collect();
        assert_eq!(
            hook_steps,
            [
                "user_prompt",
                "user_prompt",
                "user_prompt",
                "user_prompt",
                "pre_compact",
                "pre_compact"
            ],
            "{hook_steps:?}"
        );
    }

    #[test]
    fn typed_tools_read_grep_glob_edit_write_plan_in_one_turn() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(
            dir.path().join("src/lib.rs"),
            "fn main() {\n    println!(\"old\");\n}\n",
        )
        .unwrap();
        fs::write(dir.path().join("src/deep/more.rs"), "// more\n").unwrap();
        fs::write(dir.path().join("README.md"), "# hi\n").unwrap();
        let p = Arc::new(FakeProvider::new(vec![
            // Four independent calls in one reply: all run, all answered.
            resp(calls(&[
                (
                    "r1",
                    "read",
                    json!({"path":"src/lib.rs","start":"1","end":2}),
                ),
                ("g1", "grep", json!({"pattern":"println","path":"src"})),
                ("l1", "glob", json!({"pattern":"*.rs"})),
                (
                    "p1",
                    "plan",
                    json!({"items":[
                        {"text":"read the code","status":"done"},
                        {"text":"change the greeting","status":"in_progress"},
                        "add the test"
                    ]}),
                ),
            ])),
            resp(calls(&[
                (
                    "e1",
                    "edit",
                    json!({"path":"src/lib.rs","search":"println!(\"old\");","replace":"println!(\"new\");"}),
                ),
                (
                    "w1",
                    "write",
                    json!({"path":"src/deep/new/file.txt","content":"made\n"}),
                ),
                ("w2", "write", json!({"path":"README.md","content":"x"})),
                ("l2", "glob", json!({"pattern":"*.rs","path":"src/deep"})),
            ])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p.clone());
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED, "{got:?}");
        assert_eq!(got.steps, 8, "one step per tool call");
        assert_eq!(got.format_errors, 0);

        let replies: BTreeMap<String, String> = tool_replies(&h).into_iter().collect();
        assert_eq!(replies.len(), 8, "{replies:?}");
        assert!(
            replies["r1"].contains("     1\tfn main()"),
            "{}",
            replies["r1"]
        );
        assert!(
            replies["r1"].contains("[lines 1-2 of 3]"),
            "{}",
            replies["r1"]
        );
        assert!(replies["g1"].contains("src/lib.rs:2:"), "{}", replies["g1"]);
        assert!(
            replies["l1"].contains("src/deep/more.rs\n"),
            "{}",
            replies["l1"]
        );
        assert!(replies["l1"].contains("src/lib.rs\n"), "{}", replies["l1"]);
        assert!(!replies["l1"].contains(".git/"), "{}", replies["l1"]);
        assert!(!replies["l1"].contains("README"), "{}", replies["l1"]);
        assert!(
            replies["p1"].contains(
                "plan: 1/3 done\n- [x] read the code\n- [~] change the greeting\n- [ ] add the test"
            ),
            "{}",
            replies["p1"]
        );
        assert!(
            replies["e1"].contains("edited src/lib.rs"),
            "{}",
            replies["e1"]
        );
        assert!(
            replies["w1"].contains("wrote src/deep/new/file.txt (5 bytes)"),
            "{}",
            replies["w1"]
        );
        assert!(
            replies["w2"].contains("file exists; use edit")
                && replies["w2"].contains("<returncode>1</returncode>"),
            "{}",
            replies["w2"]
        );
        assert!(
            replies["l2"].contains("more.rs") && !replies["l2"].contains("lib.rs"),
            "glob under src/deep lists only that directory: {}",
            replies["l2"]
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(),
            "fn main() {\n    println!(\"new\");\n}\n"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("src/deep/new/file.txt")).unwrap(),
            "made\n"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("README.md")).unwrap(),
            "# hi\n"
        );

        let kinds: Vec<StepKind> = h
            .trajectory()
            .steps
            .iter()
            .map(|s| s.kind.clone())
            .collect();
        for k in [
            StepKind::Read,
            StepKind::Grep,
            StepKind::Glob,
            StepKind::Plan,
            StepKind::Edit,
            StepKind::Write,
        ] {
            assert!(kinds.contains(&k), "no {k} step in {kinds:?}");
        }
        let plan = h
            .trajectory()
            .steps
            .into_iter()
            .find(|s| s.kind == StepKind::Plan)
            .unwrap();
        assert!(plan.content.starts_with("plan: 1/3 done\n"), "{plan:?}");
        assert!(
            p.requests()[0]
                .tools
                .iter()
                .map(|t| t.name.as_str())
                .eq(NATIVE_TOOL_NAMES.iter().copied().chain([PROCESS_TOOL])),
            "every run offers the native tool set, plus process where background runs"
        );
    }

    #[test]
    fn unknown_tool_and_bad_arguments_count_as_format_errors() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[("x1", "fetch_url", json!({"url":"https://x"}))])),
            resp(calls(&[("r1", "read", json!({"path": 7}))])),
            resp(calls(&[("w1", "write", json!({"path":"a.txt"}))])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p);
        h.cfg.format_retries = 0;
        let got = h.run().unwrap();
        // Three unclean turns in a row end the run before the summary.
        assert_eq!(got.exit_status, EXIT_FORMAT_ERROR, "{got:?}");
        let replies = tool_replies(&h);
        assert!(
            replies[0].1.contains(
                "unknown tool fetch_url; available tools: bash, read, grep, glob, edit, write, plan"
            ),
            "{}",
            replies[0].1
        );
        assert!(replies[1].1.starts_with("read: "), "{}", replies[1].1);
        assert!(replies[2].1.contains("content"), "{}", replies[2].1);
        assert!(
            h.trajectory()
                .steps
                .iter()
                .filter(|s| s.kind == StepKind::Error)
                .count()
                >= 3
        );
    }

    #[test]
    fn glob_reports_no_match_and_caps_the_listing() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("many")).unwrap();
        for i in 0..(GLOB_RESULT_LIMIT + 5) {
            fs::write(dir.path().join(format!("many/f{i:04}.txt")), "x").unwrap();
        }
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[
                ("a", "glob", json!({"pattern":"*.nothing"})),
                ("b", "glob", json!({"pattern":"many/*.txt"})),
                ("c", "glob", json!({"pattern":"*.txt","path":"missing"})),
            ])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p);
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        let replies: BTreeMap<String, String> = tool_replies(&h).into_iter().collect();
        assert!(replies["a"].contains("no file under"), "{}", replies["a"]);
        assert!(replies["a"].contains("*.nothing"), "{}", replies["a"]);
        assert_eq!(
            replies["b"].matches("many/f").count(),
            GLOB_RESULT_LIMIT,
            "{}",
            &replies["b"][replies["b"].len().saturating_sub(300)..]
        );
        assert!(
            replies["b"].contains("5 more"),
            "{}",
            &replies["b"][replies["b"].len().saturating_sub(300)..]
        );
        assert!(
            !replies["c"].contains("<returncode>0</returncode>")
                && replies["c"].contains("missing"),
            "{}",
            replies["c"]
        );
    }

    #[test]
    fn render_plan_marks_each_status() {
        let items = vec![
            PlanItem {
                text: "one\nwith newline".into(),
                status: "done".into(),
            },
            PlanItem {
                text: "two".into(),
                status: "doing".into(),
            },
            PlanItem {
                text: "three".into(),
                status: "todo".into(),
            },
        ];
        assert_eq!(
            render_plan(&items),
            "plan: 1/3 done\n- [x] one with newline\n- [~] two\n- [ ] three\n"
        );
        assert_eq!(render_plan(&[]), "plan: 0/0 done\n");
    }

    #[test]
    fn native_call_from_args_is_lenient_about_shapes() {
        let read =
            NativeCall::from_args("read", r#"{"path":"a.rs","start":"3","end":"4"}"#).unwrap();
        assert_eq!(read.spec(), "a.rs:3-4");
        let plan = NativeCall::from_args(
            "plan",
            r#"{"items":["a",{"text":"b","status":"DONE"},{"text":"c","status":"wip"}]}"#,
        )
        .unwrap();
        let NativeCall::Plan(items) = plan else {
            panic!("not a plan");
        };
        assert_eq!(
            items.iter().map(|i| i.status.as_str()).collect::<Vec<_>>(),
            vec!["todo", "done", "doing"]
        );
        let edit = NativeCall::from_args(
            "edit",
            r#"{"path":"f","edits":[{"search":"a","replace":"b"},{"search":"c","replace":"d"}]}"#,
        )
        .unwrap();
        let NativeCall::Edit { blocks, .. } = edit else {
            panic!("not an edit");
        };
        assert_eq!(blocks.len(), 2);
        assert!(NativeCall::from_args("read", "not json").is_err());
        assert!(NativeCall::from_args("grep", r#"{"path":"x"}"#).is_err());
        assert!(NativeCall::from_args("write", r#"{"path":"x"}"#).is_err());
        assert!(NativeCall::from_args("plan", r#"{"items":"a"}"#).is_err());
    }

    struct FakeMcp {
        calls: Arc<Mutex<Vec<(String, Value)>>>,
        fail: bool,
    }

    impl McpBackend for FakeMcp {
        fn call_tool(&mut self, name: &str, arguments: Value) -> anyhow::Result<McpToolResult> {
            self.calls
                .lock()
                .unwrap()
                .push((name.to_string(), arguments.clone()));
            if self.fail {
                return Err(anyhow!("server went away"));
            }
            Ok(McpToolResult {
                is_error: name == "broken",
                text: format!("{name} saw {arguments}"),
            })
        }
    }

    fn tool_info(name: &str, description: &str) -> pulse_mcp::ToolInfo {
        pulse_mcp::ToolInfo {
            name: name.into(),
            description: description.into(),
            input_schema: json!({"type":"object","properties":{"q":{"type":"string"}}}),
        }
    }

    #[test]
    fn mcp_tools_are_offered_by_prefixed_name_and_called_through_the_backend() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[
                ("m1", "mcp__github__list_issues", json!({"q":"open"})),
                ("m2", "mcp__github__broken", json!({"q":"x"})),
                ("m3", "mcp__github__list_issues", json!("not an object")),
            ])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p.clone());
        h.add_mcp_server(McpServerTools {
            server: "github".into(),
            tools: vec![
                tool_info("list_issues", "List the open issues."),
                tool_info("broken", ""),
                pulse_mcp::ToolInfo {
                    name: "odd name!".into(),
                    description: "d".into(),
                    input_schema: Value::Null,
                },
            ],
            backend: Box::new(FakeMcp {
                calls: seen.clone(),
                fail: false,
            }),
        });
        h.note_at_start("MCP server github connected");
        let offered: Vec<String> = h.tools().into_iter().map(|t| t.name).collect();
        assert_eq!(
            offered,
            [
                NATIVE_TOOL_NAMES.to_vec(),
                vec![
                    PROCESS_TOOL,
                    "mcp__github__broken",
                    "mcp__github__list_issues",
                    "mcp__github__odd_name"
                ]
            ]
            .concat()
        );
        let odd = h
            .tools()
            .into_iter()
            .find(|t| t.name == "mcp__github__odd_name")
            .unwrap();
        assert_eq!(odd.parameters, json!({"type":"object","properties":{}}));
        let broken = h
            .tools()
            .into_iter()
            .find(|t| t.name == "mcp__github__broken")
            .unwrap();
        assert_eq!(broken.description, "broken tool of the github MCP server.");

        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED, "{got:?}");
        assert_eq!(got.steps, 2, "the malformed call is no step");
        let replies: BTreeMap<String, String> = tool_replies(&h).into_iter().collect();
        assert!(
            replies["m1"].contains("list_issues saw {\"q\":\"open\"}"),
            "{}",
            replies["m1"]
        );
        assert!(
            replies["m2"].contains("<returncode>1</returncode>"),
            "{}",
            replies["m2"]
        );
        assert!(replies["m3"].contains("JSON object"), "{}", replies["m3"]);
        assert_eq!(
            seen.lock()
                .unwrap()
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>(),
            vec!["list_issues", "broken"],
            "the malformed call never reaches the server"
        );
        let steps = h.trajectory().steps;
        assert_eq!(steps[0].kind, StepKind::Note);
        assert!(steps[0].content.contains("github connected"));
        let mcp_steps: Vec<_> = steps.iter().filter(|s| s.kind == StepKind::Mcp).collect();
        assert_eq!(mcp_steps.len(), 2);
        assert_eq!(mcp_steps[0].command, "mcp__github__list_issues");
        assert!(
            mcp_steps[0]
                .content
                .starts_with("mcp__github__list_issues {")
        );
        assert_eq!(
            p.requests()[0].tools.len(),
            NATIVE_TOOL_NAMES.len() + 1 + 3,
            "the model saw the process tool and the MCP tools"
        );
    }

    #[test]
    fn mcp_catalogue_past_the_limit_is_searched_then_loaded() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[(
                "s1",
                "mcp_search",
                json!({"query":"deploy the site"}),
            )])),
            resp(calls(&[
                ("c1", "mcp__ops__deploy_site", json!({"q":"now"})),
                ("c2", "mcp__ops__tool_0003", json!({})),
            ])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p.clone());
        let mut tools: Vec<pulse_mcp::ToolInfo> = (0..MCP_INLINE_TOOL_LIMIT)
            .map(|i| tool_info(&format!("tool_{i:04}"), "Something unrelated."))
            .collect();
        tools.push(tool_info("deploy_site", "Deploy the documentation site."));
        h.add_mcp_server(McpServerTools {
            server: "ops".into(),
            tools,
            backend: Box::new(FakeMcp {
                calls: seen.clone(),
                fail: false,
            }),
        });
        let offered: Vec<String> = h.tools().into_iter().map(|t| t.name).collect();
        assert_eq!(
            offered,
            [
                NATIVE_TOOL_NAMES.to_vec(),
                vec![PROCESS_TOOL, MCP_SEARCH_TOOL]
            ]
            .concat(),
            "past the limit only the search tool is offered"
        );
        let search = h
            .tools()
            .into_iter()
            .find(|t| t.name == MCP_SEARCH_TOOL)
            .unwrap();
        assert!(
            search.description.contains("ops (33 tools)"),
            "{}",
            search.description
        );

        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED, "{got:?}");
        let replies: BTreeMap<String, String> = tool_replies(&h).into_iter().collect();
        assert!(
            replies["s1"]
                .contains("<output>\nmcp__ops__deploy_site: Deploy the documentation site."),
            "{}",
            replies["s1"]
        );
        assert!(
            !replies["s1"].contains("tool_0003"),
            "unrelated tools stay out of the search result: {}",
            replies["s1"]
        );
        assert!(replies["s1"].contains("callable from your next reply"));
        assert!(
            replies["c1"].contains("deploy_site saw"),
            "{}",
            replies["c1"]
        );
        assert!(
            replies["c2"].contains("tool_0003 saw"),
            "a known tool is callable even before it was looked up: {}",
            replies["c2"]
        );
        let second: Vec<String> = p.requests()[1]
            .tools
            .iter()
            .map(|t| t.name.clone())
            .collect();
        assert!(
            second.contains(&"mcp__ops__deploy_site".to_string()),
            "the looked-up tool is advertised next: {second:?}"
        );
        assert!(!second.contains(&"mcp__ops__tool_0003".to_string()));
        assert!(second.contains(&MCP_SEARCH_TOOL.to_string()));
    }

    #[test]
    fn mcp_search_without_a_catalogue_is_an_unknown_tool() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[("s1", "mcp_search", json!({"query":"x"}))])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p);
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        let replies = tool_replies(&h);
        assert!(
            replies[0].1.contains("unknown tool mcp_search"),
            "{}",
            replies[0].1
        );
    }

    #[test]
    fn mcp_backend_failure_is_an_observation_not_a_crash() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[("m1", "mcp__gone__ping", json!({}))])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p);
        h.add_mcp_server(McpServerTools {
            server: "gone".into(),
            tools: vec![tool_info("ping", "Ping.")],
            backend: Box::new(FakeMcp {
                calls: Arc::new(Mutex::new(Vec::new())),
                fail: true,
            }),
        });
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED, "{got:?}");
        let replies = tool_replies(&h);
        assert!(
            replies[0].1.contains("server went away"),
            "{}",
            replies[0].1
        );
        assert!(
            replies[0].1.contains("<returncode>1</returncode>"),
            "{}",
            replies[0].1
        );
    }

    #[test]
    fn tool_name_slug_keeps_provider_safe_names() {
        assert_eq!(tool_name_slug("list-issues"), "list-issues");
        assert_eq!(tool_name_slug("odd name!"), "odd_name");
        assert_eq!(tool_name_slug("__x__"), "x");
        assert_eq!(tool_name_slug("###"), "tool");
    }

    #[test]
    fn load_instructions_reads_each_file_once_within_limits() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load_instructions(&dir.path().display().to_string(), ""),
            Instructions::default()
        );
        fs::create_dir_all(dir.path().join(".github")).unwrap();
        fs::write(dir.path().join("AGENTS.md"), "# Agents\nRun cargo test.\n").unwrap();
        // A CLAUDE.md that is a copy of AGENTS.md (a common symlink stand-in) is skipped.
        fs::write(
            dir.path().join("CLAUDE.md"),
            "# Agents\nRun cargo test.\n\n",
        )
        .unwrap();
        fs::write(dir.path().join("GEMINI.md"), "   \n").unwrap();
        let big = "x".repeat(INSTRUCTION_FILE_LIMIT + 100);
        fs::write(dir.path().join(".github/copilot-instructions.md"), &big).unwrap();
        let Instructions { text, files, .. } =
            load_instructions(&dir.path().display().to_string(), "");
        assert_eq!(files, vec!["AGENTS.md", ".github/copilot-instructions.md"]);
        assert!(
            text.starts_with("--- AGENTS.md ---\n# Agents\nRun cargo test.\n"),
            "{text}"
        );
        assert!(text.contains("--- .github/copilot-instructions.md ---\n"));
        assert!(
            text.contains("[... .github/copilot-instructions.md continues for 100 more bytes"),
            "{}",
            &text[text.len() - 200..]
        );
        assert!(text.len() < INSTRUCTION_FILE_LIMIT + 600, "{}", text.len());
    }

    #[test]
    fn load_instructions_caps_the_total() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".github")).unwrap();
        fs::write(
            dir.path().join("AGENTS.md"),
            "a".repeat(INSTRUCTION_FILE_LIMIT),
        )
        .unwrap();
        fs::write(
            dir.path().join("CLAUDE.md"),
            "b".repeat(INSTRUCTION_FILE_LIMIT),
        )
        .unwrap();
        fs::write(dir.path().join("GEMINI.md"), "c".repeat(100)).unwrap();
        let Instructions { text, files, .. } =
            load_instructions(&dir.path().display().to_string(), "");
        assert_eq!(files, vec!["AGENTS.md", "CLAUDE.md", "GEMINI.md"]);
        // The second 16 KB file would overflow the total: its section is
        // the marker alone; the small third file still fits.
        assert!(
            text.contains("--- CLAUDE.md ---\n[omitted: the instructions already fill"),
            "{}",
            &text[text.len() - 300..]
        );
        assert!(!text.contains("bbb"));
        assert!(text.contains("--- GEMINI.md ---\nccc"));
        assert!(text.len() <= INSTRUCTIONS_LIMIT + 200);
    }

    #[test]
    fn render_prompt_places_instructions_and_never_scans_them_for_keys() {
        let data = |instructions: &str| PromptData {
            cwd: "/c".into(),
            provider: "p".into(),
            model: "m".into(),
            step_timeout: "5s".into(),
            output_limit: 10,
            max_steps: 3,
            info: BTreeMap::new(),
            instructions: instructions.into(),
            memory: String::new(),
            teach: false,
        };
        // The default template carries the key: the paragraph lands there.
        let out = render_prompt(
            DEFAULT_PROMPT_TEMPLATE,
            data("--- AGENTS.md ---\nUse {{.Nope}} literally.\n"),
        )
        .unwrap();
        assert!(out.contains("Repository instructions follow"), "{out}");
        assert!(out.contains("Use {{.Nope}} literally."), "{out}");
        assert!(!out.contains("{{.Instructions}}"));
        let tail = out.rsplit("Repository instructions follow").next().unwrap();
        assert!(
            tail.contains("The task follows"),
            "the task line stays last: {tail}"
        );
        // Without instructions the key leaves no trace.
        let out = render_prompt(DEFAULT_PROMPT_TEMPLATE, data("")).unwrap();
        assert!(!out.contains("Repository instructions"), "{out}");
        assert!(!out.contains("{{.Instructions}}"));
        assert!(
            !out.contains("\n\n\n"),
            "no empty paragraph where the key stood: {out:?}"
        );
        // A custom template without the key gets the paragraph appended.
        let out = render_prompt("Hello {{.Cwd}}", data("be nice")).unwrap();
        assert_eq!(
            out,
            "Hello /c\nRepository instructions follow; they apply in addition to the task, and the rules above still win where they differ.\nbe nice"
        );
        assert_eq!(
            render_prompt("Hello {{.Cwd}}", data("")).unwrap(),
            "Hello /c"
        );
        // An unknown key in the template itself is still an error.
        assert!(render_prompt("Hello {{.Nope}}", data("x")).is_err());
    }

    #[test]
    fn render_prompt_places_memory_after_instructions_and_appends_it_when_the_key_is_missing() {
        let data = |instructions: &str, memory: &str| PromptData {
            cwd: "/c".into(),
            provider: "p".into(),
            model: "m".into(),
            step_timeout: "5s".into(),
            output_limit: 10,
            max_steps: 3,
            info: BTreeMap::new(),
            instructions: instructions.into(),
            memory: memory.into(),
            teach: false,
        };
        let out = render_prompt(
            DEFAULT_PROMPT_TEMPLATE,
            data(
                "--- AGENTS.md ---\nrules\n",
                "--- project memory (~/.pulse/memory/x.md) ---\n- [2026-01-02] use {{.Keys}} freely\n",
            ),
        )
        .unwrap();
        let instructions_at = out.find("Repository instructions follow").unwrap();
        let memory_at = out.find("What you remembered in earlier runs").unwrap();
        let task_at = out.find("The task follows").unwrap();
        assert!(instructions_at < memory_at && memory_at < task_at, "{out}");
        assert!(out.contains("use {{.Keys}} freely"), "{out}");
        assert!(!out.contains("{{.Memory}}"));
        // No memory: the key leaves no trace.
        let out = render_prompt(DEFAULT_PROMPT_TEMPLATE, data("", "")).unwrap();
        assert!(!out.contains("remembered"), "{out}");
        assert!(!out.contains("\n\n\n"), "{out:?}");
        // A custom template without either key gets both appended, in order.
        let out = render_prompt("Hello {{.Cwd}}", data("be nice", "- [2026-01-02] tabs")).unwrap();
        assert!(
            out.starts_with("Hello /c\nRepository instructions follow"),
            "{out}"
        );
        assert!(
            out.contains("be nice\nWhat you remembered in earlier runs"),
            "{out}"
        );
        assert!(out.ends_with("- [2026-01-02] tabs"), "{out:?}");
        assert_eq!(
            render_prompt("Hello {{.Cwd}}", data("", "")).unwrap(),
            "Hello /c"
        );
    }

    #[test]
    fn render_prompt_places_the_teacher_paragraph_only_when_teach_is_on() {
        let data = |teach: bool| PromptData {
            cwd: "/c".into(),
            provider: "p".into(),
            model: "m".into(),
            step_timeout: "5s".into(),
            output_limit: 10,
            max_steps: 3,
            info: BTreeMap::new(),
            instructions: String::new(),
            memory: String::new(),
            teach,
        };
        // Off: no trace of it, in either template.
        for tmpl in [
            DEFAULT_PROMPT_TEMPLATE,
            CHAT_PROMPT_TEMPLATE,
            "Hello {{.Cwd}}",
        ] {
            let out = render_prompt(tmpl, data(false)).unwrap();
            assert!(
                !out.contains("Teacher mode") && !out.contains("{{.Teach}}"),
                "{out}"
            );
        }
        // A job: the paragraph asks for LESSON: lines, before the task.
        let out = render_prompt(DEFAULT_PROMPT_TEMPLATE, data(true)).unwrap();
        let teach_at = out.find("Teacher mode is on").unwrap();
        let task_at = out.find("The task follows").unwrap();
        assert!(teach_at < task_at, "{out}");
        assert!(out.contains("LESSON: lines"), "{out}");
        assert!(!out.contains("{{.Teach}}"));
        // A session: the paragraph speaks to the user and asks for no report lines.
        let out = render_prompt(CHAT_PROMPT_TEMPLATE, data(true)).unwrap();
        assert!(
            out.contains("the user is learning from this session"),
            "{out}"
        );
        assert!(!out.contains("LESSON:"), "{out}");
        // A custom template without the key gets the paragraph appended.
        let out = render_prompt("Hello {{.Cwd}}", data(true)).unwrap();
        assert!(out.starts_with("Hello /c\nTeacher mode is on"), "{out}");
        assert_eq!(teach_paragraph(false, true), "");
    }

    #[test]
    fn native_actions_reject_path_escape_and_denied_path() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".pulse/private")).unwrap();
        fs::write(dir.path().join(".pulse/private/token"), "secret").unwrap();
        let mut cfg = base_config();
        cfg.cwd = dir.path().display().to_string();
        cfg.max_steps = 10;
        let p = Arc::new(FakeProvider::new(vec![
            resp(msg("```read\n../outside\n```")),
            resp(msg("```read\n.pulse/private/token\n```")),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = new(
            Box::new(SharedFakeProvider(p)),
            Box::new(crate::localenv::new_local_env(
                dir.path().display().to_string(),
                None,
            )),
            cfg,
            Vec::new(),
        );
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        let joined = h
            .messages()
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("must be relative"));
        assert!(joined.contains("refused by policy"));
    }

    #[test]
    fn native_path_deny_reuses_shell_deny_rule() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".pulse/private")).unwrap();
        fs::write(dir.path().join(".pulse/private/token"), "secret").unwrap();
        let mut cfg = base_config();
        cfg.cwd = dir.path().display().to_string();
        let mut h = new(
            Box::new(SharedFakeProvider(Arc::new(FakeProvider::new(Vec::new())))),
            Box::new(crate::localenv::new_local_env(
                dir.path().display().to_string(),
                None,
            )),
            cfg,
            Vec::new(),
        );
        h.compile_deny().unwrap();
        assert!(h.denied("cat .pulse/private/token").is_some());
        assert!(
            h.fs()
                .resolve_action_path(".pulse/private/token", false)
                .is_err()
        );
    }

    #[test]
    fn limits() {
        let _serial = crate::test_helpers::serial();
        let mut cfg = base_config();
        cfg.max_steps = 1;
        let p = Arc::new(FakeProvider::new(vec![resp(tool("echo hi"))]));
        let (_, got) = run_agent(p, Arc::new(Mutex::new(FakeEnv::default())), cfg).unwrap();
        assert_eq!(got.exit_status, EXIT_STEP_LIMIT);
        assert_eq!(got.calls, 1);

        let mut cfg = base_config();
        cfg.wall_time = Duration::from_millis(50);
        cfg.step_timeout = Duration::from_secs(1);
        let dir = tempfile::tempdir().unwrap();
        let mut h = new(
            Box::new(SharedFakeProvider(Arc::new(FakeProvider::new(vec![resp(
                msg("```bash\nsleep 5\n```"),
            )])))),
            Box::new(crate::localenv::new_local_env(
                dir.path().display().to_string(),
                None,
            )),
            cfg,
            Vec::new(),
        );
        let got = h.run().unwrap();
        assert_eq!(
            got.exit_status, EXIT_TIME_LIMIT,
            "exit = {}, want {}",
            got.exit_status, EXIT_TIME_LIMIT
        );
    }

    #[test]
    fn cost_limit_stops_run() {
        let _serial = crate::test_helpers::serial();
        let p = Arc::new(FakeProvider::new(vec![
            pulse_provider::Response {
                message: Some(tool("echo one")),
                usage: Usage {
                    cost: 0.6,
                    ..Usage::default()
                },
                ..pulse_provider::Response::default()
            },
            pulse_provider::Response {
                message: Some(tool("echo two")),
                usage: Usage {
                    cost: 0.6,
                    ..Usage::default()
                },
                ..pulse_provider::Response::default()
            },
        ]));
        let mut cfg = base_config();
        cfg.max_cost = 1.0;
        let env = Arc::new(Mutex::new(FakeEnv {
            observations: vec![
                Observation {
                    output: "one\n".into(),
                    exit_code: 0,
                    ..Observation::default()
                },
                Observation {
                    output: "two\n".into(),
                    exit_code: 0,
                    ..Observation::default()
                },
            ],
            ..FakeEnv::default()
        }));
        let (_, got) = run_agent(p, env, cfg).unwrap();
        assert_eq!(got.exit_status, EXIT_COST_LIMIT);
        assert_eq!(got.calls, 2);
        assert!(got.error.contains("cost budget"));
    }

    #[test]
    fn budget_notice_not_sent_below_ten_steps() {
        let _serial = crate::test_helpers::serial();
        let p = Arc::new(FakeProvider::new(
            (0..9).map(|_| resp(tool("true"))).collect(),
        ));
        let mut cfg = base_config();
        cfg.max_steps = 9;
        let (_, got) = run_agent(
            p.clone(),
            Arc::new(Mutex::new(FakeEnv {
                default_output: "ok\n".into(),
                ..FakeEnv::default()
            })),
            cfg,
        )
        .unwrap();
        assert_eq!(got.exit_status, EXIT_STEP_LIMIT);
        assert!(p.requests().iter().all(|r| {
            !r.messages
                .iter()
                .any(|m| m.content.starts_with("Budget notice:"))
        }));
    }

    #[test]
    fn budget_notice_appears_once_and_can_submit() {
        let _serial = crate::test_helpers::serial();
        let p = Arc::new(BudgetAwareProvider {
            requests: Mutex::new(Vec::new()),
        });
        let mut cfg = base_config();
        cfg.max_steps = 20;
        let notes: Arc<Mutex<Vec<Step>>> = Arc::new(Mutex::new(Vec::new()));
        let notes_sink = notes.clone();
        let mut h = new(
            Box::new(SharedBudgetProvider(p.clone())),
            Box::new(Arc::new(Mutex::new(FakeEnv {
                default_output: "ok\n".into(),
                ..FakeEnv::default()
            }))),
            cfg.clone(),
            vec![Box::new(SinkFunc(move |s| {
                if s.kind == StepKind::Note && s.content.starts_with("Budget notice:") {
                    notes_sink.lock().unwrap().push(s);
                }
            }))],
        );
        let got = h.run().unwrap();
        assert!(
            got.exit_status == EXIT_SUBMITTED
                && got.calls == 19
                && got.submission.contains("SUMMARY: done after notice"),
            "unexpected result: {got:?}"
        );
        assert_eq!(
            notes.lock().unwrap().len(),
            1,
            "budget notes = {}, want 1",
            notes.lock().unwrap().len()
        );
        let reqs = p.requests();
        let notice_req = cfg.max_steps - 5.min(cfg.max_steps / 10);
        assert!(
            reqs.len() == got.calls as usize
                && request_has_budget_notice(&reqs[notice_req as usize]),
            "notice missing from request {notice_req} of {}",
            reqs.len()
        );
        for (i, req) in reqs.iter().enumerate() {
            let count = req
                .messages
                .iter()
                .filter(|m| m.role == Role::User && m.content.starts_with("Budget notice:"))
                .count();
            if i < notice_req as usize {
                assert_eq!(count, 0, "request {i} has early budget notice");
            }
            if i >= notice_req as usize {
                assert_eq!(count, 1, "request {i} budget notices = {count}, want 1");
            }
        }
    }

    #[test]
    fn prune_note_emitted_once_per_level() {
        let _serial = crate::test_helpers::serial();
        let p = Arc::new(FakeProvider::new(vec![
            resp(tool("long")),
            resp(tool("long")),
            resp(tool("long")),
            resp(tool("long")),
            resp(msg("SUMMARY: done")),
        ]));
        let mut cfg = base_config();
        cfg.keep_observations = 1;
        let env = Arc::new(Mutex::new(FakeEnv {
            default_output: "x\n".repeat(20),
            ..FakeEnv::default()
        }));
        let notes: Arc<Mutex<Vec<Step>>> = Arc::new(Mutex::new(Vec::new()));
        let notes_sink = notes.clone();
        let mut h = new(
            Box::new(SharedFakeProvider(p)),
            Box::new(env),
            cfg,
            vec![Box::new(SinkFunc(move |s| {
                if s.kind == StepKind::Note {
                    notes_sink.lock().unwrap().push(s);
                }
            }))],
        );
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        let notes = notes.lock().unwrap();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].content.contains("level 1, keep 1"));
    }

    #[test]
    fn long_run_stays_under_window() {
        let _serial = crate::test_helpers::serial();
        let mut responses = (0..80)
            .map(|_| {
                let mut r = resp(tool("yes | head -c 20000"));
                r.usage.prompt_tokens = 1;
                r
            })
            .collect::<Vec<_>>();
        responses.push(resp(msg("SUMMARY: done")));
        let p = Arc::new(FakeProvider::new(responses));
        let mut cfg = base_config();
        cfg.max_steps = 100;
        cfg.context_window = 128000;
        cfg.output_limit = 20000;
        let notes: Arc<Mutex<Vec<Step>>> = Arc::new(Mutex::new(Vec::new()));
        let notes_sink = notes.clone();
        let mut h = new(
            Box::new(SharedFakeProvider(p.clone())),
            Box::new(Arc::new(Mutex::new(FakeEnv {
                default_output: "x\n".repeat(10000),
                ..FakeEnv::default()
            }))),
            cfg.clone(),
            vec![Box::new(SinkFunc(move |s| {
                if s.kind == StepKind::Note {
                    notes_sink.lock().unwrap().push(s);
                }
            }))],
        );
        let got = h.run().unwrap();
        assert_eq!(
            got.exit_status, EXIT_SUBMITTED,
            "exit = {}",
            got.exit_status
        );
        let limit = (0.8 * f64::from(cfg.context_window)) as usize;
        for (i, req) in p.requests().iter().enumerate() {
            if i < 6 {
                continue;
            }
            let chars = request_chars_for_test(req);
            if ((chars as f64) * 0.25) as usize > limit {
                panic!(
                    "request {i} has {chars} chars (~{} tokens), over limit {limit}",
                    ((chars as f64) * 0.25) as usize
                );
            }
        }
        assert!(!notes.lock().unwrap().is_empty(), "no prune note emitted");
    }

    #[test]
    fn deny_refuses_without_executing() {
        let _serial = crate::test_helpers::serial();
        let p = Arc::new(FakeProvider::new(vec![
            resp(tool("git push origin main")),
            resp(msg("SUMMARY: done")),
        ]));
        let env = Arc::new(Mutex::new(FakeEnv::default()));
        let (h, got) = run_agent(p, env.clone(), base_config()).unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        assert!(env.lock().unwrap().commands.is_empty());
        assert!(h.messages()[3].content.contains("refused by policy"));
    }

    #[test]
    fn credential_rule_covers_every_pulse_home_but_not_the_macos_private_root() {
        let rule = Regex::new(DEFAULT_DENY[3]).unwrap();
        for denied in [
            "cat ~/.pulse/private/providers/github",
            "ls ~/.pulse/private",
            "cat /Users/me/.pulse/private/providers/github",
            "cat \"$PULSE_HOME/private/providers/github\"",
            "cat ${PULSE_HOME}/private/heldout/h1.json",
            "ls /Users/me/.pulse/fleet/x/homes/2/private/heldout",
            "cat /opt/pulse-home/private/providers/github",
            "cat ~/.ssh/id_ed25519",
            "cat .netrc",
            "ls ~/.copilot",
            "cat ~/.config/gh/hosts.yml",
        ] {
            assert!(rule.is_match(denied), "should refuse: {denied}");
        }
        for allowed in [
            "cd /private/tmp/mh/clone && git tag -l && git log --oneline -3",
            "ls /private/var/folders/5h/T/pulse-runner-f2",
            "cat storage/private/notes.txt",
            "grep -rn private src/",
        ] {
            assert!(!rule.is_match(allowed), "should run: {allowed}");
        }
    }

    #[test]
    fn local_env_timeout_kills_process_group() {
        let _serial = crate::test_helpers::serial();
        let start = Instant::now();
        let dir = tempfile::tempdir().unwrap();
        let obs = crate::localenv::new_local_env(dir.path().display().to_string(), None).execute(
            &CancelToken::new(),
            "sleep 5",
            Duration::from_millis(200),
        );
        assert!(obs.timed_out && obs.exit_code == -1, "obs = {obs:?}");
        assert!(
            start.elapsed() <= Duration::from_secs(2),
            "timeout took too long: {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn truncation_keeps_head_tail() {
        let _serial = crate::test_helpers::serial();
        let out = format_observation(
            &Observation {
                output: "abcdefghijklmnopqrstuvwxyz".into(),
                exit_code: 0,
                ..Observation::default()
            },
            10,
        );
        assert!(out.contains("abcde"));
        assert!(out.contains("vwxyz"));
        assert!(out.contains("characters omitted"));
    }

    #[test]
    fn provider_errors() {
        let _serial = crate::test_helpers::serial();
        for (err, want) in [
            (
                anyhow!(pulse_provider::err_not_logged_in()),
                EXIT_NOT_LOGGED_IN,
            ),
            (
                anyhow!("provider failed with   spaces"),
                EXIT_PROVIDER_ERROR,
            ),
        ] {
            let p = Arc::new(FakeProvider::with_err(err));
            let (_, got) =
                run_agent(p, Arc::new(Mutex::new(FakeEnv::default())), base_config()).unwrap();
            assert_eq!(got.exit_status, want);
            assert!(!got.error.is_empty());
        }
    }

    /// Streams part of a reply then breaks `failures` times, then answers:
    /// a command first, then the summary.
    struct BrokenStream {
        failures: Mutex<u32>,
        calls: Mutex<u32>,
        answered: Mutex<u32>,
    }

    impl BrokenStream {
        fn breaking(failures: u32) -> Arc<Self> {
            Arc::new(Self {
                failures: Mutex::new(failures),
                calls: Mutex::new(0),
                answered: Mutex::new(0),
            })
        }
    }

    struct SharedBrokenStream(Arc<BrokenStream>);

    impl Provider for SharedBrokenStream {
        fn name(&self) -> &str {
            "broken"
        }
        fn label(&self) -> &str {
            "Broken"
        }
        fn chat(
            &self,
            _cancel: &CancelToken,
            _req: Request,
        ) -> anyhow::Result<pulse_provider::Response> {
            Err(anyhow!("the test streams"))
        }
        fn chat_stream(
            &self,
            _cancel: &CancelToken,
            _req: Request,
            on_text: &mut dyn FnMut(&str),
        ) -> anyhow::Result<pulse_provider::Response> {
            *self.0.calls.lock().unwrap() += 1;
            let mut failures = self.0.failures.lock().unwrap();
            if *failures > 0 {
                *failures -= 1;
                on_text("SUMM");
                return Err(pulse_provider::ProviderError::new(
                    "broken",
                    0,
                    "stream_interrupted",
                    "the reply stopped part way: error decoding response body",
                    false,
                )
                .into());
            }
            let mut answered = self.0.answered.lock().unwrap();
            *answered += 1;
            if *answered == 1 {
                return Ok(resp(tool("true")));
            }
            on_text("SUMMARY: done");
            Ok(resp(msg("SUMMARY: done")))
        }
        fn models(&self, _cancel: &CancelToken) -> anyhow::Result<Vec<Model>> {
            Ok(Vec::new())
        }
        fn status(&self, _cancel: &CancelToken) -> anyhow::Result<Login> {
            Ok(Login::default())
        }
    }

    /// Records what a stream sink hears: text pieces and `|` for a restart.
    struct RecordingStream(Arc<Mutex<String>>);

    impl StreamSink for RecordingStream {
        fn text(&mut self, text: &str) {
            self.0.lock().unwrap().push_str(text);
        }
        fn restart(&mut self) {
            self.0.lock().unwrap().push('|');
        }
    }

    #[test]
    fn a_reply_whose_stream_breaks_is_asked_for_again() {
        let _serial = crate::test_helpers::serial();
        // Two breaks fit the allowance: the first reply arrives on the
        // third call, the sink was told to start over before each retry,
        // and a Note step says so; the summary follows.
        let p = BrokenStream::breaking(STREAM_RETRIES);
        let seen: Arc<Mutex<Vec<Step>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_steps = seen.clone();
        let mut h = new(
            Box::new(SharedBrokenStream(p.clone())),
            Box::new(Arc::new(Mutex::new(FakeEnv::default()))),
            base_config(),
            vec![Box::new(SinkFunc(move |s: Step| {
                sink_steps.lock().unwrap().push(s)
            }))],
        );
        let heard = Arc::new(Mutex::new(String::new()));
        h.set_stream(Box::new(RecordingStream(heard.clone())));
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED, "{}", got.error);
        assert_eq!(*p.calls.lock().unwrap(), STREAM_RETRIES + 2);
        assert_eq!(got.calls, STREAM_RETRIES as i32 + 2);
        assert_eq!(heard.lock().unwrap().as_str(), "SUMM|SUMM|SUMMARY: done");
        let notes: Vec<String> = seen
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.kind == StepKind::Note)
            .map(|s| s.content.clone())
            .collect();
        assert_eq!(notes.len(), STREAM_RETRIES as usize, "{notes:?}");
        assert!(notes[0].contains("asking again (1 of 2)"), "{notes:?}");
        assert!(
            notes[0].contains("error decoding response body"),
            "{notes:?}"
        );

        // One break more than the allowance stops the turn as before.
        let p = BrokenStream::breaking(STREAM_RETRIES + 1);
        let mut h = new(
            Box::new(SharedBrokenStream(p.clone())),
            Box::new(Arc::new(Mutex::new(FakeEnv::default()))),
            base_config(),
            Vec::new(),
        );
        h.set_stream(Box::new(RecordingStream(Arc::new(Mutex::new(
            String::new(),
        )))));
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_PROVIDER_ERROR);
        assert!(got.error.contains("stopped part way"), "{}", got.error);
        assert_eq!(*p.calls.lock().unwrap(), STREAM_RETRIES + 1);
    }

    #[test]
    fn cancellation_reports_owner_reason() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let cancel_file = dir.path().join("cancel");
        fs::write(&cancel_file, "stop").unwrap();
        let mut cfg = base_config();
        cfg.cancel_file = cancel_file.display().to_string();
        let p = Arc::new(FakeProvider::new(Vec::new()));
        let (_, got) = run_agent(p, Arc::new(Mutex::new(FakeEnv::default())), cfg).unwrap();
        assert_eq!(got.exit_status, EXIT_CANCELLED);
        assert_eq!(got.error, ERR_OWNER_CANCELLED);

        let cancel = CancelToken::new();
        cancel.cancel();
        let mut cfg = base_config();
        cfg.cancel_reason = ERR_OWNER_CANCELLED.into();
        let mut h = new(
            Box::new(SharedFakeProvider(Arc::new(FakeProvider::new(Vec::new())))),
            Box::new(Arc::new(Mutex::new(FakeEnv::default()))),
            cfg,
            Vec::new(),
        );
        let got = h.run_with_cancel(&cancel).unwrap();
        assert_eq!(got.exit_status, EXIT_CANCELLED);
        assert_eq!(got.error, ERR_OWNER_CANCELLED);
    }

    #[test]
    fn a_hold_ends_the_run_held_not_cancelled() {
        let _serial = crate::test_helpers::serial();
        // The watcher raises the hold and cancels: the next step boundary
        // reads `held`, the trajectory keeps every message so far.
        let p = Arc::new(FakeProvider::new(vec![
            resp(tool("echo one")),
            resp(msg("SUMMARY: never reached")),
        ]));
        let cancel = CancelToken::new();
        let mut h = new(
            Box::new(SharedFakeProvider(p.clone())),
            Box::new(Arc::new(Mutex::new(FakeEnv::default()))),
            base_config(),
            Vec::new(),
        );
        let hold = h.hold_flag();
        // Raised before the run: it ends at the first boundary.
        hold.store(true, Ordering::SeqCst);
        cancel.cancel();
        let got = h.run_with_cancel(&cancel).unwrap();
        assert_eq!(got.exit_status, EXIT_HELD);
        assert_eq!(got.error, HELD_REASON);
        assert_eq!(p.requests().len(), 0, "no model call after the hold");
        let traj = h.trajectory();
        assert_eq!(traj.info.exit_status, EXIT_HELD);
        assert!(
            traj.messages
                .iter()
                .any(|m| matches!(m.role, Role::User) && m.content == "do it"),
            "the task is in the carried-over conversation: {:?}",
            traj.messages
        );
        // Without the hold the same cancel reads `cancelled`.
        let cancel = CancelToken::new();
        cancel.cancel();
        let mut h = new(
            Box::new(SharedFakeProvider(Arc::new(FakeProvider::new(Vec::new())))),
            Box::new(Arc::new(Mutex::new(FakeEnv::default()))),
            base_config(),
            Vec::new(),
        );
        let got = h.run_with_cancel(&cancel).unwrap();
        assert_eq!(got.exit_status, EXIT_CANCELLED);
    }

    #[test]
    fn resume_run_carries_the_task_and_opens_with_the_continuation() {
        let _serial = crate::test_helpers::serial();
        let plain = |role: Role, content: &str| Message {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: String::new(),
            name: String::new(),
            ..Default::default()
        };
        // The carried conversation ends in a tool round the hold cut:
        // the assistant's call has no result, so it goes.
        let carried = vec![
            plain(Role::System, "an old prompt"),
            plain(Role::User, "fix the flaky test"),
            tool("cargo test"),
            Message {
                role: Role::Tool,
                content: "ok".into(),
                tool_calls: Vec::new(),
                tool_call_id: "call-1".into(),
                name: "bash".into(),
                ..Default::default()
            },
            plain(Role::Assistant, "I will look at the test file"),
            tool("cat tests/flaky.rs"),
        ];
        let p = Arc::new(FakeProvider::new(vec![resp(msg("SUMMARY: done"))]));
        let mut cfg = base_config();
        cfg.task = "carry on where you stopped".into();
        let mut h = new(
            Box::new(SharedFakeProvider(p.clone())),
            Box::new(Arc::new(Mutex::new(FakeEnv::default()))),
            cfg,
            Vec::new(),
        );
        h.resume_run(carried, "carry on where you stopped").unwrap();
        assert_eq!(
            h.cfg.task, "fix the flaky test",
            "the task is the earlier session's first message"
        );
        let got = h.run().unwrap();
        assert_eq!(
            got.exit_status,
            EXIT_SUBMITTED,
            "{} / {:?}",
            got.error,
            p.requests()
                .iter()
                .map(|r| r
                    .messages
                    .iter()
                    .map(|m| format!("{:?}:{}", m.role, one_line(&m.content, 60)))
                    .collect::<Vec<_>>())
                .collect::<Vec<_>>()
        );
        let req = p.requests().remove(0);
        let roles: Vec<Role> = req.messages.iter().map(|m| m.role.clone()).collect();
        assert_eq!(
            roles,
            vec![
                Role::System,
                Role::User,
                Role::Assistant,
                Role::Tool,
                Role::Assistant,
                Role::User,
                Role::User
            ],
            "the dangling call is trimmed, the old prompt is not carried: {:?}",
            req.messages
        );
        assert_eq!(req.messages[1].content, "fix the flaky test");
        assert_eq!(req.messages[4].content, "I will look at the test file");
        assert!(
            req.messages[5]
                .content
                .contains("resumed an earlier session: 4 messages"),
            "{}",
            req.messages[5].content
        );
        assert_eq!(
            req.messages[6].content, "carry on where you stopped",
            "the run opens with the continuation, not the task again"
        );
        // Guards.
        let mut h = new(
            Box::new(SharedFakeProvider(Arc::new(FakeProvider::new(Vec::new())))),
            Box::new(Arc::new(Mutex::new(FakeEnv::default()))),
            base_config(),
            Vec::new(),
        );
        assert!(
            h.resume_run(vec![plain(Role::Assistant, "no task here")], "go")
                .is_err()
        );
        assert!(h.resume_run(vec![plain(Role::User, "task")], " ").is_err());
    }

    #[test]
    fn trim_dangling_tool_calls_cuts_only_unanswered_rounds() {
        let plain = |role: Role, content: &str| Message {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: String::new(),
            name: String::new(),
            ..Default::default()
        };
        let result = |id: &str| Message {
            role: Role::Tool,
            content: "ok".into(),
            tool_calls: Vec::new(),
            tool_call_id: id.into(),
            name: "bash".into(),
            ..Default::default()
        };
        let two_calls = Message {
            tool_calls: vec![
                ToolCall {
                    id: "a".into(),
                    name: "bash".into(),
                    arguments: "{}".into(),
                },
                ToolCall {
                    id: "b".into(),
                    name: "bash".into(),
                    arguments: "{}".into(),
                },
            ],
            ..tool("x")
        };
        // Complete: unchanged.
        let complete = vec![
            plain(Role::User, "t"),
            two_calls.clone(),
            result("a"),
            result("b"),
            plain(Role::Assistant, "done"),
        ];
        assert_eq!(trim_dangling_tool_calls(complete.clone()), complete);
        // One of two results missing: the round goes, with what followed.
        let partial = vec![
            plain(Role::User, "t"),
            plain(Role::Assistant, "first"),
            two_calls.clone(),
            result("a"),
        ];
        assert_eq!(
            trim_dangling_tool_calls(partial),
            vec![plain(Role::User, "t"), plain(Role::Assistant, "first")]
        );
        // Two dangling rounds in a row (a crash mid-batch): both go.
        let twice = vec![plain(Role::User, "t"), tool("one"), two_calls];
        assert_eq!(
            trim_dangling_tool_calls(twice),
            vec![plain(Role::User, "t")]
        );
        assert!(trim_dangling_tool_calls(Vec::new()).is_empty());
    }

    #[test]
    fn provider_call_cancellation_reports_cancel_reason() {
        let _serial = crate::test_helpers::serial();
        for (name, owner, want) in [
            ("plain", false, "interrupted: context canceled"),
            ("owner", true, ERR_OWNER_CANCELLED),
        ] {
            let p = Arc::new(BlockingProvider {
                started: (Mutex::new(false), Condvar::new()),
            });
            let cancel = CancelToken::new();
            let mut cfg = base_config();
            if owner {
                cfg.cancel_reason = ERR_OWNER_CANCELLED.into();
            }
            let token = cancel.clone();
            let provider = p.clone();
            let done = std::thread::spawn(move || {
                let mut h = new(
                    Box::new(SharedBlockingProvider(provider)),
                    Box::new(Arc::new(Mutex::new(FakeEnv::default()))),
                    cfg,
                    Vec::new(),
                );
                h.run_with_cancel(&token)
            });
            let (lock, cv) = &p.started;
            let started = cv
                .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(1), |started| {
                    !*started
                })
                .unwrap();
            assert!(*started.0, "provider call did not start");
            cancel.cancel();
            let got = done.join().unwrap().unwrap();
            assert!(
                got.exit_status == EXIT_CANCELLED && got.error == want,
                "{name} cancel result = {got:?}, want {want:?}"
            );
        }
    }

    #[test]
    fn write_trajectory() {
        let _serial = crate::test_helpers::serial();
        let p = Arc::new(FakeProvider::new(vec![
            resp(msg("SUMMARY: done")),
            resp(msg("SUMMARY: done")),
        ]));
        let (h, got) =
            run_agent(p, Arc::new(Mutex::new(FakeEnv::default())), base_config()).unwrap();
        if got.exit_status != EXIT_SUBMITTED {
            panic!("{}", got.exit_status);
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/trajectory.json");
        if let Err(err) = super::write_trajectory(path.to_str().unwrap(), &h.trajectory()) {
            panic!("{err}");
        }
        let data = fs::read_to_string(&path).unwrap();
        if !data.contains("\"trajectory_format\": \"pulse-harness-1\"")
            || !data.contains("\"exit_status\": \"submitted\"")
        {
            panic!("bad trajectory: {data}");
        }
    }

    #[test]
    fn trajectory_written_every_step() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trajectory.json");
        let p = Arc::new(FakeProvider::new(vec![
            resp(tool("echo hi")),
            resp(msg("SUMMARY: done")),
        ]));
        let env = Arc::new(Mutex::new(FakeEnv {
            observations: vec![Observation {
                output: "hi\n".into(),
                exit_code: 0,
                ..Observation::default()
            }],
            ..FakeEnv::default()
        }));
        let trajectory_log = TestTrajectorySink {
            path: path.display().to_string(),
            steps: Vec::new(),
        };
        let saw_checkpoint = Arc::new(Mutex::new(false));
        let saw = saw_checkpoint.clone();
        let check_path = path.clone();
        let check = SinkFunc(move |s: Step| {
            if s.n != 1 {
                return;
            }
            let data = fs::read_to_string(&check_path).unwrap();
            let traj: Trajectory = serde_json::from_str(&data).unwrap();
            assert!(
                traj.steps.len() >= 2,
                "checkpoint has {} steps, want at least 2",
                traj.steps.len()
            );
            *saw.lock().unwrap() = true;
        });
        let mut agent = new(
            Box::new(SharedFakeProvider(p)),
            Box::new(env),
            base_config(),
            vec![Box::new(trajectory_log), Box::new(check)],
        );
        let got = agent.run().unwrap();
        assert_eq!(
            got.exit_status, EXIT_SUBMITTED,
            "exit = {}",
            got.exit_status
        );
        assert!(
            *saw_checkpoint.lock().unwrap(),
            "checkpoint was not observed after the second step"
        );
        super::write_trajectory(path.to_str().unwrap(), &agent.trajectory()).unwrap();
        let data = fs::read_to_string(&path).unwrap();
        let traj: Trajectory = serde_json::from_str(&data).unwrap();
        assert_eq!(
            traj.steps.len(),
            agent.trajectory().steps.len(),
            "final trajectory steps = {}, want {}",
            traj.steps.len(),
            agent.trajectory().steps.len()
        );
        assert!(
            fs::metadata(path.with_extension("json.tmp")).is_err(),
            "tmp file left behind"
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(data.contains("call-1"));
    }

    #[test]
    fn sink_panic_is_swallowed() {
        let _serial = crate::test_helpers::serial();
        let p = Arc::new(FakeProvider::new(vec![
            resp(msg("SUMMARY: done")),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = new(
            Box::new(SharedFakeProvider(p)),
            Box::new(Arc::new(Mutex::new(FakeEnv::default()))),
            base_config(),
            vec![Box::new(SinkFunc(|_| panic!("boom")))],
        );
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
    }

    #[test]
    fn complete_task_marker_from_command_output_submits() {
        let _serial = crate::test_helpers::serial();
        let p = Arc::new(FakeProvider::new(vec![resp(msg(
            "```bash\necho marker\n```",
        ))]));
        let env = Arc::new(Mutex::new(FakeEnv {
            observations: vec![Observation {
                output: "COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT\nSUMMARY: from marker\n".into(),
                exit_code: 0,
                ..Observation::default()
            }],
            ..FakeEnv::default()
        }));
        let (_, got) = run_agent(p, env, base_config()).unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        assert_eq!(got.submission.trim(), "SUMMARY: from marker");
    }

    #[test]
    fn unsuccessful_command_cannot_submit() {
        let _serial = crate::test_helpers::serial();
        for obs in [
            Observation {
                output: "COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT\nSUMMARY: prematurely done".into(),
                exit_code: 1,
                ..Observation::default()
            },
            Observation {
                output: "COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT\nSUMMARY: prematurely done".into(),
                exit_code: 0,
                timed_out: true,
                duration: Duration::from_secs(1),
            },
        ] {
            let p = Arc::new(FakeProvider::new(vec![
                resp(msg("```bash\nrun-check\n```")),
                resp(msg("```bash\nrepair-and-check\n```")),
                resp(msg("SUMMARY: repaired and checked")),
            ]));
            let env = Arc::new(Mutex::new(FakeEnv {
                observations: vec![
                    obs.clone(),
                    Observation {
                        output: "check passed".into(),
                        exit_code: 0,
                        ..Observation::default()
                    },
                ],
                ..FakeEnv::default()
            }));
            let (h, got) = run_agent(p, env.clone(), base_config()).unwrap();
            assert_eq!(got.exit_status, EXIT_SUBMITTED);
            assert_eq!(got.submission, "SUMMARY: repaired and checked");
            assert_eq!(env.lock().unwrap().commands[1], "repair-and-check");
            assert!(h.messages()[3].content.contains(&obs.output));
        }
    }

    #[test]
    fn recovery_guidance_preserves_limits_and_policy() {
        let _serial = crate::test_helpers::serial();
        let prompt = default_system_prompt(
            &Arc::new(Mutex::new(FakeEnv::default())).info(),
            base_config(),
        );
        for want in [
            "inspect the cause",
            "peer-help channel",
            "do not repeat an unchanged failure",
            "Never bypass a policy refusal",
            "SUMMARY: blocked:",
            "SUMMARY: failed:",
        ] {
            assert!(prompt.contains(want), "missing {want}");
        }
        let out = format_observation(
            &Observation {
                output: "partial failure".into(),
                exit_code: 124,
                timed_out: true,
                duration: Duration::from_secs(1),
            },
            200,
        );
        for want in [
            "partial failure",
            "<returncode>124</returncode>",
            "remaining budget",
            "Do not leave unsupervised background work",
        ] {
            assert!(out.contains(want), "lost {want}: {out}");
        }
    }

    fn obs(content: String) -> Message {
        Message {
            role: Role::Tool,
            tool_call_id: "call-1".into(),
            name: "bash".into(),
            content,
            tool_calls: Vec::new(),
            ..Default::default()
        }
    }

    #[test]
    fn prune_history_keep_n_collapses_older_observations() {
        let _serial = crate::test_helpers::serial();
        let msgs = vec![
            Message {
                role: Role::System,
                content: "system".into(),
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
                ..Default::default()
            },
            Message {
                role: Role::User,
                content: "task".into(),
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
                ..Default::default()
            },
            obs(format_observation(
                &Observation {
                    output: "a\nb\nc\nd\ne\nf".into(),
                    exit_code: 0,
                    ..Observation::default()
                },
                0,
            )),
            msg("still full"),
            obs(format_observation(
                &Observation {
                    output: "new\noutput".into(),
                    exit_code: 0,
                    ..Observation::default()
                },
                0,
            )),
        ];
        let (out, changed) = prune_history(&msgs, 1, 1);
        assert_eq!(changed, 1);
        assert!(out[2].content.contains("characters elided"));
        assert_eq!(out[3].content, "still full");
        assert_eq!(out[4].content, msgs[4].content);
    }

    #[test]
    fn prune_history_marker_idempotence() {
        let _serial = crate::test_helpers::serial();
        let msgs = vec![
            msg("system"),
            Message { role: Role::User, content: "task".into(), tool_calls: Vec::new(), tool_call_id: String::new(), name: String::new(), ..Default::default() },
            obs("<returncode>0</returncode>\n<output>\na\n[10 characters elided - rerun the command to see them]\nz\n</output>".into()),
            obs(format_observation(&Observation { output: "new".into(), exit_code: 0, ..Observation::default() }, 0)),
        ];
        let (out, changed) = prune_history(&msgs, 1, 1);
        assert_eq!(changed, 0);
        assert_eq!(out[2].content, msgs[2].content);
    }

    #[test]
    fn prune_history_leaves_short_outputs_unchanged() {
        let _serial = crate::test_helpers::serial();
        for output in ["a\nb\nc", "a\nb\nc\nd"] {
            let msgs = vec![
                msg("system"),
                Message {
                    role: Role::User,
                    content: "task".into(),
                    tool_calls: Vec::new(),
                    tool_call_id: String::new(),
                    name: String::new(),
                    ..Default::default()
                },
                obs(format_observation(
                    &Observation {
                        output: output.into(),
                        exit_code: 0,
                        ..Observation::default()
                    },
                    0,
                )),
                obs(format_observation(
                    &Observation {
                        output: "new".into(),
                        exit_code: 0,
                        ..Observation::default()
                    },
                    0,
                )),
            ];
            let (out, changed) = prune_history(&msgs, 1, 1);
            assert_eq!(changed, 0);
            assert_eq!(out[2].content, msgs[2].content);
        }
    }

    #[test]
    fn prune_history_level2_one_liners() {
        let _serial = crate::test_helpers::serial();
        let msgs = vec![
            msg("system"),
            Message {
                role: Role::User,
                content: "task".into(),
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
                ..Default::default()
            },
            obs(format_observation(
                &Observation {
                    output: "a\nb\nc\nd".into(),
                    exit_code: 7,
                    ..Observation::default()
                },
                0,
            )),
            obs(format_observation(
                &Observation {
                    output: "new".into(),
                    exit_code: 0,
                    ..Observation::default()
                },
                0,
            )),
        ];
        let (out, changed) = prune_history(&msgs, 1, 2);
        assert_eq!(changed, 1);
        assert_eq!(
            out[2].content,
            "<returncode>7</returncode>\n<output>\n[output elided - rerun the command to see it]\n</output>"
        );
    }

    #[test]
    fn prune_history_preserves_tool_call_ids_and_first_two_messages() {
        let _serial = crate::test_helpers::serial();
        let msgs = vec![
            Message {
                role: Role::System,
                content: "<returncode>0</returncode>\n<output>\nsystem\n</output>".into(),
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
                ..Default::default()
            },
            Message {
                role: Role::User,
                content: "<returncode>0</returncode>\n<output>\ntask\n</output>".into(),
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
                ..Default::default()
            },
            Message {
                role: Role::Tool,
                tool_call_id: "call-99".into(),
                name: "bash".into(),
                content: format_observation(
                    &Observation {
                        output: "a\nb\nc\nd\ne".into(),
                        exit_code: 0,
                        ..Observation::default()
                    },
                    0,
                ),
                tool_calls: Vec::new(),
                ..Default::default()
            },
            obs(format_observation(
                &Observation {
                    output: "new".into(),
                    exit_code: 0,
                    ..Observation::default()
                },
                0,
            )),
        ];
        let (out, changed) = prune_history(&msgs, 1, 1);
        assert_eq!(changed, 1);
        assert_eq!(out[0].content, msgs[0].content);
        assert_eq!(out[1].content, msgs[1].content);
        assert_eq!(out[2].tool_call_id, "call-99");
        assert_eq!(out[2].name, "bash");
    }

    #[test]
    fn prune_history_recognizes_fenced_mode_user_observations() {
        let _serial = crate::test_helpers::serial();
        let msgs = vec![
            msg("system"),
            Message {
                role: Role::User,
                content: "task".into(),
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
                ..Default::default()
            },
            Message {
                role: Role::User,
                content: format_observation(
                    &Observation {
                        output: "a\nb\nc\nd\ne".into(),
                        exit_code: 0,
                        ..Observation::default()
                    },
                    0,
                ),
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
                ..Default::default()
            },
            obs(format_observation(
                &Observation {
                    output: "new".into(),
                    exit_code: 0,
                    ..Observation::default()
                },
                0,
            )),
        ];
        let (out, changed) = prune_history(&msgs, 1, 1);
        assert_eq!(changed, 1);
        assert!(out[2].content.contains("characters elided"));
    }
    fn hook(event: HookEvent, tools: &[&str], command: &str) -> Hook {
        Hook {
            event,
            tools: tools.iter().map(|t| (*t).to_string()).collect(),
            command: command.to_string(),
            timeout: Duration::from_secs(5),
            source: ".pulse/hooks.json".to_string(),
        }
    }

    fn observed(output: &str, exit_code: i32) -> Observation {
        Observation {
            output: output.to_string(),
            exit_code,
            ..Observation::default()
        }
    }

    #[test]
    fn pre_tool_hook_blocks_a_call_and_post_tool_hook_speaks_after_it() {
        let _serial = crate::test_helpers::serial();
        let p = Arc::new(FakeProvider::new(vec![
            resp(tool("touch x")),
            resp(tool("echo ok")),
            resp(msg("SUMMARY: done")),
        ]));
        let env = Arc::new(Mutex::new(FakeEnv {
            observations: vec![
                observed("no file creation in this clone\n", HOOK_BLOCK_EXIT),
                observed("", 0),
                observed("ok\n", 0),
                observed("audited\n", 0),
            ],
            ..FakeEnv::default()
        }));
        let mut h = new(
            Box::new(SharedFakeProvider(p)),
            Box::new(env.clone()),
            base_config(),
            Vec::new(),
        );
        h.set_hooks(HookSet {
            hooks: vec![
                hook(HookEvent::PreTool, &["bash"], "guard.sh"),
                hook(HookEvent::PostTool, &[], "audit.sh"),
            ],
            ..HookSet::default()
        });
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        assert_eq!(got.steps, 1, "a blocked call and hooks are not tool steps");
        let replies = tool_replies(&h);
        assert_eq!(replies.len(), 2);
        assert!(
            replies[0].1.starts_with("<returncode>126</returncode>"),
            "{}",
            replies[0].1
        );
        assert!(
            replies[0]
                .1
                .contains("blocked by hook `guard.sh`: no file creation in this clone"),
            "{}",
            replies[0].1
        );
        assert!(replies[1].1.contains("ok\n"), "{}", replies[1].1);
        assert!(
            replies[1].1.ends_with("\n[hook `audit.sh`] audited"),
            "{}",
            replies[1].1
        );
        let commands = env.lock().unwrap().commands.clone();
        assert_eq!(commands.len(), 4, "{commands:?}");
        assert!(
            commands[0]
                .contains("PULSE_HOOK_EVENT='pre_tool' PULSE_TOOL='bash' /bin/sh -c 'guard.sh'"),
            "{}",
            commands[0]
        );
        assert!(
            commands[0].contains(
                r#"{"cwd":"","event":"pre_tool","input":{"command":"touch x"},"tool":"bash"}"#
            ),
            "{}",
            commands[0]
        );
        assert_eq!(commands[2], "echo ok");
        assert!(
            commands[3].contains("PULSE_HOOK_EVENT='post_tool'"),
            "{}",
            commands[3]
        );
        assert!(
            commands[3].contains(r#""exit_code":0"#) && commands[3].contains(r#""output":"ok\n""#),
            "{}",
            commands[3]
        );
        let kinds: Vec<String> = h.steps.iter().map(|s| s.kind.to_string()).collect();
        assert_eq!(
            kinds.iter().filter(|k| *k == "hook").count(),
            3,
            "{kinds:?}"
        );
        let blocked = h
            .steps
            .iter()
            .find(|s| s.kind == StepKind::Error)
            .expect("an error step for the blocked call");
        assert!(blocked.content.starts_with("blocked by hook `guard.sh`"));
        assert_eq!(blocked.command, "bash");
    }

    #[test]
    fn run_hooks_feed_the_prompt_and_hear_the_outcome() {
        let _serial = crate::test_helpers::serial();
        let p = Arc::new(FakeProvider::new(vec![
            resp(msg("SUMMARY: done")),
            resp(msg("SUMMARY: done")),
        ]));
        let env = Arc::new(Mutex::new(FakeEnv {
            observations: vec![
                observed("branch: main\nopen incidents: 0\n", 0),
                observed("", 0),
            ],
            ..FakeEnv::default()
        }));
        let mut h = new(
            Box::new(SharedFakeProvider(p.clone())),
            Box::new(env.clone()),
            base_config(),
            Vec::new(),
        );
        h.set_hooks(HookSet {
            hooks: vec![
                hook(HookEvent::RunStart, &[], "context.sh"),
                hook(HookEvent::RunEnd, &[], "notify.sh"),
            ],
            ..HookSet::default()
        });
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        let reqs = p.requests();
        // The first evidence-free report is sent back once.
        assert_eq!(reqs.len(), 2);
        assert_eq!(
            reqs[0].messages[0].content,
            "system\nHook output at run start:\nbranch: main\nopen incidents: 0\n"
        );
        let commands = env.lock().unwrap().commands.clone();
        assert_eq!(commands.len(), 2, "{commands:?}");
        assert!(
            commands[0]
                .contains("PULSE_HOOK_EVENT='run_start' PULSE_TOOL='' /bin/sh -c 'context.sh'")
        );
        assert!(
            commands[0]
                .contains(r#"{"cwd":"","depth":0,"event":"run_start","model":"m","task":"do it"}"#),
            "{}",
            commands[0]
        );
        assert!(
            commands[1].contains("PULSE_HOOK_EVENT='run_end'"),
            "{}",
            commands[1]
        );
        assert!(
            commands[1].contains(r#""exit_status":"submitted""#)
                && commands[1].contains(r#""submission":"SUMMARY: done""#)
                && commands[1].contains(r#""calls":2,"#),
            "{}",
            commands[1]
        );
        let kinds: Vec<String> = h.steps.iter().map(|s| s.kind.to_string()).collect();
        assert_eq!(
            kinds,
            vec![
                "hook",
                "observation",
                "note",
                "assistant",
                "format_retry",
                "assistant",
                "hook",
                "observation",
                "exit"
            ],
            "{kinds:?}"
        );
        assert!(
            h.steps[2]
                .content
                .starts_with("added 30 characters of run_start hook output")
        );
    }

    #[test]
    fn skill_tool_serves_folder_and_inline_skills() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let folder = dir.path().join(".pulse/skills/deploy");
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            folder.join("SKILL.md"),
            "---\nname: deploy\ndescription: Ship a release\n---\n# Deploy\nRun ship.sh first.\n",
        )
        .unwrap();
        fs::write(folder.join("notes.md"), "staging needs a warm cache\n").unwrap();
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[
                ("c1", "skill", json!({"name": "deploy"})),
                ("c2", "skill", json!({"name": "deploy", "file": "notes.md"})),
                ("c3", "skill", json!({"name": "triage"})),
                ("c4", "skill", json!({"name": "nope"})),
            ])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p.clone());
        let mut skills = crate::skills::load_skills(dir.path().to_str().unwrap(), "");
        skills.push(Skill::inline(
            "triage",
            "Sort the inbox",
            "Read every FINDING first.",
            "pulse skill",
        ));
        h.set_skills(skills);
        assert!(h.tools().iter().any(|t| t.name == "skill"));
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        assert_eq!(got.steps, 4);
        let replies = tool_replies(&h);
        assert_eq!(replies.len(), 4);
        assert!(
            replies[0]
                .1
                .contains("skill deploy (.pulse/skills/deploy)\n# Deploy\nRun ship.sh first.\n")
                && replies[0]
                    .1
                    .contains("Files in this skill (pass file to read one): notes.md"),
            "{}",
            replies[0].1
        );
        assert!(
            replies[1]
                .1
                .contains("notes.md of skill deploy\nstaging needs a warm cache"),
            "{}",
            replies[1].1
        );
        assert!(
            replies[2]
                .1
                .contains("skill triage (pulse skill)\nRead every FINDING first."),
            "{}",
            replies[2].1
        );
        assert!(
            replies[3].1.starts_with("<returncode>1</returncode>")
                && replies[3]
                    .1
                    .contains("unknown skill \"nope\"; available: deploy, triage"),
            "{}",
            replies[3].1
        );
        let skill_steps: Vec<&Step> = h
            .steps
            .iter()
            .filter(|s| s.kind == StepKind::Skill)
            .collect();
        assert_eq!(skill_steps.len(), 4);
        assert_eq!(skill_steps[1].command, "deploy notes.md");
        let req = &p.requests()[0];
        let tool = req.tools.iter().find(|t| t.name == "skill").unwrap();
        assert!(
            tool.description.contains("deploy: Ship a release")
                && tool.description.contains("triage: Sort the inbox"),
            "{}",
            tool.description
        );
    }

    #[test]
    fn fetch_tool_needs_a_policy_and_refuses_hosts_outside_it() {
        let _serial = crate::test_helpers::serial();
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[(
                "c1",
                "fetch",
                json!({"url": "https://evil.example.org/x"}),
            )])),
            resp(msg("SUMMARY: done")),
        ]));
        let env = Arc::new(Mutex::new(FakeEnv::default()));
        let mut h = new(
            Box::new(SharedFakeProvider(p)),
            Box::new(env),
            base_config(),
            Vec::new(),
        );
        assert!(!h.tools().iter().any(|t| t.name == "fetch"));
        h.set_fetch_policy(FetchPolicy::parse("docs.example.com, *.rust-lang.org"));
        assert!(h.tools().iter().any(|t| t.name == "fetch"));
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        assert_eq!(got.steps, 1);
        let replies = tool_replies(&h);
        assert!(
            replies[0].1.starts_with("<returncode>126</returncode>")
                && replies[0]
                    .1
                    .contains("refused by policy: host \"evil.example.org\""),
            "{}",
            replies[0].1
        );
        let kinds: Vec<String> = h.steps.iter().map(|s| s.kind.to_string()).collect();
        assert_eq!(
            kinds,
            vec![
                "assistant",
                "fetch",
                "error",
                "observation",
                "assistant",
                "exit"
            ],
            "{kinds:?}"
        );

        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[(
                "c1",
                "fetch",
                json!({"url": "https://docs.example.com/"}),
            )])),
            resp(msg("SUMMARY: done")),
        ]));
        let env = Arc::new(Mutex::new(FakeEnv::default()));
        let mut h = new(
            Box::new(SharedFakeProvider(p)),
            Box::new(env),
            base_config(),
            Vec::new(),
        );
        h.run().unwrap();
        let replies = tool_replies(&h);
        assert!(
            replies[0]
                .1
                .starts_with("unknown tool fetch; available tools: bash, read"),
            "{}",
            replies[0].1
        );
    }

    #[test]
    fn subagent_is_offered_only_to_a_top_level_run_that_allows_it() {
        let env = Arc::new(Mutex::new(FakeEnv::default()));
        let offered = |cfg: Config| {
            let p = Arc::new(FakeProvider::new(Vec::new()));
            let h = new(
                Box::new(SharedFakeProvider(p)),
                Box::new(env.clone()),
                cfg,
                Vec::new(),
            );
            h.tools().iter().any(|t| t.name == SUBAGENT_TOOL)
        };
        assert!(!offered(base_config()));
        let mut on = base_config();
        on.subagents = true;
        assert!(offered(on.clone()));
        on.depth = 1;
        assert!(!offered(on));
    }

    #[test]
    fn subagent_runs_a_helper_whose_steps_and_usage_join_the_run() {
        let _serial = crate::test_helpers::serial();
        let with_usage = |mut r: pulse_provider::Response, prompt: i32| {
            r.usage.prompt_tokens = prompt;
            r
        };
        let p = Arc::new(FakeProvider::new(vec![
            with_usage(
                resp(calls(&[(
                    "c1",
                    "subagent",
                    json!({"task": "count the files under src", "max_steps": 5}),
                )])),
                100,
            ),
            with_usage(resp(tool("ls src | wc -l")), 10),
            with_usage(resp(msg("SUMMARY: three files under src")), 20),
            with_usage(resp(msg("SUMMARY: done; the helper counted three")), 200),
        ]));
        let env = Arc::new(Mutex::new(FakeEnv {
            observations: vec![observed("3\n", 0)],
            ..FakeEnv::default()
        }));
        let seen: Arc<Mutex<Vec<Step>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_steps = seen.clone();
        let mut cfg = base_config();
        cfg.subagents = true;
        cfg.max_steps = 12;
        let mut h = new(
            Box::new(SharedFakeProvider(p.clone())),
            Box::new(env.clone()),
            cfg,
            vec![Box::new(SinkFunc(move |s: Step| {
                sink_steps.lock().unwrap().push(s)
            }))],
        );
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        assert_eq!(got.submission, "SUMMARY: done; the helper counted three");
        assert_eq!(got.calls, 4, "the helper's calls count against the run");
        assert_eq!(got.steps, 2, "the subagent call and the helper's command");
        assert_eq!(got.subagents, 1);
        assert_eq!(got.usage.prompt_tokens, 330);
        let replies = tool_replies(&h);
        assert_eq!(replies.len(), 1);
        assert!(
            replies[0].1.starts_with(
                "<returncode>0</returncode>\n<output>\nsubagent submitted: 2 model calls, 1 tool steps, "
            ),
            "{}",
            replies[0].1
        );
        assert!(
            replies[0]
                .1
                .ends_with("\n\nSUMMARY: three files under src\n</output>"),
            "{}",
            replies[0].1
        );
        assert_eq!(
            env.lock().unwrap().commands,
            vec!["ls src | wc -l".to_string()]
        );

        let reqs = p.requests();
        assert_eq!(reqs.len(), 4);
        let child = &reqs[1];
        assert!(
            child.messages[0].content.contains("Your tools:"),
            "child gets a rendered prompt"
        );
        assert!(
            child.messages[1]
                .content
                .starts_with("You are a helper agent of a larger run")
        );
        assert!(
            child.messages[1]
                .content
                .ends_with("Sub-task:\ncount the files under src")
        );
        assert!(
            !child.tools.iter().any(|t| t.name == SUBAGENT_TOOL),
            "no nesting"
        );
        assert!(reqs[0].tools.iter().any(|t| t.name == SUBAGENT_TOOL));

        let steps = seen.lock().unwrap();
        let ns: Vec<i32> = steps.iter().map(|s| s.n).collect();
        let mut sorted = ns.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ns, sorted, "step numbers run through the helper: {ns:?}");
        let summary: Vec<(String, u8)> = steps
            .iter()
            .map(|s| (s.kind.to_string(), s.depth))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("assistant".to_string(), 0),
                ("subagent".to_string(), 0),
                ("assistant".to_string(), 1),
                ("command".to_string(), 1),
                ("observation".to_string(), 1),
                ("assistant".to_string(), 1),
                ("exit".to_string(), 1),
                ("observation".to_string(), 0),
                ("assistant".to_string(), 0),
                ("exit".to_string(), 0),
            ],
            "{summary:?}"
        );
        assert_eq!(steps[1].command, "subagent (5 calls)");
        assert_eq!(steps[6].content, EXIT_SUBMITTED);
        assert_eq!(
            h.steps.len(),
            steps.len(),
            "the trajectory keeps the helper's steps too"
        );
        assert!(
            h.steps
                .iter()
                .any(|s| s.depth == 1 && s.kind == StepKind::Command)
        );
    }

    #[test]
    fn subagent_is_refused_when_the_run_is_nearly_out_of_calls() {
        let _serial = crate::test_helpers::serial();
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[("c1", "subagent", json!({"task": "look around"}))])),
            resp(msg("SUMMARY: done")),
        ]));
        let env = Arc::new(Mutex::new(FakeEnv::default()));
        let mut cfg = base_config();
        cfg.subagents = true;
        cfg.max_steps = 5;
        let (h, got) = run_agent(p.clone(), env, cfg).unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        assert_eq!(got.calls, 2);
        assert_eq!(got.subagents, 0);
        let replies = tool_replies(&h);
        assert!(
            replies[0].1.starts_with("<returncode>1</returncode>")
                && replies[0]
                    .1
                    .contains("subagent: only 4 model calls remain in this run; do the work yourself and finish"),
            "{}",
            replies[0].1
        );
        assert_eq!(p.requests().len(), 2, "no helper ran");
    }

    #[test]
    fn compaction_replaces_older_messages_with_a_summary_the_model_writes() {
        let _serial = crate::test_helpers::serial();
        let with_usage = |mut r: pulse_provider::Response, prompt: i32| {
            r.usage.prompt_tokens = prompt;
            r
        };
        let p = Arc::new(FakeProvider::new(vec![
            with_usage(resp(tool("echo 1")), 800),
            with_usage(resp(tool("echo 2")), 800),
            with_usage(resp(tool("echo 3")), 800),
            with_usage(resp(tool("echo 4")), 800),
            with_usage(resp(msg("Earlier: ran echo 1, saw one.")), 50),
            with_usage(resp(msg("SUMMARY: done")), 300),
        ]));
        let env = Arc::new(Mutex::new(FakeEnv {
            default_output: "line\n".to_string(),
            ..FakeEnv::default()
        }));
        let mut cfg = base_config();
        cfg.max_steps = 20;
        cfg.context_window = 1000;
        cfg.compact_at = 70;
        let (h, got) = run_agent(p.clone(), env, cfg).unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        assert_eq!(got.calls, 5, "the summary call is not a step of the run");
        assert_eq!(got.compactions, 1);
        assert_eq!(got.usage.prompt_tokens, 4 * 800 + 50 + 300);
        let reqs = p.requests();
        assert_eq!(reqs.len(), 6);
        let summary_req = &reqs[4];
        assert!(summary_req.tools.is_empty());
        assert_eq!(summary_req.initiator, "agent");
        assert!(
            summary_req
                .messages
                .iter()
                .any(|m| m.content.contains("echo 1")),
            "the transcript carries the cut messages"
        );
        let after = &reqs[5].messages;
        assert_eq!(after[0].content, "system");
        assert_eq!(after[1].content, "do it");
        assert!(
            after[2].content.starts_with(crate::compact::COMPACT_MARKER)
                && after[2].content.contains("Earlier: ran echo 1, saw one."),
            "{}",
            after[2].content
        );
        assert!(
            !after
                .iter()
                .any(|m| m.tool_calls.iter().any(|c| c.arguments.contains("echo 1"))),
            "the first call is gone from the live history"
        );
        assert!(
            after
                .iter()
                .any(|m| m.tool_calls.iter().any(|c| c.arguments.contains("echo 2")))
        );
        let note = h
            .steps
            .iter()
            .find(|s| s.kind == StepKind::Note && s.content.starts_with("compacted "))
            .expect("a compaction note");
        assert!(
            note.content.starts_with(
                "compacted 2 older messages into a 29-character summary (10 messages became 9)"
            ),
            "{}",
            note.content
        );
        assert_eq!(note.usage.prompt_tokens, 50);

        let mut off = base_config();
        off.max_steps = 20;
        off.context_window = 1000;
        let p = Arc::new(FakeProvider::new(vec![
            with_usage(resp(tool("echo 1")), 800),
            with_usage(resp(tool("echo 2")), 800),
            with_usage(resp(tool("echo 3")), 800),
            with_usage(resp(tool("echo 4")), 800),
            with_usage(resp(msg("SUMMARY: done")), 800),
        ]));
        let env = Arc::new(Mutex::new(FakeEnv::default()));
        let (_, got) = run_agent(p.clone(), env, off).unwrap();
        assert_eq!(got.compactions, 0, "compact_at 0 is off");
        assert_eq!(p.requests().len(), 5);
    }

    /// A temp directory that is a git repo with one commit, for the
    /// read-only tree guard.
    fn git_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let out = std::process::Command::new("sh")
            .args([
                "-c",
                "git init -q -b main && echo one > a.txt && git add . && git -c user.name=t -c user.email=t@example.com commit -q -m init",
            ])
            .current_dir(dir.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        dir
    }

    #[test]
    fn read_only_run_offers_no_edit_or_write_and_refuses_changes() {
        let _serial = crate::test_helpers::serial();
        let dir = git_repo();
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[
                ("r1", "read", json!({"path":"a.txt"})),
                (
                    "e1",
                    "edit",
                    json!({"path":"a.txt","search":"one","replace":"two"}),
                ),
                ("w1", "write", json!({"path":"b.txt","content":"x"})),
            ])),
            resp(calls(&[(
                "b1",
                "bash",
                json!({"command":"git add . && git commit -m x"}),
            )])),
            resp(calls(&[(
                "b2",
                "bash",
                json!({"command":"python3 -c \"open('made.txt','w').write('x'); open('a.txt','w').write('changed')\" && echo ran"}),
            )])),
            resp(calls(&[(
                "b3",
                "bash",
                json!({"command":"cat a.txt && touch /tmp/pulse-ro-ok"}),
            )])),
            resp(msg("SUMMARY: looked, nothing changed")),
        ]));
        let mut h = local_agent(dir.path(), p.clone());
        h.cfg.read_only = true;
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED, "{got:?}");

        let requests = p.requests();
        let offered: Vec<&str> = requests[0].tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(offered, READ_ONLY_TOOL_NAMES.to_vec());
        assert!(
            requests[0].messages[0]
                .content
                .contains("This is a read-only run"),
            "the system prompt says so"
        );
        let steps = h.trajectory().steps;
        assert_eq!(steps[0].kind, StepKind::Note);
        assert!(steps[0].content.starts_with("read-only run:"), "{steps:?}");

        let replies: BTreeMap<String, String> = tool_replies(&h).into_iter().collect();
        assert!(replies["r1"].contains("one"), "{}", replies["r1"]);
        for id in ["e1", "w1"] {
            assert!(
                replies[id].contains("refused: this is a read-only run")
                    && replies[id].contains("<returncode>126</returncode>"),
                "{id}: {}",
                replies[id]
            );
        }
        assert!(
            replies["b1"].contains("refused: this is a read-only run and `git add`")
                && replies["b1"].contains("<returncode>126</returncode>"),
            "{}",
            replies["b1"]
        );
        // The python one-liner slipped past the checker: the guard put the
        // clone back and told the model.
        assert!(replies["b2"].contains("ran"), "{}", replies["b2"]);
        assert!(
            replies["b2"].contains("[read-only run: the command changed the clone (")
                && replies["b2"].contains("made.txt (new, removed)")
                && replies["b2"].contains("a.txt"),
            "{}",
            replies["b2"]
        );
        assert!(!dir.path().join("made.txt").exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "one\n"
        );
        assert!(replies["b3"].contains("one"), "{}", replies["b3"]);
        assert!(
            !replies["b3"].contains("read-only run:"),
            "a scratch write under /tmp is fine: {}",
            replies["b3"]
        );
        let _ = fs::remove_file("/tmp/pulse-ro-ok");
        let errors = steps.iter().filter(|s| s.kind == StepKind::Error).count();
        assert_eq!(
            errors, 4,
            "edit, write, git commit, slipped change: {steps:?}"
        );
    }

    #[test]
    fn read_only_subagents_inherit_the_mode() {
        let _serial = crate::test_helpers::serial();
        let dir = git_repo();
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[(
                "s1",
                SUBAGENT_TOOL,
                json!({"task":"look around"}),
            )])),
            // The child's one reply: it tries to write.
            resp(calls(&[(
                "c1",
                "write",
                json!({"path":"c.txt","content":"x"}),
            )])),
            resp(msg("SUMMARY: child saw a refusal")),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p.clone());
        h.cfg.read_only = true;
        h.cfg.subagents = true;
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED, "{got:?}");
        let requests = p.requests();
        let child_tools: Vec<&str> = requests[1].tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            !child_tools.contains(&"write") && !child_tools.contains(&"edit"),
            "{child_tools:?}"
        );
        assert!(!dir.path().join("c.txt").exists());
    }

    #[test]
    fn bash_description_becomes_the_command_step_title() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[(
                "b1",
                "bash",
                json!({"command":"echo one","description":"  Print the first word  "}),
            )])),
            // A call without one still runs; the row simply has no title.
            resp(calls(&[("b2", "bash", json!({"command":"echo two"}))])),
            resp(calls(&[(
                "b3",
                "bash",
                json!({"command":"echo three","description":"x".repeat(300)}),
            )])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p);
        let bash = h.tools().into_iter().find(|t| t.name == "bash").unwrap();
        assert!(bash.parameters["properties"]["description"].is_object());
        assert!(
            bash.parameters["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "description"),
            "{}",
            bash.parameters
        );
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED, "{}", got.error);
        let titles: Vec<(String, String)> = h
            .trajectory()
            .steps
            .iter()
            .filter(|s| s.kind == StepKind::Command)
            .map(|s| (s.command.clone(), s.title.clone()))
            .collect();
        assert_eq!(titles.len(), 3, "{titles:?}");
        assert_eq!(
            titles[0],
            ("echo one".into(), "Print the first word".into())
        );
        assert_eq!(titles[1], ("echo two".into(), String::new()));
        assert_eq!(
            titles[2].1.len(),
            TITLE_LIMIT,
            "a long description is cut to a line: {:?}",
            titles[2].1
        );
        // The title rides along in the trajectory and is left out when empty.
        let json = serde_json::to_value(&h.trajectory().steps).unwrap();
        let commands: Vec<&Value> = json
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["kind"] == "command")
            .collect();
        assert_eq!(commands[0]["title"], "Print the first word");
        assert!(commands[1].get("title").is_none(), "{}", commands[1]);
    }

    #[test]
    fn parallel_read_calls_answer_in_call_order_around_a_sequential_one() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "alpha\n").unwrap();
        fs::write(dir.path().join("b.txt"), "beta\n").unwrap();
        fs::write(dir.path().join("c.txt"), "gamma\n").unwrap();
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[
                ("r1", "read", json!({"path":"a.txt"})),
                ("r2", "read", json!({"path":"b.txt"})),
                ("b1", "bash", json!({"command":"echo from-bash"})),
                ("g1", "grep", json!({"pattern":"gamma"})),
                ("r3", "read", json!({"path":"missing.txt"})),
            ])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p);
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED, "{got:?}");
        assert_eq!(got.steps, 5);
        let replies = tool_replies(&h);
        let ids: Vec<&str> = replies.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            ["r1", "r2", "b1", "g1", "r3"],
            "results keep call order"
        );
        let by_id: BTreeMap<String, String> = replies.into_iter().collect();
        assert!(by_id["r1"].contains("alpha"), "{}", by_id["r1"]);
        assert!(by_id["r2"].contains("beta"), "{}", by_id["r2"]);
        assert!(by_id["b1"].contains("from-bash"), "{}", by_id["b1"]);
        assert!(by_id["g1"].contains("c.txt:1:"), "{}", by_id["g1"]);
        assert!(
            by_id["r3"].contains("cannot be resolved inside the clone"),
            "a failed read in the batch still answers: {}",
            by_id["r3"]
        );
        // Every batched call is announced before any of them answers; the
        // bash call runs after the batch, in its own turn.
        let kinds: Vec<StepKind> = h
            .trajectory()
            .steps
            .iter()
            .map(|s| s.kind.clone())
            .collect();
        let first_obs = kinds
            .iter()
            .position(|k| *k == StepKind::Observation)
            .unwrap();
        assert_eq!(
            &kinds[1..first_obs],
            &[
                StepKind::Read,
                StepKind::Read,
                StepKind::Grep,
                StepKind::Read
            ],
            "{kinds:?}"
        );
        let bash_at = kinds.iter().position(|k| *k == StepKind::Command).unwrap();
        assert!(bash_at > first_obs, "{kinds:?}");
        let obs_commands: Vec<String> = h
            .trajectory()
            .steps
            .iter()
            .filter(|s| s.kind == StepKind::Observation)
            .map(|s| s.command.clone())
            .collect();
        assert_eq!(
            obs_commands,
            [
                "read tool",
                "read tool",
                "grep tool",
                "read tool",
                "echo from-bash"
            ],
            "{obs_commands:?}"
        );
    }

    #[test]
    fn a_lone_read_beside_a_bash_call_runs_in_order() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[
                ("b1", "bash", json!({"command":"echo made > a.txt"})),
                ("r1", "read", json!({"path":"a.txt"})),
            ])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p);
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED, "{got:?}");
        let by_id: BTreeMap<String, String> = tool_replies(&h).into_iter().collect();
        assert!(
            by_id["r1"].contains("made"),
            "one read-side call is no batch, so it sees the bash call's file: {}",
            by_id["r1"]
        );
    }

    // ---- interactive sessions (pulse harness chat) ----

    /// A scripted [`Ui`]: answers approvals from a queue (`true` allows,
    /// `false` declines with "not that") and questions from another.
    #[derive(Default)]
    struct ScriptUi {
        approvals: Vec<bool>,
        /// Decisions taken before `approvals`, for `Approval::Always`.
        decisions: Vec<Approval>,
        answers: Vec<Option<String>>,
        seen: Vec<ApprovalRequest>,
        asked: Vec<(String, Vec<String>)>,
    }

    impl Ui for Arc<Mutex<ScriptUi>> {
        fn approve(&mut self, req: &ApprovalRequest) -> Approval {
            let mut ui = self.lock().unwrap();
            ui.seen.push(req.clone());
            if !ui.decisions.is_empty() {
                return ui.decisions.remove(0);
            }
            if ui.approvals.is_empty() || ui.approvals.remove(0) {
                Approval::Allow
            } else {
                Approval::Deny("not that".into())
            }
        }
        fn ask(&mut self, question: &str, choices: &[String]) -> Option<String> {
            let mut ui = self.lock().unwrap();
            ui.asked.push((question.to_string(), choices.to_vec()));
            if ui.answers.is_empty() {
                None
            } else {
                ui.answers.remove(0)
            }
        }
    }

    fn chat_agent(
        dir: &Path,
        p: Arc<FakeProvider>,
        ui: Option<Arc<Mutex<ScriptUi>>>,
        steps: Arc<Mutex<Vec<Step>>>,
    ) -> Agent {
        let mut cfg = base_config();
        cfg.task = String::new();
        cfg.system_prompt = String::new();
        cfg.cwd = dir.display().to_string();
        cfg.max_steps = 20;
        cfg.interactive = true;
        let sink_steps = steps.clone();
        let mut h = new(
            Box::new(SharedFakeProvider(p)),
            Box::new(crate::localenv::new_local_env(
                dir.display().to_string(),
                None,
            )),
            cfg,
            vec![Box::new(SinkFunc(move |s| {
                sink_steps.lock().unwrap().push(s);
            }))],
        );
        if let Some(ui) = ui {
            h.set_ui(Arc::new(Mutex::new(ui)));
        }
        h
    }

    fn chat_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(dir.path().join("a.txt"), "old line\n").unwrap();
        dir
    }

    #[test]
    fn chat_turns_answer_in_plain_text_and_keep_the_conversation() {
        let _serial = crate::test_helpers::serial();
        let dir = chat_dir();
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[("r1", "read", json!({"path":"a.txt"}))])),
            resp(msg("It says old line.")),
            resp(msg("Yes, one line.")),
        ]));
        let steps = Arc::new(Mutex::new(Vec::new()));
        let mut h = chat_agent(dir.path(), p.clone(), None, steps.clone());
        let cancel = CancelToken::new();
        let first = h.turn(&cancel, "what is in a.txt?").unwrap();
        assert!(
            matches!(&first, Turn::Reply(t) if t == "It says old line."),
            "{first:?}"
        );
        let second = h.turn(&cancel, "just one line?").unwrap();
        assert!(
            matches!(&second, Turn::Reply(t) if t == "Yes, one line."),
            "{second:?}"
        );
        assert!(!h.is_ended());
        let reqs = p.requests();
        assert_eq!(reqs.len(), 3);
        let system = &reqs[0].messages[0];
        assert_eq!(system.role, Role::System);
        assert!(
            system.content.contains("interactive session"),
            "chat prompt is used: {}",
            &system.content[..120]
        );
        assert!(
            !system.content.contains("SUMMARY:"),
            "no report format in the chat prompt"
        );
        assert!(
            !reqs[0].tools.iter().any(|t| t.name == QUESTION_TOOL),
            "no question tool without a ui"
        );
        // The second turn sees the whole first turn.
        let last = &reqs[2].messages;
        assert_eq!(last[1].content, "what is in a.txt?");
        assert_eq!(last.last().unwrap().content, "just one line?");
        assert!(last.iter().any(|m| m.role == Role::Tool));
        let got = h.end(EXIT_ENDED);
        assert_eq!(got.exit_status, EXIT_ENDED);
        assert_eq!(got.calls, 3);
        assert_eq!(got.steps, 1);
        assert!(h.is_ended());
        let kinds: Vec<StepKind> = steps
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.kind.clone())
            .collect();
        assert_eq!(kinds.last(), Some(&StepKind::Exit), "{kinds:?}");
        // Ending twice is harmless.
        assert_eq!(h.end(EXIT_ENDED).exit_status, EXIT_ENDED);
    }

    #[test]
    fn chat_rewind_puts_files_and_conversation_back_to_before_a_turn() {
        use crate::rewind::What;
        let _serial = crate::test_helpers::serial();
        let dir = chat_dir();
        let home = tempfile::tempdir().unwrap();
        let p = Arc::new(FakeProvider::new(vec![
            // Turn 1: edits a.txt and writes a new file under a new directory.
            resp(calls(&[
                (
                    "e1",
                    "edit",
                    json!({"path":"a.txt","search":"old line","replace":"new line"}),
                ),
                (
                    "w1",
                    "write",
                    json!({"path":"src/new.rs","content":"fn main() {}\n"}),
                ),
            ])),
            resp(msg("Edited a and made src/new.rs.")),
            // Turn 2: edits a.txt again, reads it, and one edit fails.
            resp(calls(&[(
                "e2",
                "edit",
                json!({"path":"a.txt","search":"new line","replace":"newer line"}),
            )])),
            resp(calls(&[
                ("r1", "read", json!({"path":"a.txt"})),
                (
                    "e3",
                    "edit",
                    json!({"path":"b.txt","search":"nothing","replace":"x"}),
                ),
            ])),
            resp(msg("Changed a again.")),
            // Turn 3 after the rewind: the conversation starts over from turn 2.
            resp(msg("Fresh start.")),
        ]));
        let steps = Arc::new(Mutex::new(Vec::new()));
        let mut h = chat_agent(dir.path(), p.clone(), None, steps.clone());
        let rewind_dir = home.path().join("harness/hs-1.rewind");
        h.set_rewind_dir(rewind_dir.clone());
        assert!(h.rewind_turns().is_empty());
        assert_eq!(
            h.rewind(1, What::Both).unwrap_err(),
            "there are 0 turns to rewind; /rewind lists them"
        );
        assert_eq!(
            h.rewind(0, What::Both).unwrap_err(),
            "turns count from 1; /rewind lists them"
        );
        let cancel = CancelToken::new();
        h.turn(&cancel, "change a and add new.rs").unwrap();
        h.turn(&cancel, "--- the user ran `ls` in the shell; its output ---\na.txt\n--- end of output ---\n\nchange a again\nplease").unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "newer line\n"
        );
        assert!(dir.path().join("src/new.rs").exists());
        assert_eq!(
            h.rewind_turns(),
            [
                "1. change a and add new.rs - 2 files: a.txt, src/new.rs",
                "2. change a again - 1 file: a.txt",
            ],
            "a failed edit and a read record nothing"
        );
        let index: Value =
            serde_json::from_slice(&fs::read(rewind_dir.join("index.json")).unwrap()).unwrap();
        assert_eq!(index["turns"].as_array().unwrap().len(), 2);
        assert_eq!(
            fs::read_to_string(rewind_dir.join("2/0")).unwrap(),
            "new line\n",
            "turn 2's pre-image of a.txt is what turn 1 left"
        );
        let messages_before = h.messages().len();

        // Back to before turn 2: a.txt as turn 1 left it, the conversation
        // ends with turn 1's answer.
        let report = h.rewind(2, What::Both).unwrap();
        assert_eq!(report.restored, ["a.txt"]);
        assert!(report.removed.is_empty());
        assert_eq!(report.turns, 1);
        assert!(
            report.dropped_messages > 0
                && messages_before - report.dropped_messages == h.messages().len()
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "new line\n"
        );
        let msgs = h.messages();
        assert_eq!(
            msgs.last().unwrap().content,
            "Edited a and made src/new.rs."
        );
        assert_eq!(msgs.iter().filter(|m| is_turn_start(m)).count(), 1);
        assert_eq!(
            h.rewind_turns(),
            ["1. change a and add new.rs - 2 files: a.txt, src/new.rs"]
        );
        let note = steps
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|s| s.kind == StepKind::Note)
            .map(|s| s.content.clone())
            .unwrap();
        assert_eq!(
            note,
            "rewound to before turn 2: put 1 file back (a.txt); dropped 7 messages of 1 turn from the conversation"
        );

        // The next turn continues from there: the model sees turn 1 and
        // the new message, nothing of turn 2.
        let third = h.turn(&cancel, "start over").unwrap();
        assert!(
            matches!(&third, Turn::Reply(t) if t == "Fresh start."),
            "{third:?}"
        );
        let reqs = p.requests();
        let last = &reqs.last().unwrap().messages;
        assert!(
            !last.iter().any(|m| m.content.contains("change a again")),
            "{last:?}"
        );
        assert_eq!(last.last().unwrap().content, "start over");
        assert_eq!(h.rewind_turns().len(), 2);

        // Files only: the checkout goes back to the start, the conversation
        // keeps both turns (with nothing left to put back).
        let report = h.rewind(1, What::Files).unwrap();
        assert_eq!(report.restored, ["a.txt"]);
        assert_eq!(report.removed, ["src/new.rs"]);
        assert_eq!(report.truncate_to, None);
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "old line\n"
        );
        assert!(
            !dir.path().join("src").exists(),
            "the directory the write made goes too"
        );
        assert_eq!(h.messages().last().unwrap().content, "Fresh start.");
        assert_eq!(
            h.rewind_turns(),
            [
                "1. change a and add new.rs - no file changes",
                "2. start over - no file changes",
            ]
        );
        // Conversation only, to the very start: just the system prompt is
        // left and the records are gone.
        let report = h.rewind(1, What::Conversation).unwrap();
        assert_eq!(report.truncate_to, Some(1));
        assert_eq!(h.messages().len(), 1);
        assert!(h.rewind_turns().is_empty());
        let index: Value =
            serde_json::from_slice(&fs::read(rewind_dir.join("index.json")).unwrap()).unwrap();
        assert_eq!(index["turns"].as_array().unwrap().len(), 0);
        assert_eq!(h.end(EXIT_ENDED).exit_status, EXIT_ENDED);
    }

    #[test]
    fn chat_rewind_reaches_the_turns_of_a_resumed_session() {
        use crate::rewind::What;
        let _serial = crate::test_helpers::serial();
        let dir = chat_dir();
        let home = tempfile::tempdir().unwrap();
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[(
                "e1",
                "edit",
                json!({"path":"a.txt","search":"old line","replace":"new line"}),
            )])),
            resp(msg("Edited a.")),
        ]));
        let steps = Arc::new(Mutex::new(Vec::new()));
        let mut h = chat_agent(dir.path(), p, None, steps.clone());
        h.set_rewind_dir(home.path().join("harness/hs-1.rewind"));
        let cancel = CancelToken::new();
        h.turn(&cancel, "change a").unwrap();
        let carried = h.messages();
        h.end(EXIT_ENDED);

        // Resumed: the earlier turn is listed and rewinds; the note the
        // resume writes is not a turn.
        let p = Arc::new(FakeProvider::new(vec![resp(msg("Hello again."))]));
        let mut later = chat_agent(dir.path(), p.clone(), None, steps.clone());
        later.set_rewind_dir(home.path().join("harness/hs-2.rewind"));
        later.resume_messages(carried).unwrap();
        assert_eq!(
            later
                .load_rewind(&home.path().join("harness/hs-1.rewind"))
                .unwrap(),
            1
        );
        assert_eq!(
            later
                .load_rewind(&home.path().join("harness/none.rewind"))
                .unwrap(),
            0
        );
        later.turn(&cancel, "hello").unwrap();
        assert_eq!(
            later.rewind_turns(),
            ["1. change a - 1 file: a.txt", "2. hello - no file changes"]
        );
        let reqs = p.requests();
        let last = &reqs[0].messages;
        assert!(
            last.iter()
                .any(|m| m.content.starts_with("Note: resumed an earlier session"))
        );
        let report = later.rewind(1, What::Both).unwrap();
        assert_eq!(report.restored, ["a.txt"]);
        assert_eq!(report.turns, 2);
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "old line\n"
        );
        assert_eq!(later.messages().len(), 1, "only the system prompt is left");
        assert!(later.rewind_turns().is_empty());
    }

    #[test]
    fn chat_step_budget_is_per_turn_and_a_stopped_turn_can_go_on() {
        let _serial = crate::test_helpers::serial();
        let dir = chat_dir();
        let mut script: Vec<pulse_provider::Response> =
            (0..8).map(|_| resp(tool("true"))).collect();
        script.push(resp(msg("second turn done")));
        script.extend((0..3).map(|_| resp(tool("true"))));
        script.push(resp(msg("third turn done")));
        let p = Arc::new(FakeProvider::new(script));
        let steps = Arc::new(Mutex::new(Vec::new()));
        let mut h = chat_agent(dir.path(), p.clone(), None, steps.clone());
        h.cfg.max_steps = 8;
        let cancel = CancelToken::new();
        let first = h.turn(&cancel, "run true forever").unwrap();
        assert!(
            matches!(&first, Turn::Stopped { status, error }
                if status == EXIT_STEP_LIMIT && error.contains("say continue")),
            "{first:?}"
        );
        assert!(!h.is_ended(), "a budget stop does not end the session");
        assert!(
            steps
                .lock()
                .unwrap()
                .iter()
                .any(|s| s.kind == StepKind::Note && s.content.starts_with("turn stopped: ")),
        );
        // The next turn gets a fresh budget of its own.
        let second = h.turn(&cancel, "continue").unwrap();
        assert!(
            matches!(&second, Turn::Reply(t) if t == "second turn done"),
            "{second:?}"
        );
        let third = h.turn(&cancel, "and again").unwrap();
        assert!(
            matches!(&third, Turn::Reply(t) if t == "third turn done"),
            "{third:?}"
        );
        assert_eq!(p.requests().len(), 13);
        let got = h.end(EXIT_ENDED);
        assert_eq!(got.calls, 13);
    }

    #[test]
    fn chat_shows_edits_writes_and_landing_commands_to_the_user() {
        let _serial = crate::test_helpers::serial();
        let dir = chat_dir();
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[(
                "e1",
                "edit",
                json!({"path":"a.txt","search":"old line","replace":"new line"}),
            )])),
            resp(calls(&[(
                "w1",
                "write",
                json!({"path":"b.txt","content":"made\n"}),
            )])),
            resp(tool("git add a.txt")),
            resp(tool("git push origin main")),
            resp(tool("cat a.txt")),
            resp(msg("done")),
        ]));
        let ui = Arc::new(Mutex::new(ScriptUi {
            approvals: vec![false, true, true, false],
            ..ScriptUi::default()
        }));
        let steps = Arc::new(Mutex::new(Vec::new()));
        let mut h = chat_agent(dir.path(), p.clone(), Some(ui.clone()), steps.clone());
        let cancel = CancelToken::new();
        let turn = h.turn(&cancel, "change a.txt").unwrap();
        assert!(matches!(&turn, Turn::Reply(t) if t == "done"), "{turn:?}");
        let replies: BTreeMap<String, String> = tool_replies(&h).into_iter().collect();
        assert!(
            replies["e1"].contains("edit: declined by the user: not that"),
            "{}",
            replies["e1"]
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "old line\n"
        );
        assert!(replies["w1"].contains("wrote b.txt"), "{}", replies["w1"]);
        let seen = ui.lock().unwrap().seen.clone();
        assert_eq!(seen.len(), 4, "{seen:?}");
        assert_eq!(seen[0].tool, "edit");
        assert_eq!(seen[0].summary, "a.txt");
        assert!(
            seen[0].detail.contains("- old line\n+ new line"),
            "{}",
            seen[0].detail
        );
        assert_eq!(seen[1].tool, "write");
        assert!(
            seen[1].reason.starts_with("creates b.txt"),
            "{}",
            seen[1].reason
        );
        assert_eq!(seen[2].summary, "git add a.txt");
        assert!(
            seen[2].reason.contains("changes the index"),
            "{}",
            seen[2].reason
        );
        assert_eq!(seen[3].summary, "git push origin main");
        assert!(
            seen[3].reason.starts_with("crosses the job policy line"),
            "{}",
            seen[3].reason
        );
        let push_reply = h
            .messages()
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| m.content.clone())
            .find(|c| c.contains("git push") || c.contains("declined by the user: not that"))
            .unwrap_or_default();
        assert!(push_reply.contains("declined by the user"), "{push_reply}");
        // Plain reads go through without a word to the user.
        assert!(seen.iter().all(|r| r.summary != "cat a.txt"));
        assert!(
            steps
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.kind == StepKind::Note && s.content.contains("declined by the user"))
                .count()
                == 2
        );
    }

    #[test]
    fn chat_without_a_ui_still_refuses_policy_commands() {
        let _serial = crate::test_helpers::serial();
        let dir = chat_dir();
        let p = Arc::new(FakeProvider::new(vec![
            resp(tool("git push origin main")),
            resp(msg("could not")),
        ]));
        let steps = Arc::new(Mutex::new(Vec::new()));
        let mut h = chat_agent(dir.path(), p.clone(), None, steps);
        let cancel = CancelToken::new();
        h.turn(&cancel, "push it").unwrap();
        let reply = tool_replies(&h).remove(0).1;
        assert!(reply.contains("refused by policy"), "{reply}");
    }

    fn rules(specs: &[(bool, &str)]) -> Rules {
        let mut out = Rules::new();
        for (allow, spec) in specs {
            out.push(Rule::parse(*allow, spec, permissions::Source::Flag).unwrap());
        }
        out
    }

    #[test]
    fn background_commands_run_on_under_the_process_tool_and_end_with_the_run() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let server =
            "echo ready; i=0; while [ $i -lt 300 ]; do sleep 1; i=$((i+1)); echo tick $i; done";
        let p = Arc::new(FakeProvider::new(vec![
            // Quick enough to end within the wait: comes back whole.
            resp(calls(&[(
                "c1",
                "bash",
                json!({"command": "echo fast; exit 4", "background": true, "wait": 10}),
            )])),
            // A server: stays in the background.
            resp(calls(&[(
                "c2",
                "bash",
                json!({"command": server, "background": true, "wait": 1}),
            )])),
            // A plain command runs beside it; the sweep leaves it alone.
            resp(calls(&[("c3", "bash", json!({"command": "echo beside"}))])),
            resp(calls(&[("c4", "process", json!({"action": "list"}))])),
            resp(calls(&[(
                "c5",
                "process",
                json!({"action": "wait", "id": "bg-2", "seconds": 2}),
            )])),
            resp(calls(&[(
                "c6",
                "process",
                json!({"action": "output", "id": "bg-2"}),
            )])),
            resp(calls(&[(
                "c7",
                "process",
                json!({"action": "output", "id": "bg-9"}),
            )])),
            resp(calls(&[("c8", "process", json!({"action": "dance"}))])),
            // One more left running when the run ends.
            resp(calls(&[(
                "c9",
                "bash",
                json!({"command": "sleep 300", "background": true, "wait": 0}),
            )])),
            resp(calls(&[(
                "c10",
                "process",
                json!({"action": "stop", "id": "bg-2"}),
            )])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p);
        assert!(h.offers_background());
        let tools = h.tools();
        let bash = tools.iter().find(|t| t.name == "bash").unwrap();
        assert!(bash.parameters["properties"]["background"].is_object());
        assert!(bash.description.contains("background:true"));
        assert!(tools.iter().any(|t| t.name == PROCESS_TOOL));
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED, "{}", got.error);
        let replies = tool_replies(&h);
        let by_id = |id: &str| -> String {
            replies
                .iter()
                .find(|(i, _)| i == id)
                .map(|(_, t)| t.clone())
                .unwrap_or_else(|| panic!("no reply {id}: {replies:?}"))
        };
        let fast = by_id("c1");
        assert!(fast.contains("<returncode>4</returncode>"), "{fast}");
        assert!(
            fast.contains("fast\n[bg-1 ended on its own with exit 4 after"),
            "{fast}"
        );
        let started = by_id("c2");
        assert!(
            started.contains("[bg-2 is running in the background (pid "),
            "{started}"
        );
        assert!(started.contains("ready"), "{started}");
        assert!(by_id("c3").contains("beside"), "{}", by_id("c3"));
        let list = by_id("c4");
        assert!(list.contains("bg-2 running for"), "{list}");
        assert!(!list.contains("bg-1"), "{list}");
        let waited = by_id("c5");
        assert!(
            waited.contains("still running after waiting 2s"),
            "{waited}"
        );
        assert!(waited.contains("tick"), "{waited}");
        let output = by_id("c6");
        assert!(output.contains("[bg-2 `echo ready;"), "{output}");
        assert!(output.contains("running for"), "{output}");
        let unknown = by_id("c7");
        assert!(
            unknown.contains("bg-9: no such background process; the list has bg-2"),
            "{unknown}"
        );
        assert!(
            by_id("c8").contains("unknown action dance"),
            "{}",
            by_id("c8")
        );
        let stopped = by_id("c10");
        assert!(stopped.contains("[bg-2 `echo ready;"), "{stopped}");
        assert!(stopped.contains("stopped after"), "{stopped}");
        let steps = h.trajectory().steps;
        assert!(
            steps
                .iter()
                .any(|s| s.kind == StepKind::Command && s.command == format!("{server} &")),
            "the background start is a command step marked &"
        );
        assert!(
            steps
                .iter()
                .any(|s| s.kind == StepKind::Process && s.command == "wait bg-2 2s")
        );
        let note = steps
            .iter()
            .find(|s| {
                s.kind == StepKind::Note && s.content.starts_with("stopped 1 background process")
            })
            .expect("the run end stops what is left");
        assert!(note.content.contains("bg-3 pid"), "{}", note.content);
        assert!(note.content.contains("sleep 300"), "{}", note.content);
        assert!(h.procs().is_empty());
    }

    #[test]
    fn background_is_withheld_from_read_only_runs_and_fake_environments() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[(
                "c1",
                "bash",
                json!({"command": "sleep 5", "background": true}),
            )])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p);
        h.cfg.read_only = true;
        assert!(!h.offers_background());
        assert!(!h.tools().iter().any(|t| t.name == PROCESS_TOOL));
        assert!(
            h.tools()
                .iter()
                .find(|t| t.name == "bash")
                .unwrap()
                .parameters["properties"]["background"]
                .is_null()
        );
        h.run().unwrap();
        let reply = tool_replies(&h).remove(0).1;
        assert!(
            reply.contains("read-only run runs its commands to completion"),
            "{reply}"
        );

        let env = Arc::new(Mutex::new(FakeEnv::default()));
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[(
                "c1",
                "bash",
                json!({"command": "sleep 5", "background": true}),
            )])),
            resp(calls(&[("c2", "process", json!({"action": "list"}))])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = new(
            Box::new(SharedFakeProvider(p)),
            Box::new(env.clone()),
            base_config(),
            Vec::new(),
        );
        assert!(!h.offers_background());
        h.run().unwrap();
        let replies = tool_replies(&h);
        assert!(
            replies[0].1.contains("background is not offered"),
            "{}",
            replies[0].1
        );
        assert!(
            replies[1].1.contains("unknown tool process"),
            "{}",
            replies[1].1
        );
        assert!(env.lock().unwrap().commands.is_empty());
    }

    #[test]
    fn memory_tool_remembers_forgets_and_lists_in_the_home_files() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[(
                "c1",
                "memory",
                json!({"action": "remember", "text": "tests need cargo test -p app"}),
            )])),
            resp(calls(&[(
                "c2",
                "memory",
                json!({"action": "remember", "text": "prefers tabs"}),
            )])),
            resp(calls(&[("c3", "memory", json!({"action": "list"}))])),
            resp(calls(&[(
                "c4",
                "memory",
                json!({"action": "forget", "text": "tabs"}),
            )])),
            resp(calls(&[(
                "c5",
                "memory",
                json!({"action": "forget", "text": "nothing like this"}),
            )])),
            resp(calls(&[("c6", "memory", json!({"action": "dance"}))])),
            // A rule can refuse one scope.
            resp(calls(&[(
                "c7",
                "memory",
                json!({"action": "remember", "scope": "user", "text": "secret"}),
            )])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p.clone());
        h.set_memory(Memory::new(home.path().to_str().unwrap(), "acme-app"));
        h.set_permissions(rules(&[(false, "memory(user)")]));
        assert!(h.offers_memory());
        assert!(h.tools().iter().any(|t| t.name == MEMORY_TOOL));
        assert!(
            h.environment()
                .iter()
                .any(|l| l.starts_with("memory: 0 for the project"))
        );
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED, "{}", got.error);
        let replies = tool_replies(&h);
        let by_id = |id: &str| -> String {
            replies
                .iter()
                .find(|(i, _)| i == id)
                .map(|(_, t)| t.clone())
                .unwrap_or_else(|| panic!("no reply {id}: {replies:?}"))
        };
        // The path is absolute, or shortened to `~/...` when the home lives
        // under $HOME (the runner's TMPDIR does).
        assert!(
            by_id("c1").contains("remembered for the project (1 memory in /")
                || by_id("c1").contains("remembered for the project (1 memory in ~/"),
            "{}",
            by_id("c1")
        );
        assert!(
            by_id("c1").contains("/memory/acme-app.md)"),
            "{}",
            by_id("c1")
        );
        assert!(
            by_id("c2").contains("remembered for the project (2 memories in"),
            "{}",
            by_id("c2")
        );
        let list = by_id("c3");
        assert!(
            list.contains("project memory (") && list.contains("2 memories"),
            "{list}"
        );
        assert!(list.contains("] tests need cargo test -p app"), "{list}");
        assert!(list.contains("] prefers tabs"), "{list}");
        assert!(
            list.contains("user memory (") && list.contains("nothing yet"),
            "{list}"
        );
        assert!(
            by_id("c4").contains("forgotten from the project (1 memory left"),
            "{}",
            by_id("c4")
        );
        assert!(
            by_id("c5").contains("nothing remembered contains"),
            "{}",
            by_id("c5")
        );
        assert!(by_id("c6").contains("unknown action"), "{}", by_id("c6"));
        assert!(
            by_id("c7").contains("refused by permission rule `deny memory(user)`"),
            "{}",
            by_id("c7")
        );
        let project = fs::read_to_string(home.path().join("memory/acme-app.md")).unwrap();
        assert!(project.starts_with("# Memory for acme-app\n"), "{project}");
        assert!(
            project.contains("] tests need cargo test -p app\n"),
            "{project}"
        );
        assert!(!project.contains("tabs"), "{project}");
        assert!(
            !home.path().join("memory/user.md").exists(),
            "the denied call wrote nothing"
        );
        let steps = h.trajectory().steps;
        assert_eq!(
            steps.iter().filter(|s| s.kind == StepKind::Memory).count(),
            5,
            "a memory step per run action; the denied one and the bad one leave none"
        );
        assert!(
            steps.iter().any(
                |s| s.kind == StepKind::Memory && s.command == "remember project: prefers tabs"
            )
        );
        assert_eq!(
            h.memory()
                .unwrap()
                .entries(crate::memory::Scope::Project)
                .len(),
            1
        );
    }

    #[test]
    fn memory_is_withheld_from_read_only_runs_and_agents_without_one() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[(
                "c1",
                "memory",
                json!({"action": "remember", "text": "x"}),
            )])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p);
        h.set_memory(Memory::new(home.path().to_str().unwrap(), "acme-app"));
        h.cfg.read_only = true;
        assert!(!h.offers_memory());
        assert!(!h.tools().iter().any(|t| t.name == MEMORY_TOOL));
        h.run().unwrap();
        let reply = tool_replies(&h).remove(0).1;
        assert!(reply.contains("unknown tool memory"), "{reply}");
        assert!(!home.path().join("memory").exists());
        // /memory still works on the side: the files are the session's.
        assert!(h.memory().is_some());

        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[("c1", "memory", json!({"action": "list"}))])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = new(
            Box::new(SharedFakeProvider(p)),
            Box::new(Arc::new(Mutex::new(FakeEnv::default()))),
            base_config(),
            Vec::new(),
        );
        assert!(!h.offers_memory());
        assert!(h.environment().iter().any(|l| l == "memory: off"));
        h.run().unwrap();
        let reply = tool_replies(&h).remove(0).1;
        assert!(reply.contains("unknown tool memory"), "{reply}");
    }

    #[test]
    fn web_search_tool_answers_with_numbered_results_and_runs_in_a_batch() {
        use crate::search::test_support::{ddg_page, http, serve};
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "alpha\n").unwrap();
        let page = ddg_page(
            &[
                (
                    "read_to_string in std::fs - Rust",
                    "https://doc.rust-lang.org/std/fs/fn.read_to_string.html",
                    "Reads the entire contents of a file into a string.",
                ),
                (
                    "std::fs - Rust",
                    "https://doc.rust-lang.org/std/fs/",
                    "File system.",
                ),
            ],
            true,
        );
        let (base, seen) = serve(vec![
            http("200 OK", "text/html", &page),
            http("200 OK", "text/html", &ddg_page(&[], false)),
            http("200 OK", "text/html", &page),
        ]);
        let p = Arc::new(FakeProvider::new(vec![
            // A lone search, then a search batched beside a read, then one
            // the engine answers with nothing, then bad arguments.
            resp(calls(&[(
                "s1",
                "web_search",
                json!({"query": "  rust read_to_string\n  ", "count": 2}),
            )])),
            resp(calls(&[
                ("r1", "read", json!({"path": "a.txt"})),
                ("s2", "web_search", json!({"query": "nothing here"})),
            ])),
            resp(calls(&[("s3", "web_search", json!({"query": "rust fs"}))])),
            resp(calls(&[("s4", "web_search", json!({"query": "   "}))])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p);
        assert!(!h.offers_web_search());
        assert!(h.environment().iter().any(|l| l == "web search: off"));
        h.set_web_search(WebSearch::duckduckgo(&format!("{base}/html/")));
        assert!(h.offers_web_search());
        assert!(
            h.environment()
                .iter()
                .any(|l| l == "web search: duckduckgo")
        );
        assert!(h.tools().iter().any(|t| t.name == WEB_SEARCH_TOOL));
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED, "{got:?}");
        assert_eq!(got.steps, 4, "bad arguments never become a step");
        let replies = tool_replies(&h);
        let ids: Vec<&str> = replies.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["s1", "r1", "s2", "s3", "s4"]);
        let by_id: BTreeMap<String, String> = replies.into_iter().collect();
        assert!(
            by_id["s1"].contains(
                "2 results for \"rust read_to_string\" from duckduckgo:\n\n1. read_to_string in std::fs - Rust\n   https://doc.rust-lang.org/std/fs/fn.read_to_string.html\n   Reads the entire contents of a file into a string.\n\n2. std::fs - Rust\n   https://doc.rust-lang.org/std/fs/\n   File system."
            ),
            "{}",
            by_id["s1"]
        );
        assert!(by_id["r1"].contains("alpha"), "{}", by_id["r1"]);
        assert!(
            by_id["s2"]
                .contains("no results for \"nothing here\" from duckduckgo; try other words"),
            "{}",
            by_id["s2"]
        );
        assert!(
            by_id["s3"].contains("2 results for \"rust fs\" from duckduckgo:"),
            "{}",
            by_id["s3"]
        );
        assert!(
            by_id["s4"].contains("web_search: the query is empty"),
            "{}",
            by_id["s4"]
        );
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(
            requests[0].starts_with("GET /html/?q=rust+read_to_string HTTP/1.1"),
            "{}",
            requests[0]
        );
        assert!(
            requests[0].contains("pulse-harness/"),
            "a user agent names the harness: {}",
            requests[0]
        );
        // The batched search is announced before the batch answers; its
        // step and observation carry the query.
        let steps = &h.trajectory().steps;
        let kinds: Vec<StepKind> = steps.iter().map(|s| s.kind.clone()).collect();
        let first_batch_obs = kinds
            .iter()
            .enumerate()
            .filter(|(_, k)| **k == StepKind::Observation)
            .nth(1)
            .map(|(i, _)| i)
            .unwrap();
        assert_eq!(
            &kinds[first_batch_obs - 2..first_batch_obs],
            &[StepKind::Read, StepKind::Search],
            "{kinds:?}"
        );
        let searches: Vec<(&str, &str)> = steps
            .iter()
            .filter(|s| s.kind == StepKind::Search)
            .map(|s| (s.content.as_str(), s.command.as_str()))
            .collect();
        assert_eq!(
            searches,
            [
                ("rust read_to_string", "rust read_to_string"),
                ("nothing here", "nothing here"),
                ("rust fs", "rust fs"),
            ]
        );
        assert_eq!(kinds.iter().filter(|k| **k == StepKind::Search).count(), 3);
        let search_obs: Vec<&str> = steps
            .iter()
            .filter(|s| s.kind == StepKind::Observation && s.command == "web_search tool")
            .map(|s| s.content.as_str())
            .collect();
        assert_eq!(search_obs.len(), 3, "{search_obs:?}");
        assert!(
            search_obs[1].starts_with("no results for"),
            "{}",
            search_obs[1]
        );
        // Only the bad arguments make an error step; an empty answer is a
        // plain observation.
        assert_eq!(
            kinds.iter().filter(|k| **k == StepKind::Error).count(),
            1,
            "{kinds:?}"
        );
        assert_eq!(kinds[kinds.len() - 3], StepKind::Error, "{kinds:?}");
    }

    #[test]
    fn web_search_names_engine_failures_and_is_denied_by_rule() {
        use crate::search::test_support::{http, serve};
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let (base, _seen) = serve(vec![http("503 Service Unavailable", "text/plain", "later")]);
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[("s1", "web_search", json!({"query": "rust"}))])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p);
        h.set_web_search(WebSearch::duckduckgo(&format!("{base}/html/")));
        h.run().unwrap();
        let reply = tool_replies(&h).remove(0).1;
        assert!(
            reply.starts_with("<returncode>1</returncode>")
                && reply.contains("web_search: HTTP 503 from duckduckgo: later"),
            "{reply}"
        );
        let kinds: Vec<String> = h.steps.iter().map(|s| s.kind.to_string()).collect();
        assert_eq!(
            kinds,
            vec!["assistant", "search", "observation", "assistant", "exit"],
            "{kinds:?}"
        );

        // A deny rule on the tool withdraws it; one on a query refuses that
        // query alone.
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[("s1", "web_search", json!({"query": "rust"}))])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p);
        h.set_web_search(WebSearch::duckduckgo(&format!("{base}/html/")));
        h.set_permissions(rules(&[(false, "web_search")]));
        assert!(!h.tools().iter().any(|t| t.name == WEB_SEARCH_TOOL));
        h.run().unwrap();
        let reply = tool_replies(&h).remove(0).1;
        assert!(
            reply.contains("refused by permission rule `deny web_search`"),
            "{reply}"
        );

        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[(
                "s1",
                "web_search",
                json!({"query": "company secrets"}),
            )])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p);
        h.set_web_search(WebSearch::duckduckgo(&format!("{base}/html/")));
        h.set_permissions(rules(&[(false, "web_search(company secrets)")]));
        assert!(h.tools().iter().any(|t| t.name == WEB_SEARCH_TOOL));
        h.run().unwrap();
        let reply = tool_replies(&h).remove(0).1;
        assert!(
            reply.contains("refused by permission rule `deny web_search(company secrets)`"),
            "{reply}"
        );
    }

    #[test]
    fn web_search_stays_in_read_only_runs_and_is_unknown_when_off() {
        use crate::search::test_support::{ddg_page, http, serve};
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let (base, _seen) = serve(vec![http(
            "200 OK",
            "text/html",
            &ddg_page(
                &[("Rust", "https://www.rust-lang.org/", "A language.")],
                false,
            ),
        )]);
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[("s1", "web_search", json!({"query": "rust"}))])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = local_agent(dir.path(), p);
        h.cfg.read_only = true;
        h.set_web_search(WebSearch::duckduckgo(&format!("{base}/html/")));
        assert!(
            h.offers_web_search(),
            "reading the web changes nothing here"
        );
        h.run().unwrap();
        let reply = tool_replies(&h).remove(0).1;
        assert!(
            reply.contains("1 result for \"rust\" from duckduckgo:\n\n1. Rust\n   https://www.rust-lang.org/\n   A language."),
            "{reply}"
        );

        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[("s1", "web_search", json!({"query": "rust"}))])),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = new(
            Box::new(SharedFakeProvider(p)),
            Box::new(Arc::new(Mutex::new(FakeEnv::default()))),
            base_config(),
            Vec::new(),
        );
        assert!(!h.offers_web_search());
        h.run().unwrap();
        let reply = tool_replies(&h).remove(0).1;
        assert!(reply.contains("unknown tool web_search"), "{reply}");

        // A subagent searches through its parent's engine.
        let (base, seen) = serve(vec![http(
            "200 OK",
            "text/html",
            &ddg_page(
                &[("Rust", "https://www.rust-lang.org/", "A language.")],
                false,
            ),
        )]);
        let mut cfg = base_config();
        cfg.subagents = true;
        cfg.max_steps = 12;
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[(
                "sub",
                SUBAGENT_TOOL,
                json!({"task": "find the rust site", "max_steps": 5}),
            )])),
            resp(calls(&[("s1", "web_search", json!({"query": "rust"}))])),
            resp(msg("SUMMARY: found it")),
            resp(msg("SUMMARY: done")),
        ]));
        let mut h = new(
            Box::new(SharedFakeProvider(p)),
            Box::new(Arc::new(Mutex::new(FakeEnv::default()))),
            cfg,
            Vec::new(),
        );
        h.set_web_search(WebSearch::duckduckgo(&format!("{base}/html/")));
        h.run().unwrap();
        assert_eq!(seen.lock().unwrap().len(), 1, "the child searched");
        let reply = tool_replies(&h).remove(0).1;
        assert!(reply.contains("found it"), "{reply}");
    }

    #[test]
    fn permission_deny_rules_refuse_calls_and_withdraw_tools() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(dir.path().join(".env"), "SECRET=1\n").unwrap();
        fs::write(dir.path().join("a.txt"), "old line\n").unwrap();
        let p = Arc::new(FakeProvider::new(vec![
            // A typed call, then the same command as a fenced block.
            resp(tool("rm -rf a.txt")),
            resp(msg("```bash\nrm -rf a.txt\n```")),
            // A write though the tool is withdrawn; a batch of reads with
            // one denied path.
            resp(calls(&[
                ("w1", "write", json!({"path":"b.txt","content":"x\n"})),
                ("r1", "read", json!({"path":".env"})),
                ("r2", "read", json!({"path":"./a.txt"})),
            ])),
            resp(tool("echo fine")),
            resp(msg("SUMMARY: done")),
        ]));
        let steps = Arc::new(Mutex::new(Vec::new()));
        let sink_steps = steps.clone();
        let mut h = local_agent(dir.path(), p.clone());
        h.sinks.push(Box::new(SinkFunc(move |s| {
            sink_steps.lock().unwrap().push(s);
        })));
        h.set_permissions(rules(&[
            (false, "bash(rm *)"),
            (false, "write"),
            (false, "read(.env*)"),
            (true, "bash(rm -rf a.txt)"),
        ]));
        let got = h.run().unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED, "{got:?}");
        assert!(dir.path().join("a.txt").exists(), "the rm never ran");
        assert!(!dir.path().join("b.txt").exists(), "the write never ran");
        let reqs = p.requests();
        assert!(
            !reqs[0].tools.iter().any(|t| t.name == "write"),
            "a tool denied whole is not offered: {:?}",
            reqs[0].tools.iter().map(|t| &t.name).collect::<Vec<_>>()
        );
        assert!(reqs[0].tools.iter().any(|t| t.name == "edit"));
        let all = tool_replies(&h);
        assert!(
            all[0]
                .1
                .contains("refused by permission rule `deny bash(rm *)`"),
            "deny beats allow: {}",
            all[0].1
        );
        assert!(all.last().unwrap().1.contains("fine"), "{all:?}");
        let replies: BTreeMap<String, String> = all.into_iter().collect();
        assert!(
            replies["w1"].contains("refused by permission rule `deny write`"),
            "{}",
            replies["w1"]
        );
        assert!(
            replies["r1"].contains("refused by permission rule `deny read(.env*)`"),
            "{}",
            replies["r1"]
        );
        assert!(replies["r2"].contains("old line"), "{}", replies["r2"]);
        let fenced = h
            .messages()
            .iter()
            .filter(|m| m.role == Role::User)
            .map(|m| m.content.clone())
            .find(|c| c.contains("refused by permission rule"))
            .unwrap_or_default();
        assert!(
            fenced.contains("deny bash(rm *)"),
            "fenced blocks pass the same gate: {fenced}"
        );
        let errors = steps
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.kind == StepKind::Error && s.content.contains("permission rule"))
            .count();
        assert_eq!(errors, 4, "{:?}", steps.lock().unwrap());
        assert!(
            h.environment()
                .iter()
                .any(|l| l.contains("permissions: deny bash(rm *) (flag); deny write (flag)")),
            "{:?}",
            h.environment()
        );
    }

    #[test]
    fn permission_allow_rules_skip_approval_and_always_adds_a_session_rule() {
        let _serial = crate::test_helpers::serial();
        let dir = chat_dir();
        let p = Arc::new(FakeProvider::new(vec![
            resp(tool("git add a.txt")),
            resp(tool("git commit -m one")),
            resp(tool("git commit --amend --no-edit")),
            resp(calls(&[(
                "e1",
                "edit",
                json!({"path":"a.txt","search":"old line","replace":"new line"}),
            )])),
            resp(calls(&[(
                "e2",
                "edit",
                json!({"path":"a.txt","search":"new line","replace":"newer line"}),
            )])),
            resp(msg("done")),
        ]));
        let ui = Arc::new(Mutex::new(ScriptUi {
            decisions: vec![Approval::Always, Approval::Always],
            ..ScriptUi::default()
        }));
        let steps = Arc::new(Mutex::new(Vec::new()));
        let mut h = chat_agent(dir.path(), p.clone(), Some(ui.clone()), steps.clone());
        h.set_permissions(rules(&[(true, "bash(git add *)")]));
        let cancel = CancelToken::new();
        let turn = h.turn(&cancel, "commit a.txt").unwrap();
        assert!(matches!(&turn, Turn::Reply(t) if t == "done"), "{turn:?}");
        let seen = ui.lock().unwrap().seen.clone();
        let summaries: Vec<&str> = seen.iter().map(|r| r.summary.as_str()).collect();
        assert_eq!(
            summaries,
            vec!["git commit -m one", "a.txt"],
            "the allowed add and the calls covered by `always` ask nobody"
        );
        assert_eq!(seen[0].always, "bash(git commit *)");
        assert_eq!(seen[1].always, "edit");
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "newer line\n"
        );
        let described = h.permissions().describe();
        assert_eq!(
            described,
            vec![
                "allow bash(git add *) (flag)".to_string(),
                "allow bash(git commit *) (session)".to_string(),
                "allow edit (session)".to_string(),
            ]
        );
        assert_eq!(
            steps
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.kind == StepKind::Note && s.content.starts_with("permission: allow"))
                .count(),
            2
        );
        assert_eq!(h.clear_session_permissions(), 2);
        assert_eq!(h.permissions().len(), 1);
    }

    #[test]
    fn chat_question_tool_asks_the_user_and_relays_the_answer() {
        let _serial = crate::test_helpers::serial();
        let dir = chat_dir();
        let p = Arc::new(FakeProvider::new(vec![
            resp(calls(&[(
                "q1",
                "question",
                json!({"question":"tabs or spaces?","choices":["tabs","spaces"]}),
            )])),
            resp(calls(&[("q2", "question", json!({"question":"sure?"}))])),
            resp(calls(&[("q3", "question", json!({"question":""}))])),
            resp(msg("spaces then")),
        ]));
        let ui = Arc::new(Mutex::new(ScriptUi {
            answers: vec![Some("spaces".into()), None],
            ..ScriptUi::default()
        }));
        let steps = Arc::new(Mutex::new(Vec::new()));
        let mut h = chat_agent(dir.path(), p.clone(), Some(ui.clone()), steps.clone());
        assert!(h.tool_names().iter().any(|n| n == QUESTION_TOOL));
        let cancel = CancelToken::new();
        let turn = h.turn(&cancel, "format the file").unwrap();
        assert!(
            matches!(&turn, Turn::Reply(t) if t == "spaces then"),
            "{turn:?}"
        );
        let replies: BTreeMap<String, String> = tool_replies(&h).into_iter().collect();
        assert!(
            replies["q1"].contains("The user answered: spaces"),
            "{}",
            replies["q1"]
        );
        assert!(
            replies["q2"].contains("did not answer"),
            "{}",
            replies["q2"]
        );
        assert!(
            replies["q3"].contains("string question"),
            "{}",
            replies["q3"]
        );
        let asked = ui.lock().unwrap().asked.clone();
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[0].0, "tabs or spaces?");
        assert_eq!(asked[0].1, vec!["tabs".to_string(), "spaces".to_string()]);
        assert!(
            steps
                .lock()
                .unwrap()
                .iter()
                .any(|s| s.kind == StepKind::Note
                    && s.content == "question: tabs or spaces?\ntabs | spaces"),
        );
        assert!(
            p.requests()[0]
                .tools
                .iter()
                .any(|t| t.name == QUESTION_TOOL),
            "the question tool is offered when a ui is set"
        );
    }

    #[test]
    fn chat_resumes_an_earlier_conversation() {
        let _serial = crate::test_helpers::serial();
        let dir = chat_dir();
        let p = Arc::new(FakeProvider::new(vec![resp(msg("still blue"))]));
        let steps = Arc::new(Mutex::new(Vec::new()));
        let mut h = chat_agent(dir.path(), p.clone(), None, steps.clone());
        let plain = |role: Role, content: &str| Message {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: String::new(),
            name: String::new(),
            ..Default::default()
        };
        h.resume_messages(vec![
            plain(Role::System, "an old prompt"),
            plain(Role::User, "the sky is blue"),
            plain(Role::Assistant, "noted"),
        ])
        .unwrap();
        let cancel = CancelToken::new();
        let turn = h.turn(&cancel, "what colour was it?").unwrap();
        assert!(
            matches!(&turn, Turn::Reply(t) if t == "still blue"),
            "{turn:?}"
        );
        let req = p.requests().remove(0);
        let roles: Vec<Role> = req.messages.iter().map(|m| m.role.clone()).collect();
        assert_eq!(
            roles,
            vec![
                Role::System,
                Role::User,
                Role::Assistant,
                Role::User,
                Role::User
            ],
            "{:?}",
            req.messages
        );
        assert_ne!(
            req.messages[0].content, "an old prompt",
            "the prompt is rendered afresh"
        );
        assert_eq!(req.messages[1].content, "the sky is blue");
        assert_eq!(req.messages[2].content, "noted");
        assert!(
            req.messages[3]
                .content
                .contains("resumed an earlier session: 2 messages"),
            "{}",
            req.messages[3].content
        );
        assert_eq!(req.messages[4].content, "what colour was it?");
        assert_eq!(
            h.cfg.task, "the sky is blue",
            "compaction's task line comes from the first message"
        );
        assert!(
            h.resume_messages(Vec::new()).is_err(),
            "no resuming after the first turn"
        );
    }

    #[test]
    fn chat_turn_requires_interactive_and_text() {
        let _serial = crate::test_helpers::serial();
        let dir = chat_dir();
        let p = Arc::new(FakeProvider::new(vec![]));
        let steps = Arc::new(Mutex::new(Vec::new()));
        let mut h = chat_agent(dir.path(), p.clone(), None, steps.clone());
        let cancel = CancelToken::new();
        assert!(h.turn(&cancel, "   ").is_err());
        h.cfg.interactive = false;
        assert!(h.turn(&cancel, "hi").is_err());
        // Ending an unstarted session is quiet.
        let got = h.end(EXIT_ENDED);
        assert_eq!(got.exit_status, EXIT_ENDED);
        assert_eq!(got.calls, 0);
    }

    #[test]
    fn job_runs_are_unchanged_by_the_turn_machinery() {
        let _serial = crate::test_helpers::serial();
        let p = Arc::new(FakeProvider::new(vec![
            resp(tool("true")),
            resp(msg("no report here")),
            resp(msg("SUMMARY: done")),
        ]));
        let (h, got) = run_agent(
            p.clone(),
            Arc::new(Mutex::new(FakeEnv {
                default_output: "ok\n".into(),
                ..FakeEnv::default()
            })),
            base_config(),
        )
        .unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        assert_eq!(got.calls, 3);
        assert_eq!(
            got.format_errors, 1,
            "a plain reply is still a format error in a job"
        );
        assert!(h.is_ended());
        let cancel = CancelToken::new();
        let mut h = h;
        assert!(
            h.turn(&cancel, "more").is_err(),
            "a job agent takes no turns"
        );
    }
}
