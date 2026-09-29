use anyhow::anyhow;
use pulse_provider::{CancelToken, Message, Provider, Request, Role, Tool, ToolCall, Usage};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

pub const DEFAULT_MAX_STEPS: i32 = 60;
pub const DEFAULT_STEP_TIMEOUT: Duration = Duration::from_secs(5 * 60);
pub const DEFAULT_OUTPUT_LIMIT: usize = 20000;
pub const DEFAULT_KEEP_OBSERVATIONS: usize = 5;
pub const DEFAULT_MAX_FORMAT_ERRORS: i32 = 3;

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
    r#"(^|[\s"'`])(~/.ssh/|\.netrc\b|/private/|~/.pulse/private\b|~/.copilot\b|~/.config/gh\b)"#,
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
    pub wall_time: Duration,
    pub max_cost: f64,
    pub output_limit: usize,
    pub keep_observations: usize,
    pub context_window: i32,
    pub max_format_errors: i32,
    pub deny: Vec<String>,
    pub provider: String,
    pub cancel_file: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepKind {
    Assistant,
    Command,
    Observation,
    Note,
    Error,
    Exit,
}

pub const STEP_ASSISTANT: StepKind = StepKind::Assistant;
pub const STEP_COMMAND: StepKind = StepKind::Command;
pub const STEP_OBSERVATION: StepKind = StepKind::Observation;
pub const STEP_NOTE: StepKind = StepKind::Note;
pub const STEP_ERROR: StepKind = StepKind::Error;
pub const STEP_EXIT: StepKind = StepKind::Exit;

pub use STEP_ASSISTANT as StepAssistant;
pub use STEP_COMMAND as StepCommand;
pub use STEP_ERROR as StepError;
pub use STEP_EXIT as StepExit;
pub use STEP_NOTE as StepNote;
pub use STEP_OBSERVATION as StepObservation;

impl std::fmt::Display for StepKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Assistant => "assistant",
            Self::Command => "command",
            Self::Observation => "observation",
            Self::Note => "note",
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

pub use EXIT_CANCELLED as ExitCancelled;
pub use EXIT_COST_LIMIT as ExitCostLimit;
pub use EXIT_FORMAT_ERROR as ExitFormatError;
pub use EXIT_NOT_LOGGED_IN as ExitNotLoggedIn;
pub use EXIT_PROVIDER_ERROR as ExitProviderError;
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
}

pub const ERR_OWNER_CANCELLED: &str = "cancelled by the owner";
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
        let mut format_errors = 0;
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
                        self.cancel_reason = "interrupted: context canceled".to_string();
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
            let (clean, submitted, submission) = self.handle_assistant(cancel, msg);
            if submitted {
                return Ok(self.finish(EXIT_SUBMITTED, &submission, ""));
            }
            if clean {
                format_errors = 0;
            } else {
                format_errors += 1;
                if format_errors >= self.cfg.max_format_errors {
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
            self.cancel_reason = "interrupted: context canceled".to_string();
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

    fn handle_assistant(&mut self, cancel: &CancelToken, msg: Message) -> (bool, bool, String) {
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
                    return (true, true, sub);
                }
            }
            return (all_clean, false, String::new());
        }
        let blocks = bash_blocks(&msg.content);
        if blocks.len() == 1 {
            let (obs_text, _, submit, sub) = self.run_command(cancel, &blocks[0]);
            self.msgs.push(Message {
                role: Role::User,
                content: obs_text,
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
            });
            if submit {
                return (true, true, sub);
            }
            return (true, false, String::new());
        }
        if blocks.len() > 1 {
            let text =
                "write exactly one ```bash block per message, or your final report with no command"
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
            return (false, false, String::new());
        }
        if is_report(&msg.content) {
            return (true, true, msg.content);
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
            kind: StepKind::Error,
            content: text,
            ..Step::default()
        });
        (false, false, String::new())
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
        let obs = self.env.execute(cancel, command, self.cfg.step_timeout);
        self.emit_observation(command, &obs);
        let formatted = format_observation(&obs, self.cfg.output_limit);
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

    fn emit_observation(&mut self, command: &str, obs: &Observation) {
        self.emit(Step {
            kind: StepKind::Observation,
            content: obs.output.clone(),
            command: command.to_string(),
            exit_code: Some(obs.exit_code),
            duration_ms: obs.duration.as_millis() as i64,
            ..Step::default()
        });
    }

    fn finish(&mut self, status: &str, submission: &str, err_text: &str) -> ResultSummary {
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
    "If you cannot call tools, write exactly one fenced ```bash or ```sh block per message.\n",
    "Each command runs in a fresh non-interactive {{.Info.shell}} in the working directory.\n",
    "No state survives between commands: cd, exports, aliases, and shell variables are lost.\n",
    "Chain setup and use with &&, or use absolute paths.\n",
    "Commands time out after {{.StepTimeout}}.\n",
    "Keep outputs small with head, tail, grep, and focused test selectors.\n",
    "Output over {{.OutputLimit}} characters is cut in the middle.\n",
    "Older command outputs are shortened to their first and last lines as the conversation grows; rerun a command if you need its full output again.\n",
    "Never run interactive programs: editors, pagers, watch modes, or foreground servers.\n",
    "To edit files, use heredocs, sed -i, or a small python3 -c script.\n",
    "Verify your work by running the project's own checks.\n",
    "On a command failure, inspect the cause, make a small safe correction and rerun the targeted check.\n",
    "If it still fails, try a different evidence-based approach or use the task's peer-help channel; do not repeat an unchanged failure or poll while waiting for help.\n",
    "Never bypass a policy refusal or approval boundary, weaken a check, or report unfinished work as completed.\n",
    "A budget notice arrives when few model calls remain; after it, stop exploring and finish with the targeted check, commit, and final SUMMARY report.\n",
    "An unfinished task is reported as SUMMARY: blocked: <cause and attempted fixes> or SUMMARY: failed: <cause and attempted fixes>.\n",
    "When done, or when you cannot continue, reply with no command and a plain-text final report.\n",
    "The report must include a SUMMARY: line and any FINDING:, SUGGESTION:, QUESTION:, or VOTE: lines the task asked for.\n",
    "Those key words must start their lines because a program reads the report.\n",
    "A message with neither a command nor a report wastes a step.\n",
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
    if cfg.output_limit == 0 {
        cfg.output_limit = DEFAULT_OUTPUT_LIMIT;
    }
    if cfg.keep_observations == 0 {
        cfg.keep_observations = DEFAULT_KEEP_OBSERVATIONS;
    }
    if cfg.max_format_errors == 0 {
        cfg.max_format_errors = DEFAULT_MAX_FORMAT_ERRORS;
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
    static RE: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"(?s)```(?:bash|sh)\s*\n(.*?)\n```").unwrap());
    RE.captures_iter(text)
        .map(|c| c[1].trim().to_string())
        .collect()
}

pub fn is_report(text: &str) -> bool {
    static RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?m)^\s*-?\s*`?(SUMMARY|QUESTION|FINDING|SUGGESTION|VOTE)`?:").unwrap()
    });
    text.contains("COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT") || RE.is_match(text)
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

pub fn truncate(s: &str, limit: usize) -> String {
    if limit == 0 || s.len() <= limit {
        return s.to_string();
    }
    let half = limit / 2;
    let omitted = s.len() - half - half;
    format!(
        "{}\n[... {omitted} characters omitted ...]\n{}",
        &s[..half],
        &s[s.len() - half..]
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
        s[..limit].to_string()
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
    use std::sync::{Arc, Mutex};

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

    #[derive(Default)]
    struct FakeEnv {
        observations: Vec<Observation>,
        commands: Vec<String>,
        default_output: String,
    }

    impl Environment for Arc<Mutex<FakeEnv>> {
        fn execute(
            &mut self,
            _cancel: &CancelToken,
            command: &str,
            _timeout: Duration,
        ) -> Observation {
            let mut e = self.lock().unwrap();
            e.commands.push(command.to_string());
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

    fn base_config() -> Config {
        Config {
            model: "m".into(),
            task: "do it".into(),
            system_prompt: "system".into(),
            max_steps: 10,
            ..Config::default()
        }
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
        let custom = render_prompt(
            "{{.Cwd}} {{.Provider}} {{.Model}} {{.Info.shell}}",
            prompt_data(&info, &cfg),
        )
        .unwrap();
        assert_eq!(custom, "/work fake m /bin/zsh");
    }

    #[test]
    fn text_mode_path_appends_user_observation() {
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
    }

    #[test]
    fn two_block_format_error_stops_when_limit_reached() {
        let mut cfg = base_config();
        cfg.max_format_errors = 1;
        let p = Arc::new(FakeProvider::new(vec![resp(msg(
            "```bash\na\n```\n```sh\nb\n```",
        ))]));
        let env = Arc::new(Mutex::new(FakeEnv::default()));
        let (h, got) = run_agent(p, env, cfg).unwrap();
        assert_eq!(got.exit_status, EXIT_FORMAT_ERROR);
        assert!(h.messages()[3].content.contains("write exactly one"));
    }

    #[test]
    fn format_errors_stop_after_three_plan_only_messages() {
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
    fn step_limit() {
        let mut cfg = base_config();
        cfg.max_steps = 1;
        let p = Arc::new(FakeProvider::new(vec![resp(tool("echo hi"))]));
        let (_, got) = run_agent(p, Arc::new(Mutex::new(FakeEnv::default())), cfg).unwrap();
        assert_eq!(got.exit_status, EXIT_STEP_LIMIT);
        assert_eq!(got.calls, 1);
    }

    #[test]
    fn cost_limit_stops_run() {
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
    fn prune_note_emitted_once_per_level() {
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
    fn deny_refuses_without_executing() {
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
    fn truncation_keeps_head_tail() {
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
    fn cancellation_reports_owner_reason_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let cancel_file = dir.path().join("cancel");
        fs::write(&cancel_file, "stop").unwrap();
        let mut cfg = base_config();
        cfg.cancel_file = cancel_file.display().to_string();
        let p = Arc::new(FakeProvider::new(Vec::new()));
        let (_, got) = run_agent(p, Arc::new(Mutex::new(FakeEnv::default())), cfg).unwrap();
        assert_eq!(got.exit_status, EXIT_CANCELLED);
        assert_eq!(got.error, ERR_OWNER_CANCELLED);
    }

    #[test]
    fn write_trajectory() {
        let p = Arc::new(FakeProvider::new(vec![resp(msg("SUMMARY: done"))]));
        let (h, got) =
            run_agent(p, Arc::new(Mutex::new(FakeEnv::default())), base_config()).unwrap();
        assert_eq!(got.exit_status, EXIT_SUBMITTED);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/trajectory.json");
        super::write_trajectory(path.to_str().unwrap(), &h.trajectory()).unwrap();
        let data = fs::read_to_string(path).unwrap();
        assert!(data.contains("\"trajectory_format\": \"pulse-harness-1\""));
        assert!(data.contains("\"exit_status\": \"submitted\""));
    }

    #[test]
    fn sink_panic_is_swallowed() {
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
