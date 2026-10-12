use anyhow::anyhow;
use pulse_provider::{CancelToken, Message, Provider, Request, Role, Tool, ToolCall, Usage};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
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
    Tool {
        name: "bash".to_string(),
        description: "Run one non-interactive /bin/sh command in the working directory. Each call is a fresh process; chain setup and use with &&.".to_string(),
        parameters: json!({"type":"object","properties":{"command":{"type":"string","description":"The shell command to run with /bin/sh -c."}},"required":["command"]}),
    }
}

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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepKind {
    Assistant,
    Command,
    Read,
    Grep,
    Edit,
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
            Self::Edit => "edit",
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub duration_ms: i64,
    pub usage: Usage,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
}

impl Default for Step {
    fn default() -> Self {
        Self {
            n: 0,
            at: String::new(),
            kind: StepKind::Note,
            content: String::new(),
            command: String::new(),
            exit_code: None,
            duration_ms: 0,
            usage: Usage::default(),
            model: String::new(),
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
    #[serde(skip)]
    pub elapsed: Duration,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

pub type Result = ResultSummary;

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
}

pub struct Agent {
    p: Box<dyn Provider>,
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
    /// Set by a command step when the run cannot go on (the working
    /// directory is gone); the loop ends the run with it before the next
    /// model call.
    fatal: Option<(&'static str, String)>,
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
        fatal: None,
    }
}

pub use new as New;

impl Agent {
    pub fn run(&mut self) -> anyhow::Result<ResultSummary> {
        self.run_with_cancel(&CancelToken::new())
    }

    pub fn run_with_cancel(&mut self, cancel: &CancelToken) -> anyhow::Result<ResultSummary> {
        if self.ran {
            return Err(anyhow!("harness agent can only run once"));
        }
        self.ran = true;
        self.cfg = apply_defaults(self.cfg.clone());
        if self.cfg.model.trim().is_empty() {
            return Err(anyhow!("harness model is empty"));
        }
        if self.cfg.task.trim().is_empty() {
            return Err(anyhow!("harness task is empty"));
        }
        self.started = Instant::now();
        if self.cfg.system_prompt.is_empty() {
            let tmpl = if self.cfg.system_prompt_template.is_empty() {
                DEFAULT_PROMPT_TEMPLATE
            } else {
                &self.cfg.system_prompt_template
            };
            self.cfg.system_prompt = render_prompt(tmpl, prompt_data(&self.env.info(), &self.cfg))?;
        }
        self.compile_deny()?;
        self.msgs = vec![
            Message {
                role: Role::System,
                content: self.cfg.system_prompt.clone(),
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
            },
            Message {
                role: Role::User,
                content: self.cfg.task.clone(),
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
            },
        ];
        let mut initiator = "user".to_string();
        let mut counted_format_errors = 0;
        let mut used_format_retries = 0;
        let mut keep_observations = self.cfg.keep_observations;
        let mut last_prompt_tokens = 0;
        let mut last_prune_level = 0;
        let mut budget_notice_sent = false;
        let mut emitted_prune_notes = BTreeSet::new();
        loop {
            if let Some(status) = self.limit_status(cancel) {
                let msg = self.limit_error(&status);
                return Ok(self.finish(&status, "", &msg));
            }
            if self.result.calls > 0 {
                let mut level = 1;
                if self.cfg.context_window > 0
                    && last_prompt_tokens > (0.8 * f64::from(self.cfg.context_window)) as i32
                {
                    level = 2;
                    if last_prune_level == 2 && keep_observations > 1 {
                        keep_observations = 1.max(keep_observations / 2);
                    }
                }
                let (next, changed) = prune_history(&self.msgs, keep_observations, level);
                self.msgs = next;
                let key = format!("{level}/{keep_observations}");
                if changed > 0 && !emitted_prune_notes.contains(&key) {
                    self.emit(Step {
                        kind: StepKind::Note,
                        content: format!(
                            "collapsed {changed} older observations to fit the context window (level {level}, keep {keep_observations})"
                        ),
                        ..Step::default()
                    });
                    emitted_prune_notes.insert(key);
                }
                last_prune_level = level;
            }
            if !budget_notice_sent && let Some(notice) = self.budget_notice() {
                self.msgs.push(Message {
                    role: Role::User,
                    content: notice.clone(),
                    tool_calls: Vec::new(),
                    tool_call_id: String::new(),
                    name: String::new(),
                });
                self.emit(Step {
                    kind: StepKind::Note,
                    content: notice,
                    ..Step::default()
                });
                budget_notice_sent = true;
            }
            let req = Request {
                model: self.cfg.model.clone(),
                messages: clone_messages(&self.msgs),
                tools: vec![bash_tool()],
                initiator: initiator.clone(),
                ..Request::default()
            };
            let resp = self.p.chat(cancel, req);
            self.result.calls += 1;
            let resp = match resp {
                Ok(r) => r,
                Err(err) => {
                    if !self.cfg.wall_time.is_zero() && self.started.elapsed() >= self.cfg.wall_time
                    {
                        let msg = self.limit_error(EXIT_TIME_LIMIT);
                        return Ok(self.finish(EXIT_TIME_LIMIT, "", &msg));
                    }
                    if cancel.is_cancelled() {
                        self.cancel_reason = if self.cfg.cancel_reason.is_empty() {
                            "interrupted: context canceled".to_string()
                        } else {
                            self.cfg.cancel_reason.clone()
                        };
                        let msg = self.limit_error(EXIT_CANCELLED);
                        self.emit(Step {
                            kind: StepKind::Error,
                            content: msg.clone(),
                            ..Step::default()
                        });
                        return Ok(self.finish(EXIT_CANCELLED, "", &msg));
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
                    return Ok(self.finish(status, "", &msg));
                }
            };
            initiator = "agent".to_string();
            self.result.usage.add(resp.usage);
            last_prompt_tokens = resp.usage.prompt_tokens;
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
                usage: resp.usage,
                model,
                ..Step::default()
            });
            let free_format_retry = used_format_retries < self.cfg.format_retries.max(0);
            let (clean, submitted, submission, prose_only) =
                self.handle_assistant(cancel, msg, free_format_retry);
            if let Some((status, msg)) = self.fatal.take() {
                return Ok(self.finish(status, "", &msg));
            }
            if submitted {
                return Ok(self.finish(EXIT_SUBMITTED, &submission, ""));
            }
            if clean {
                counted_format_errors = 0;
                used_format_retries = 0;
            } else {
                self.result.format_errors += 1;
                if prose_only && free_format_retry {
                    used_format_retries += 1;
                    self.result.calls = self.result.calls.saturating_sub(1);
                    continue;
                }
                counted_format_errors += 1;
                if counted_format_errors >= self.cfg.max_format_errors {
                    return Ok(self.finish(EXIT_FORMAT_ERROR, "", ""));
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

    fn limit_status(&mut self, cancel: &CancelToken) -> Option<String> {
        if !self.cfg.cancel_file.is_empty() && Path::new(&self.cfg.cancel_file).exists() {
            self.cancel_reason = ERR_OWNER_CANCELLED.to_string();
            return Some(EXIT_CANCELLED.to_string());
        }
        if cancel.is_cancelled() {
            self.cancel_reason = if self.cfg.cancel_reason.is_empty() {
                "interrupted: context canceled".to_string()
            } else {
                self.cfg.cancel_reason.clone()
            };
            return Some(EXIT_CANCELLED.to_string());
        }
        if self.result.calls >= self.cfg.max_steps {
            return Some(EXIT_STEP_LIMIT.to_string());
        }
        if !self.cfg.wall_time.is_zero() && self.started.elapsed() >= self.cfg.wall_time {
            return Some(EXIT_TIME_LIMIT.to_string());
        }
        if self.cfg.max_cost > 0.0 && self.result.usage.cost >= self.cfg.max_cost {
            return Some(EXIT_COST_LIMIT.to_string());
        }
        None
    }

    fn budget_notice(&self) -> Option<String> {
        if self.cfg.max_steps < 10 {
            return None;
        }
        let reserve = 5.min(self.cfg.max_steps / 10);
        let remaining = self.cfg.max_steps - self.result.calls;
        if remaining != reserve {
            return None;
        }
        Some(format!(
            "Budget notice: {remaining} model calls remain of {}. Stop exploring. If you changed files, run the targeted check and commit finished work now, then reply with no command and your final report starting with SUMMARY: (use SUMMARY: blocked: or SUMMARY: failed: if unfinished). Do not start new work.",
            self.cfg.max_steps
        ))
    }

    fn limit_error(&self, status: &str) -> String {
        match status {
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
            EXIT_CANCELLED => self.cancel_reason.clone(),
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
            let mut all_clean = true;
            for call in msg.tool_calls {
                if call.name != "bash" {
                    let text = format!("unknown tool {}; only bash exists", call.name);
                    self.msgs.push(tool_message(&call, &text));
                    self.emit(Step {
                        kind: StepKind::Error,
                        content: text,
                        ..Step::default()
                    });
                    all_clean = false;
                    continue;
                }
                let Some(command) = parse_command(&call.arguments) else {
                    let text =
                        "tool arguments must be a JSON object with a string command".to_string();
                    self.msgs.push(Message {
                        role: Role::Tool,
                        tool_call_id: call.id,
                        name: "bash".to_string(),
                        content: text.clone(),
                        tool_calls: Vec::new(),
                    });
                    self.emit(Step {
                        kind: StepKind::Error,
                        content: text,
                        ..Step::default()
                    });
                    all_clean = false;
                    continue;
                };
                let (obs_text, _obs, submit, sub) = self.run_command(cancel, &command);
                self.msgs.push(Message {
                    role: Role::Tool,
                    tool_call_id: call.id,
                    name: "bash".to_string(),
                    content: obs_text,
                    tool_calls: Vec::new(),
                });
                if submit {
                    return (true, true, sub, false);
                }
            }
            return (all_clean, false, String::new(), false);
        }
        let actions = action_blocks(&msg.content);
        if actions.len() == 1 {
            let (obs_text, submit, sub) = match &actions[0] {
                ActionBlock::Command(command) => {
                    let (text, _, submit, sub) = self.run_command(cancel, command);
                    (text, submit, sub)
                }
                ActionBlock::Read(spec) => self.run_native_action(NativeAction::Read, spec),
                ActionBlock::Grep(spec) => self.run_native_action(NativeAction::Grep, spec),
                ActionBlock::Edit(spec) => self.run_native_action(NativeAction::Edit, spec),
            };
            self.msgs.push(Message {
                role: Role::User,
                content: obs_text,
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
            });
            if submit {
                return (true, true, sub, false);
            }
            return (true, false, String::new(), false);
        }
        if actions.len() > 1 {
            let text =
                "write exactly one fenced bash/read/grep/edit block per message, or your final report with no command"
                    .to_string();
            self.msgs.push(Message {
                role: Role::User,
                content: text.clone(),
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
            });
            self.emit(Step {
                kind: StepKind::Error,
                content: text,
                ..Step::default()
            });
            return (false, false, String::new(), false);
        }
        if is_report(&msg.content) {
            return (true, true, msg.content, false);
        }
        let text = "Your message had no command and no report. Either call the bash tool (or write one ```bash block) to keep working, or finish with your report: a `SUMMARY:` line plus any `FINDING:`/`SUGGESTION:`/`QUESTION:`/`VOTE:` lines.".to_string();
        self.msgs.push(Message {
            role: Role::User,
            content: text.clone(),
            tool_calls: Vec::new(),
            tool_call_id: String::new(),
            name: String::new(),
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

    fn run_command(
        &mut self,
        cancel: &CancelToken,
        command: &str,
    ) -> (String, Observation, bool, String) {
        self.result.steps += 1;
        self.emit(Step {
            kind: StepKind::Command,
            content: command.to_string(),
            command: command.to_string(),
            ..Step::default()
        });
        if let Some(rule) = self.denied(command) {
            let obs = Observation {
                output: format!(
                    "refused by policy: {}. Never push, never add remotes, never call gh, never touch credentials (job rule 3).",
                    rule.description
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
        let gate = self.verify_gate_for(command);
        let timeout = match &gate {
            Some(_) => self.commit_deadline(),
            None => self.cfg.step_timeout,
        };
        let obs = self.env.execute(cancel, command, timeout);
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

    fn denied(&self, command: &str) -> Option<&DenyRule> {
        self.deny.iter().find(|r| r.re.is_match(command))
    }

    /// True when the configured working directory existed at start and is
    /// gone now. Exit 127 alone is not enough: `command not found` uses it too.
    fn sandbox_lost(&self) -> bool {
        !self.cfg.cwd.is_empty() && !Path::new(&self.cfg.cwd).is_dir()
    }

    fn run_native_action(&mut self, action: NativeAction, spec: &str) -> (String, bool, String) {
        self.result.steps += 1;
        self.emit(Step {
            kind: action.step_kind(),
            content: spec.to_string(),
            command: spec.to_string(),
            ..Step::default()
        });
        let obs = match action {
            NativeAction::Read => self.native_read(spec),
            NativeAction::Grep => self.native_grep(spec),
            NativeAction::Edit => self.native_edit(spec),
        };
        let command = format!("{} action", action.name());
        self.emit_observation(&command, &obs);
        let formatted = format_observation(&obs, self.cfg.output_limit);
        (formatted, false, String::new())
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
                .saturating_sub(self.started.elapsed())
                .saturating_sub(Duration::from_secs(60));
            d = d.min(left.max(self.cfg.step_timeout));
        }
        d
    }

    fn native_read(&self, spec: &str) -> Observation {
        let start = Instant::now();
        let mut out = String::new();
        for raw in spec.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let (path, range) = parse_read_target(raw);
            let resolved = match self.resolve_action_path(path, false) {
                Ok(p) => p,
                Err(e) => return action_error(e, start, 126),
            };
            if !resolved.path.exists() {
                return action_error(format!("read: {} does not exist", path), start, 1);
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
            for (idx, line) in text.lines().enumerate() {
                let n = idx + 1;
                if let Some((start_line, end_line)) = range
                    && (n < start_line || n > end_line)
                {
                    continue;
                }
                out.push_str(&format!("{n:>6}\t{line}\n"));
            }
        }
        Observation {
            output: out,
            duration: start.elapsed(),
            ..Observation::default()
        }
    }

    fn native_grep(&self, spec: &str) -> Observation {
        let start = Instant::now();
        let mut lines = spec.lines();
        let pattern = lines.next().unwrap_or("").trim();
        if pattern.is_empty() {
            return action_error("grep: missing regex pattern".into(), start, 1);
        }
        let re = match Regex::new(pattern) {
            Ok(r) => r,
            Err(e) => return action_error(format!("grep: invalid regex: {e}"), start, 1),
        };
        let scope = lines
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(".");
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

    fn native_edit(&self, spec: &str) -> Observation {
        let start = Instant::now();
        let Some((path, rest)) = spec.split_once('\n') else {
            return action_error("edit: missing path and replacement block".into(), start, 1);
        };
        let path = path.trim();
        let blocks = match parse_edit_blocks(rest) {
            Ok(b) => b,
            Err(e) => return action_error(e, start, 1),
        };
        let create = !Path::new(&self.cfg.cwd).join(path).exists()
            && blocks.len() == 1
            && blocks[0].search.is_empty();
        let resolved = match self.resolve_action_path(path, create) {
            Ok(p) => p,
            Err(e) => return action_error(e, start, 126),
        };
        if create {
            if let Err(e) = fs::write(&resolved.path, &blocks[0].replace) {
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
        let root = fs::canonicalize(if self.cfg.cwd.is_empty() {
            "."
        } else {
            &self.cfg.cwd
        })
        .map_err(|e| format!("canonicalize clone root: {e}"))?;
        let path = root.join(rel_path);
        let check = if allow_create {
            path.parent().unwrap_or(&root).to_path_buf()
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
        self.result.exit_status = status.to_string();
        self.result.submission = submission.to_string();
        self.result.error = err_text.to_string();
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
        self.steps.push(s.clone());
        for sink in &mut self.sinks {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                sink.step(s.clone());
            }));
        }
    }
}

fn tool_message(call: &ToolCall, content: &str) -> Message {
    Message {
        role: Role::Tool,
        content: content.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: call.id.clone(),
        name: call.name.clone(),
    }
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
    "You have exactly one tool: bash. Call it with a JSON object containing a command string.\n",
    "If you cannot call tools, write exactly one fenced block per message; exactly one fenced block is executed.\n",
    "The working directory is already the clone root ({{.Info.cwd}}), so `cd <root> &&` is unnecessary.\n",
    "Example command reply: ```bash\ncargo test -p crate_name\n```.\n",
    "Example report reply: SUMMARY: done\nFINDING: tests pass.\n",
    "Prose-only replies are not executed.\n",
    "Shell command fences may use ```bash, ```sh, ```shell, ```zsh, ```console, or ```terminal.\n",
    "Native action fences are cheaper than shell for the same job:\n",
    "```read\nsrc/lib.rs:1-80\n```\n",
    "```grep\nTODO\nsrc\n```\n",
    "```edit\nsrc/lib.rs\n<<<<<<< SEARCH\nold\n=======\nnew\n>>>>>>> REPLACE\n```\n",
    "Each shell command runs in a fresh non-interactive {{.Info.shell}} in the working directory.\n",
    "No state survives between commands: cd, exports, aliases, and shell variables are lost.\n",
    "Background processes a command starts (cmd &, nohup, servers) are killed when the command ends; to smoke-test a server, start it, probe it, and stop it within one command.\n",
    "Chain setup and use with &&, or use absolute paths.\n",
    "Commands time out after {{.StepTimeout}}.\n",
    "Keep outputs small with head, tail, grep, and focused test selectors.\n",
    "Output over {{.OutputLimit}} characters is cut in the middle.\n",
    "Older command outputs are shortened to their first and last lines as the conversation grows; rerun a command if you need its full output again.\n",
    "Never run interactive programs: editors, pagers, watch modes, or foreground servers.\n",
    "To edit files, prefer ```edit blocks; shell heredocs, sed -i, or python3 -c are still available when needed.\n",
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
    "The task follows in the next message and its rules apply in full."
);

pub use DEFAULT_PROMPT_TEMPLATE as DefaultPromptTemplate;

#[derive(Clone, Debug, Default)]
pub struct PromptData {
    pub cwd: String,
    pub model: String,
    pub provider: String,
    pub step_timeout: String,
    pub output_limit: usize,
    pub max_steps: i32,
    pub info: BTreeMap<String, String>,
}

pub fn render_prompt(tmpl: &str, data: PromptData) -> anyhow::Result<String> {
    let data = normalize_prompt_data(data);
    let mut out = tmpl.to_string();
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
}

pub use render_prompt as RenderPrompt;

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

#[derive(Clone, Debug)]
struct ResolvedPath {
    rel: String,
    path: PathBuf,
}

#[derive(Clone, Debug)]
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
    }

    fn msg(content: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: String::new(),
            name: String::new(),
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
            h.resolve_action_path(".pulse/private/token", false)
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
        let p = Arc::new(FakeProvider::new(vec![resp(msg("SUMMARY: done"))]));
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
        let p = Arc::new(FakeProvider::new(vec![resp(msg("SUMMARY: done"))]));
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
            },
            Message {
                role: Role::User,
                content: "task".into(),
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
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
            Message { role: Role::User, content: "task".into(), tool_calls: Vec::new(), tool_call_id: String::new(), name: String::new() },
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
            },
            Message {
                role: Role::User,
                content: "<returncode>0</returncode>\n<output>\ntask\n</output>".into(),
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
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
}
