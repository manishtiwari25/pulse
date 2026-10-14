use crate::compact::DEFAULT_COMPACT_AT;
use crate::fetch::FetchPolicy;
use crate::harness::*;
use crate::hooks::load_hooks;
use crate::localenv::LocalEnv;
use crate::memory::{self, Memory};
use crate::permissions::{Rule, Rules};
use crate::search::WebSearch;
use crate::skills::{Skill, load_skills};
use anyhow::Result;
use pulse_cli::{Command, Context, FlagSet, errorf, table, write_json};
use pulse_provider::{CancelToken, Login, Options as ProviderOptions, Provider};
use pulse_store::{HarnessSession, HarnessSessionFilter, HarnessStep, STORE_NAME, Store};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once};
use std::thread;
use std::time::{Duration, Instant};

const HARNESS_USAGE: &str = "pulse harness run -p TEXT [-C DIR] [--model M] [--steps-file PATH] ... | chat [-C DIR] [--model M] [--yes] [--resume ID | --continue] | login [--with-token] | status | models | logout | sessions | show ID | cancel ID | prompt";
const LOGIN_FIX: &str =
    "sign in with GitHub in the console (Settings → Harness) or run pulse harness login";

pub fn not_logged_in_line(err: &anyhow::Error) -> String {
    let mut reason = err
        .to_string()
        .trim()
        .trim_start_matches("not logged in")
        .trim()
        .trim_start_matches(':')
        .trim()
        .to_string();
    if reason.is_empty() {
        reason = LOGIN_FIX.to_string();
        format!("not logged in: {reason}")
    } else {
        format!("not logged in: {reason} - {LOGIN_FIX}")
    }
}

static REGISTER: Once = Once::new();

pub fn register() {
    REGISTER.call_once(|| {
        pulse_cli::register(Command {
            name: "harness".into(),
            group: "runner".into(),
            summary: "the built-in agent harness: run a task or chat through the model provider, sign in, list models and sessions".into(),
            usage: HARNESS_USAGE.into(),
            aliases: vec![],
            no_store: true,
            read_only: false,
            raw: true,
            run: run_harness_command,
            flags: None,
        });
    });
}

pub fn run_harness_command(c: &mut Context, _fs: &FlagSet, args: &[String]) -> Result<()> {
    if std::env::var("PULSE_HARNESS_FAKE").as_deref() == Ok("1") {
        pulse_provider_fake::register();
    }
    let Some((verb, rest)) = args.split_first() else {
        return Err(unknown_harness_verb("").into());
    };
    match verb.as_str() {
        "run" => harness_run(c, rest),
        "chat" => crate::chat::harness_chat(c, rest),
        "login" => harness_login(c, rest),
        "status" => harness_status(c, rest),
        "models" => harness_models(c, rest),
        "logout" => harness_logout(c, rest),
        "sessions" => harness_sessions(c, rest),
        "show" => harness_show(c, rest),
        "cancel" => harness_cancel(c, rest),
        "prompt" => harness_prompt(c, rest),
        _ => Err(unknown_harness_verb(verb).into()),
    }
}

pub fn unknown_harness_verb(verb: &str) -> pulse_cli::Error {
    let what = if verb.is_empty() {
        "pulse harness: missing verb".to_string()
    } else {
        format!("pulse harness: unknown verb {verb:?}")
    };
    errorf(
        2,
        &what,
        "",
        "pulse harness run|chat|login|status|models|logout|sessions|show|cancel|prompt",
    )
}

/// The flags `harness run` and `harness chat` share: everything that
/// shapes the agent but the task.
pub(crate) fn add_agent_flags(fs: &mut FlagSet) {
    fs.string_var("C", ".", "working directory");
    fs.string_var("model", "", "model id");
    fs.string_var("provider", "", "provider name");
    fs.int_var("max-steps", 0, "maximum model calls");
    fs.float64_var("max-cost", 0.0, "maximum provider-reported cost");
    fs.int_var("step-timeout", 0, "per-command timeout in seconds");
    fs.int_var(
        "commit-timeout",
        0,
        "timeout in seconds for a git commit that runs the pre-commit verify gate",
    );
    fs.int_var("output-limit", 0, "characters of command output kept");
    fs.int_var(
        "keep-observations",
        0,
        "observations kept verbatim in context",
    );
    fs.int_var(
        "context-window",
        0,
        "tokens of context the model takes; 0 reads the model list",
    );
    fs.int_var("minutes", 0, "wall-clock cap in minutes");
    fs.string_var("session", "", "harness session id");
    fs.string_var("trajectory", "", "trajectory path, or none");
    fs.string_var(
        "steps-file",
        "",
        "append newline-delimited harness events to PATH",
    );
    fs.string_var("cancel-file", "", "stop when PATH exists");
    fs.string_var(
        "prompt-file",
        "",
        "read the system prompt template from PATH",
    );
    fs.bool_var("no-store", false, "do not open the PULSE store");
    fs.bool_var(
        "no-instructions",
        false,
        "do not read AGENTS.md and the other repository instruction files into the prompt",
    );
    fs.bool_var(
        "no-mcp",
        false,
        "do not offer the project's enabled MCP servers as tools",
    );
    fs.bool_var(
        "no-skills",
        false,
        "do not offer the clone's, the owner's and the registry's skills through the skill tool",
    );
    fs.bool_var(
        "no-hooks",
        false,
        "do not run the owner's hooks (.pulse/hooks.json) around the run and its tool calls",
    );
    fs.bool_var(
        "no-subagents",
        false,
        "do not offer the subagent tool (a bounded helper run on a sub-task)",
    );
    fs.bool_var(
        "no-memory",
        false,
        "do not read ~/.pulse/memory into the prompt or offer the memory tool",
    );
    fs.bool_var(
        "web-search",
        false,
        "offer the web_search tool (DuckDuckGo, or Brave with BRAVE_SEARCH_API_KEY) whatever harness.web_search says",
    );
    fs.bool_var("no-web-search", false, "do not offer the web_search tool");
    fs.string_var(
        "fetch-allow",
        "",
        "hosts the fetch tool may read web pages from (comma-separated, *.example.com, or *); overrides harness.fetch_allow",
    );
    fs.strings_var(
        "allow-tool",
        "calls that run without approval: bash(git *), edit(src/*), write, fetch(docs.rs), mcp__github__* (repeatable, or comma-separated; adds to harness.allow_tools)",
    );
    fs.strings_var(
        "deny-tool",
        "calls that are refused outright: bash(git push *), read(.env*), write(*.lock), subagent (repeatable, or comma-separated; adds to harness.deny_tools; a tool denied whole is not offered)",
    );
    fs.bool_var(
        "bare",
        false,
        "a plain agent: no instruction files, memory, skills, hooks or MCP servers (the same as --no-instructions --no-memory --no-skills --no-hooks --no-mcp)",
    );
    fs.int_var(
        "compact-at",
        -1,
        "percent of the context window at which the conversation is summarised and compacted; 0 turns it off; overrides harness.compact_at",
    );
    fs.bool_var(
        "read-only",
        false,
        "a run that looks but does not touch: edit and write are not offered, commands that would change the clone are refused, and a change that slips through is put back (the runner sets this for answer and ballot jobs)",
    );
    fs.string_var(
        "reasoning",
        "",
        "reasoning effort asked of the model (minimal, low, medium, high, xhigh) where the provider takes one; empty leaves the model's default; overrides harness.reasoning",
    );
}

/// An agent built from the flags, with everything around it that the
/// command finishes with once the agent is done.
pub(crate) struct Prepared {
    pub(crate) shared: Arc<Mutex<Agent>>,
    pub(crate) provider: Arc<dyn Provider>,
    pub(crate) provider_name: String,
    pub(crate) model_name: String,
    pub(crate) session_id: String,
    pub(crate) cwd: String,
    pub(crate) trajectory_path: String,
    pub(crate) st: Option<Store>,
    pub(crate) step_log: Option<Arc<Mutex<StepsFileSink>>>,
    pub(crate) rec_for_finish: Option<StoreSink>,
    _close_store: StoreClose,
}

impl Prepared {
    /// Writes the trajectory, if one is kept; a failure is a warning.
    pub(crate) fn write_trajectory(&self, c: &mut Context) -> Result<()> {
        if self.trajectory_path.is_empty() {
            return Ok(());
        }
        let traj = self.shared.lock().unwrap().trajectory();
        if let Err(e) = write_trajectory(&self.trajectory_path, &traj) {
            writeln!(c.err, "pulse harness: trajectory warning: {e}")?;
        }
        Ok(())
    }

    /// Closes the store session and the steps file with the result.
    pub(crate) fn finish(&mut self, c: &mut Context, result: &ResultSummary) -> Result<()> {
        if let Some(mut rec) = self.rec_for_finish.take()
            && let Err(e) = rec.finish(result, &self.trajectory_path)
        {
            writeln!(c.err, "pulse harness: store warning: {e}")?;
        }
        if let Some(log) = &self.step_log {
            log.lock().expect("steps-file sink poisoned").finish(result);
            if let Some(warning) = log
                .lock()
                .expect("steps-file sink poisoned")
                .warning
                .clone()
            {
                writeln!(c.err, "pulse harness: steps-file warning: {warning}")?;
            }
        }
        Ok(())
    }

    /// Cancels `cancel` when the owner cancels the session in the store
    /// (`pulse harness cancel ID`); the closure stops the watch.
    pub(crate) fn watch_cancel(&self, cancel: &CancelToken) -> Option<impl FnOnce() + use<>> {
        self.st
            .as_ref()
            .map(|s| watch_harness_cancel(s.clone(), self.session_id.clone(), cancel.clone()))
    }
}

/// Builds the agent `harness run` and `harness chat` drive from the parsed
/// flags: store, provider, model, budgets, prompt, instructions, tools and
/// sinks. `extra_sinks` go first; the progress lines on stderr are added
/// when `progress` is set and the command is not quiet.
pub(crate) fn prepare_agent(
    c: &mut Context,
    fs: &FlagSet,
    verb: &str,
    task: String,
    interactive: bool,
    extra_sinks: Vec<Box<dyn Sink>>,
    progress: bool,
) -> Result<Prepared> {
    let abs_cwd = existing_dir(&fs.get_string("C"))?;
    let mut session_id = fs.get_string("session");
    if session_id.is_empty() {
        session_id = std::env::var("PULSE_SESSION").unwrap_or_default();
    }
    if session_id.is_empty() {
        session_id = new_harness_session_id();
    }
    let mut steps_file = fs.get_string("steps-file");
    if steps_file.is_empty() {
        steps_file = std::env::var("PULSE_HARNESS_STEPS").unwrap_or_default();
    }
    let mut cancel_file = fs.get_string("cancel-file");
    if cancel_file.is_empty() {
        cancel_file = std::env::var("PULSE_HARNESS_CANCEL").unwrap_or_default();
    }
    let mut prompt_file = fs.get_string("prompt-file");
    if prompt_file.is_empty() {
        prompt_file = std::env::var("PULSE_HARNESS_PROMPT_FILE").unwrap_or_default();
    }
    let step_log = (!steps_file.is_empty()).then(|| {
        std::sync::Arc::new(std::sync::Mutex::new(StepsFileSink::new(
            c,
            &steps_file,
            &session_id,
        )))
    });
    let no_store = fs.get_bool("no-store");
    let (st, close_store) = open_harness_store(c, no_store)?;
    if st.is_none() && !no_store && !c.quiet {
        writeln!(
            c.err,
            "pulse harness: no store at {}; running without store defaults or session history",
            Path::new(&c.home).join(STORE_NAME).display()
        )?;
    }
    let mut provider_name = fs.get_string("provider");
    if provider_name.is_empty() {
        provider_name = st.as_ref().map_or_else(
            || pulse_provider::DEFAULT.to_string(),
            |s| s.setting("provider", pulse_provider::DEFAULT),
        );
    }
    let p = pulse_provider::open(&provider_name, harness_provider_options(c, st.as_ref()))
        .map_err(|e| {
            errorf(
                2,
                &format!("pulse harness {verb}: {e}"),
                "",
                "pass --provider P",
            )
        })?;
    let p: Arc<dyn Provider> = Arc::from(p);
    let mut model_name = fs.get_string("model");
    if model_name.is_empty() {
        match default_run_model(&CancelToken::new(), st.as_ref(), p.as_ref()) {
            Ok(m) => model_name = m,
            Err(e) if pulse_provider::is_not_logged_in(&e) => {
                let result = ResultSummary {
                    exit_status: EXIT_NOT_LOGGED_IN.into(),
                    error: e.to_string(),
                    ..ResultSummary::default()
                };
                writeln!(c.err, "{}", not_logged_in_line(&e))?;
                if let Some(log) = &step_log {
                    let mut log = log.lock().expect("steps-file sink poisoned");
                    log.begin(&provider_name, "", &abs_cwd);
                    log.finish(&result);
                }
                if let Some(s) = &st {
                    let mut rec = StoreSink::new(c, s.clone(), &session_id);
                    if rec.begin(&provider_name, "", &abs_cwd, "").is_ok() {
                        let _ = rec.finish(&result, "");
                    }
                }
                if !interactive {
                    if c.json {
                        write_json(&mut c.out, &run_json(&session_id, &result, ""))?;
                    } else {
                        write_run_text(&mut c.out, &[], &result)?;
                    }
                }
                return Err(errorf(4, "", "", "").into());
            }
            Err(e) => return Err(e),
        }
    }
    let max_steps = setting_int_or(
        fs,
        "max-steps",
        st.as_ref(),
        "harness.max_steps",
        DEFAULT_MAX_STEPS,
    );
    let max_cost = if fs.get_f64("max-cost") != 0.0 {
        fs.get_f64("max-cost")
    } else {
        st.as_ref()
            .map_or(0.0, |s| s.setting_float("harness.max_cost"))
    };
    let step_timeout = setting_int_or(
        fs,
        "step-timeout",
        st.as_ref(),
        "harness.step_timeout",
        DEFAULT_STEP_TIMEOUT.as_secs() as i32,
    );
    let commit_timeout = setting_int_or(
        fs,
        "commit-timeout",
        st.as_ref(),
        "harness.commit_timeout",
        DEFAULT_COMMIT_TIMEOUT.as_secs() as i32,
    );
    let output_limit = setting_int_or(
        fs,
        "output-limit",
        st.as_ref(),
        "harness.output_limit",
        DEFAULT_OUTPUT_LIMIT as i32,
    );
    let keep_observations = setting_int_or(
        fs,
        "keep-observations",
        st.as_ref(),
        "harness.keep_observations",
        DEFAULT_KEEP_OBSERVATIONS as i32,
    );
    let format_retries = st.as_ref().map_or(DEFAULT_FORMAT_RETRIES, |s| {
        s.setting_int("harness.format_retries") as i32
    });
    // A session speaks to a person: it keeps the chat prompt unless a
    // prompt file says otherwise; a job takes the owner's template.
    let mut system_prompt_template = if interactive {
        String::new()
    } else {
        st.as_ref()
            .map_or_else(String::new, |s| s.setting("harness.system_prompt", ""))
    };
    if !prompt_file.is_empty() {
        system_prompt_template = fs::read_to_string(&prompt_file).map_err(|e| {
            errorf(
                2,
                &format!("pulse harness {verb}: read prompt file: {e}"),
                "",
                "pass --prompt-file PATH",
            )
        })?;
    }
    // The flag, then the setting, override the window the model list
    // reports; both are off at zero.
    let context_window = match setting_int_or(
        fs,
        "context-window",
        st.as_ref(),
        "harness.context_window",
        0,
    ) {
        n if n > 0 => n,
        _ => model_context_window(&CancelToken::new(), p.as_ref(), &model_name),
    };
    let mut trajectory = fs.get_string("trajectory");
    let trajectory_path = if trajectory == "none" {
        String::new()
    } else {
        if trajectory.is_empty() {
            trajectory = Path::new(&c.home)
                .join("harness")
                .join(format!("{session_id}.json"))
                .display()
                .to_string();
        }
        if let Some(parent) = Path::new(&trajectory).parent() {
            fs::create_dir_all(parent)?;
        }
        trajectory
    };
    let progress_start = Instant::now();
    let shared_err = SharedWriter::new(std::mem::replace(&mut c.err, Box::new(io::sink())));
    c.err = Box::new(shared_err.clone());
    let mut sinks: Vec<Box<dyn Sink>> = extra_sinks;
    if progress && !c.quiet {
        sinks.push(Box::new(ProgressSink::new(shared_err, progress_start)));
    }
    if let Some(log) = &step_log {
        log.lock()
            .expect("steps-file sink poisoned")
            .begin(&provider_name, &model_name, &abs_cwd);
    }
    if let Some(log) = &step_log {
        sinks.push(Box::new(SharedStepsFileSink(log.clone())));
    }
    let mut rec_for_finish = None;
    if let Some(s) = &st {
        let mut rec = StoreSink::new(c, s.clone(), &session_id);
        if let Err(e) = rec.begin(&provider_name, &model_name, &abs_cwd, &trajectory_path) {
            writeln!(c.err, "pulse harness: store warning: {e}")?;
        }
        rec_for_finish = Some(rec.clone_for_finish());
        sinks.push(Box::new(rec));
    }
    let env: Vec<String> = std::env::vars()
        .map(|(k, v)| format!("{k}={v}"))
        .chain([
            "PULSE_HARNESS=1".to_string(),
            format!("PULSE_HARNESS_SESSION={session_id}"),
            "GIT_TERMINAL_PROMPT=0".to_string(),
        ])
        .collect();
    let bare = fs.get_bool("bare");
    let instructions_on = !bare
        && !fs.get_bool("no-instructions")
        && st
            .as_ref()
            .is_none_or(|s| s.setting("harness.instructions", "on") != "off");
    let (instructions, instruction_files) = if instructions_on {
        load_instructions(&abs_cwd)
    } else {
        (String::new(), Vec::new())
    };
    let project = std::env::var("PULSE_PROJECT").unwrap_or_default();
    let memory_on = !bare
        && !fs.get_bool("no-memory")
        && st
            .as_ref()
            .is_none_or(|s| s.setting("harness.memory", "on") != "off");
    let memory = memory_on.then(|| {
        // A chat in a registered project's checkout shares the file with
        // the fleet's jobs on that project.
        let key = if project.is_empty() {
            st.as_ref()
                .and_then(|s| s.project_for_dir(&abs_cwd).ok().flatten())
                .map(|p| p.slug)
                .unwrap_or_default()
        } else {
            project.clone()
        };
        Memory::new(&c.home, &memory::project_key(&abs_cwd, &key))
    });
    let (memory_text, memory_files) = memory
        .as_ref()
        .map(Memory::prompt_section)
        .unwrap_or_default();
    let subagents = !fs.get_bool("no-subagents")
        && st
            .as_ref()
            .is_none_or(|s| s.setting("harness.subagents", "on") != "off");
    let compact_at = match fs.get_int("compact-at") {
        n if n >= 0 => n.min(95),
        _ => st.as_ref().map_or(DEFAULT_COMPACT_AT, |s| {
            s.setting_int("harness.compact_at") as i32
        }),
    };
    let read_only = fs.get_bool("read-only");
    let reasoning = {
        let flag = fs.get_string("reasoning").trim().to_lowercase();
        let v = if flag.is_empty() {
            st.as_ref()
                .map(|s| s.setting("harness.reasoning", "default"))
                .unwrap_or_default()
        } else {
            flag
        };
        if v.is_empty() || v == "default" {
            String::new()
        } else if pulse_provider::REASONING_EFFORTS.contains(&v.as_str()) {
            v
        } else {
            return Err(errorf(
                2,
                &format!("pulse harness {verb}: --reasoning {v:?} is not a reasoning effort"),
                "",
                &format!(
                    "use one of default, {}",
                    pulse_provider::REASONING_EFFORTS.join(", ")
                ),
            )
            .into());
        }
    };
    let mut cfg = Config {
        model: model_name.clone(),
        task,
        cwd: abs_cwd.clone(),
        env: env.clone(),
        provider: provider_name.clone(),
        cancel_reason: if interactive {
            "interrupted by the user".into()
        } else {
            ERR_OWNER_CANCELLED.into()
        },
        max_steps,
        step_timeout: Duration::from_secs(step_timeout as u64),
        commit_timeout: Duration::from_secs(commit_timeout as u64),
        max_cost,
        output_limit: output_limit as usize,
        keep_observations: keep_observations as usize,
        format_retries,
        context_window,
        system_prompt_template,
        cancel_file,
        instructions,
        memory: memory_text,
        subagents,
        compact_at,
        read_only,
        reasoning,
        interactive,
        ..Config::default()
    };
    if fs.get_int("minutes") > 0 {
        cfg.wall_time = Duration::from_secs(fs.get_int("minutes") as u64 * 60);
    }
    if !cfg.system_prompt_template.is_empty() {
        cfg.system_prompt = render_prompt(
            &cfg.system_prompt_template,
            prompt_data(
                &LocalEnv::new(abs_cwd.clone(), Some(env.clone())).info(),
                &cfg,
            ),
        )
        .map_err(|e| {
            errorf(
                2,
                &format!("pulse harness {verb}: harness.system_prompt render failed: {e}"),
                "",
                "fix harness.system_prompt or pass --prompt-file",
            )
        })?;
    }
    let trajectory_sink =
        (!trajectory_path.is_empty()).then(|| TrajectorySink::new(c, &trajectory_path));
    let mut agent = new_shared(
        p.clone(),
        Box::new(LocalEnv::new(abs_cwd.clone(), Some(env))),
        cfg,
        sinks,
    );
    if !instruction_files.is_empty() {
        agent.note_at_start(format!(
            "read repository instructions from {} ({} bytes) into the system prompt",
            instruction_files.join(", "),
            agent.cfg.instructions.len()
        ));
    }
    if let Some(m) = memory {
        if !memory_files.is_empty() {
            agent.note_at_start(format!(
                "read memory from {} ({} bytes) into the system prompt",
                memory_files.join(", "),
                agent.cfg.memory.len()
            ));
        }
        agent.set_memory(m);
    }
    let mcp_on = !bare
        && !fs.get_bool("no-mcp")
        && st
            .as_ref()
            .is_some_and(|s| s.setting("harness.mcp_tools", "on") != "off");
    if mcp_on && let Some(s) = &st {
        for note in attach_mcp_servers(&mut agent, s, &project, &c.home) {
            agent.note_at_start(note);
        }
    }
    let skills_on = !bare
        && !fs.get_bool("no-skills")
        && st
            .as_ref()
            .is_none_or(|s| s.setting("harness.skills", "on") != "off");
    if skills_on && let Some(note) = attach_skills(&mut agent, st.as_ref(), &project, &c.home) {
        agent.note_at_start(note);
    }
    let hooks_on = !bare
        && !fs.get_bool("no-hooks")
        && st
            .as_ref()
            .is_none_or(|s| s.setting("harness.hooks", "on") != "off");
    if hooks_on {
        for note in attach_hooks(&mut agent, &c.home) {
            agent.note_at_start(note);
        }
    }
    let fetch_allow = match fs.get_string("fetch-allow") {
        spec if !spec.trim().is_empty() => spec,
        _ => st
            .as_ref()
            .map_or_else(String::new, |s| s.setting("harness.fetch_allow", "")),
    };
    let fetch_policy = FetchPolicy::parse(&fetch_allow);
    if !fetch_policy.is_empty() {
        agent.note_at_start(format!("fetch tool on for {}", fetch_policy.describe()));
        agent.set_fetch_policy(fetch_policy);
    }
    // Conversations search the web by default, since the owner sits by
    // them; jobs do so when the setting says `on`.
    let web_search_on = !fs.get_bool("no-web-search")
        && (fs.get_bool("web-search")
            || match st
                .as_ref()
                .map_or_else(
                    || "chat".to_string(),
                    |s| s.setting("harness.web_search", "chat"),
                )
                .as_str()
            {
                "on" => true,
                "off" => false,
                _ => interactive,
            });
    if web_search_on {
        let search = WebSearch::from_env(&c.home);
        agent.note_at_start(format!("web_search tool on through {}", search.describe()));
        agent.set_web_search(search);
    }
    let permissions = permission_rules(fs, st.as_ref())?;
    if !permissions.is_empty() {
        let (allow, deny) = permissions.counts();
        agent.note_at_start(format!(
            "permissions: {allow} allow and {deny} deny rule{} ({})",
            if allow + deny == 1 { "" } else { "s" },
            permissions
                .iter()
                .map(|r| format!("{} {}", if r.allow { "allow" } else { "deny" }, r.pattern()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        agent.set_permissions(permissions);
    }
    let shared = shared_agent(agent);
    if let Some(ts) = trajectory_sink {
        shared
            .lock()
            .unwrap()
            .sinks
            .push(Box::new(ts.with_agent(shared.clone())));
    }
    Ok(Prepared {
        shared,
        provider: p,
        provider_name,
        model_name,
        session_id,
        cwd: abs_cwd,
        trajectory_path,
        st,
        step_log,
        rec_for_finish,
        _close_store: close_store,
    })
}

/// The permission rules from the settings (`harness.allow_tools`,
/// `harness.deny_tools`) and the flags (`--allow-tool`, `--deny-tool`).
fn permission_rules(fs: &FlagSet, st: Option<&Store>) -> Result<Rules> {
    use crate::permissions::Source;
    let mut rules = Rules::new();
    let parse = |allow: bool, specs: &[String], source: Source, what: &str| -> Result<Vec<Rule>> {
        Rules::parse_all(allow, specs, source)
            .map_err(|e| errorf(2, &format!("pulse harness: {what}: {e}"), "", "").into())
    };
    if let Some(s) = st {
        let allow = s.setting("harness.allow_tools", "");
        let deny = s.setting("harness.deny_tools", "");
        rules.extend(parse(
            true,
            &[allow],
            Source::Setting,
            "harness.allow_tools",
        )?);
        rules.extend(parse(
            false,
            &[deny],
            Source::Setting,
            "harness.deny_tools",
        )?);
    }
    rules.extend(parse(
        true,
        &fs.get_strings("allow-tool"),
        Source::Flag,
        "--allow-tool",
    )?);
    rules.extend(parse(
        false,
        &fs.get_strings("deny-tool"),
        Source::Flag,
        "--deny-tool",
    )?);
    Ok(rules)
}

fn harness_run(c: &mut Context, args: &[String]) -> Result<()> {
    let mut fs = FlagSet::new("harness run");
    fs.set_output(pulse_cli::flags::Output::Discard);
    fs.string_var("p", "", "task text");
    fs.string_var("task", "", "task text");
    fs.string_var("task-file", "", "read task from file, or - for stdin");
    add_agent_flags(&mut fs);
    bind_raw_globals(&mut fs);
    fs.parse(args)?;
    apply_raw_globals(c, &fs);
    if fs.narg() != 0 {
        return Err(errorf(
            2,
            &format!(
                "pulse harness run: unrecognized arguments: {}",
                fs.args().join(" ")
            ),
            "",
            "",
        )
        .into());
    }
    let task = read_task(&first_set(&fs, "p", "task"), &fs.get_string("task-file"))?;
    let mut prepared = prepare_agent(c, &fs, "run", task, false, Vec::new(), true)?;
    let cancel = CancelToken::new();
    let mut stop_watch = prepared.watch_cancel(&cancel);
    let result = match prepared.shared.lock().unwrap().run_with_cancel(&cancel) {
        Ok(result) => result,
        Err(err) => {
            if let Some(stop) = stop_watch.take() {
                stop();
            }
            prepared.write_trajectory(c)?;
            return Err(err);
        }
    };
    if let Some(stop) = stop_watch.take() {
        stop();
    }
    let trajectory_steps = prepared.shared.lock().unwrap().trajectory().steps;
    prepared.write_trajectory(c)?;
    prepared.finish(c, &result)?;
    if result.exit_status == EXIT_NOT_LOGGED_IN {
        if result.error.trim() == "not logged in" || result.error.trim().is_empty() {
            writeln!(c.err, "not logged in: {LOGIN_FIX}")?;
        } else {
            writeln!(c.err, "not logged in: {} - {}", result.error, LOGIN_FIX)?;
        }
    }
    let session_id = prepared.session_id.clone();
    let trajectory_path = prepared.trajectory_path.clone();
    if c.json {
        if !c.quiet {
            return write_json(
                &mut c.out,
                &run_json(&session_id, &result, &trajectory_path),
            );
        }
        if result.exit_status == EXIT_SUBMITTED {
            return Ok(());
        }
        return Err(errorf(exit_code_for(&result.exit_status), "", "", "").into());
    }
    write_run_text(&mut c.out, &trajectory_steps, &result)?;
    if result.exit_status == EXIT_SUBMITTED {
        Ok(())
    } else {
        Err(errorf(exit_code_for(&result.exit_status), "", "", "").into())
    }
}

fn harness_login(c: &mut Context, args: &[String]) -> Result<()> {
    let mut fs = FlagSet::new("harness login");
    fs.set_output(pulse_cli::flags::Output::Discard);
    fs.string_var("provider", pulse_provider::DEFAULT, "provider name");
    fs.bool_var("with-token", false, "read token from stdin");
    bind_raw_globals(&mut fs);
    fs.parse(args)?;
    apply_raw_globals(c, &fs);
    if fs.narg() != 0 {
        return Err(errorf(
            2,
            &format!(
                "pulse harness login: unrecognized arguments: {}",
                fs.args().join(" ")
            ),
            "",
            "",
        )
        .into());
    }
    let (st, _) = open_harness_store(c, false)?;
    let p = pulse_provider::open(
        &fs.get_string("provider"),
        harness_provider_options(c, st.as_ref()),
    )
    .map_err(|e| {
        errorf(
            2,
            &format!("pulse harness login: {e}"),
            "",
            "pass --provider P",
        )
    })?;
    let cancel = CancelToken::new();
    let login = if fs.get_bool("with-token") {
        let Some(tl) = p.as_token_login() else {
            return Err(errorf(
                2,
                &format!(
                    "pulse harness login: provider {} does not support token login",
                    p.label()
                ),
                "",
                "",
            )
            .into());
        };
        let mut data = String::new();
        read_stdin_to_string(&mut data)?;
        let token = data.trim();
        if token.is_empty() {
            return Err(errorf(
                2,
                "pulse harness login: empty token",
                "",
                "pipe the provider's API key or token to pulse harness login --with-token",
            )
            .into());
        }
        tl.login_with_token(&cancel, token)?
    } else {
        device_login_flow(c, p.as_ref(), &cancel)?
    };
    if c.json {
        write_json(&mut c.out, &login)
    } else {
        writeln!(c.out, "Signed in as {} ({})", login.user, p.label())?;
        Ok(())
    }
}

fn device_login_flow(c: &mut Context, p: &dyn Provider, cancel: &CancelToken) -> Result<Login> {
    let Some(dl) = p.as_device_login() else {
        return Err(errorf(
            2,
            &format!(
                "pulse harness login: provider {} does not support device login",
                p.label()
            ),
            "",
            "",
        )
        .into());
    };
    let flow = dl.begin_device_login(cancel)?;
    writeln!(
        c.err,
        "Open {} and enter the code {}",
        flow.verification_uri,
        c.style().bold(&flow.user_code)
    )?;
    let mut interval = if flow.interval <= 0 { 5 } else { flow.interval };
    let deadline = Instant::now() + Duration::from_secs(flow.expires_in.max(0) as u64);
    loop {
        if flow.expires_in > 0 && Instant::now() > deadline {
            return Err(errorf(
                2,
                "pulse harness login: device code expired",
                "",
                "run pulse harness login again",
            )
            .into());
        }
        thread::sleep(Duration::from_secs(interval as u64));
        let (state, login) = dl.poll_device_login(cancel, &flow.device_code)?;
        match state.as_str() {
            pulse_provider::DEVICE_COMPLETE => return Ok(login),
            pulse_provider::DEVICE_PENDING => {}
            pulse_provider::DEVICE_SLOW_DOWN => interval += 5,
            pulse_provider::DEVICE_EXPIRED => {
                return Err(errorf(
                    2,
                    "pulse harness login: device code expired",
                    "",
                    "run pulse harness login again",
                )
                .into());
            }
            pulse_provider::DEVICE_DENIED => {
                return Err(errorf(2, "pulse harness login: device login denied", "", "").into());
            }
            _ => {
                return Err(errorf(
                    2,
                    &format!("pulse harness login: unexpected device login state {state}"),
                    "",
                    "",
                )
                .into());
            }
        }
    }
}

fn harness_status(c: &mut Context, args: &[String]) -> Result<()> {
    let mut fs = FlagSet::new("harness status");
    fs.set_output(pulse_cli::flags::Output::Discard);
    fs.string_var("provider", "", "provider name");
    bind_raw_globals(&mut fs);
    fs.parse(args)?;
    apply_raw_globals(c, &fs);
    if fs.narg() != 0 {
        return Err(errorf(
            2,
            &format!(
                "pulse harness status: unrecognized arguments: {}",
                fs.args().join(" ")
            ),
            "",
            "",
        )
        .into());
    }
    let (st, _) = open_harness_store(c, false)?;
    let provider_name = if fs.get_string("provider").is_empty() {
        st.as_ref().map_or_else(
            || pulse_provider::DEFAULT.to_string(),
            |s| s.setting("provider", pulse_provider::DEFAULT),
        )
    } else {
        fs.get_string("provider")
    };
    let p = pulse_provider::open(&provider_name, harness_provider_options(c, st.as_ref()))
        .map_err(|e| {
            errorf(
                2,
                &format!("pulse harness status: {e}"),
                "",
                "pass --provider P",
            )
        })?;
    let login_res = p.status(&CancelToken::new());
    let (login, login_err) = match login_res {
        Ok(l) => (l, None),
        Err(e) => (
            Login {
                detail: one_line(&e.to_string(), 200),
                ..Login::default()
            },
            Some(e),
        ),
    };
    let status = status_report(st.as_ref(), p.as_ref(), login, login_err.as_ref());
    if c.json {
        write_json(&mut c.out, &status)
    } else {
        write_harness_status(&mut c.out, &status)
    }
}

fn harness_models(c: &mut Context, args: &[String]) -> Result<()> {
    let mut fs = FlagSet::new("harness models");
    fs.set_output(pulse_cli::flags::Output::Discard);
    fs.string_var("provider", pulse_provider::DEFAULT, "provider name");
    bind_raw_globals(&mut fs);
    fs.parse(args)?;
    apply_raw_globals(c, &fs);
    if fs.narg() != 0 {
        return Err(errorf(
            2,
            &format!(
                "pulse harness models: unrecognized arguments: {}",
                fs.args().join(" ")
            ),
            "",
            "",
        )
        .into());
    }
    let (st, _) = open_harness_store(c, false)?;
    let p = pulse_provider::open(
        &fs.get_string("provider"),
        harness_provider_options(c, st.as_ref()),
    )
    .map_err(|e| {
        errorf(
            2,
            &format!("pulse harness models: {e}"),
            "",
            "pass --provider P",
        )
    })?;
    let models = match p.models(&CancelToken::new()) {
        Ok(m) => m,
        Err(e) if pulse_provider::is_not_logged_in(&e) => {
            writeln!(c.err, "{}", not_logged_in_line(&e))?;
            return Err(errorf(4, "", "", "").into());
        }
        Err(e) => return Err(e),
    };
    if c.json {
        return write_json(&mut c.out, &models);
    }
    let rows: Vec<Vec<String>> = models
        .into_iter()
        .map(|m| {
            vec![
                if m.default {
                    format!("{} *", m.id)
                } else {
                    m.id
                },
                m.name,
                m.vendor,
                yes_no(m.tool_calls).into(),
                if m.premium > 0.0 {
                    format!("x{}", trim_float(m.premium))
                } else {
                    "-".into()
                },
                int_text(m.context_window),
                int_text(m.max_output),
            ]
        })
        .collect();
    table(
        &mut c.out,
        c.color,
        Some(&[
            "ID", "NAME", "VENDOR", "TOOLS", "PREMIUM", "CONTEXT", "OUTPUT",
        ]),
        &rows,
    )?;
    Ok(())
}

fn harness_logout(c: &mut Context, args: &[String]) -> Result<()> {
    let mut fs = FlagSet::new("harness logout");
    fs.set_output(pulse_cli::flags::Output::Discard);
    fs.string_var("provider", pulse_provider::DEFAULT, "provider name");
    bind_raw_globals(&mut fs);
    fs.parse(args)?;
    apply_raw_globals(c, &fs);
    if fs.narg() != 0 {
        return Err(errorf(
            2,
            &format!(
                "pulse harness logout: unrecognized arguments: {}",
                fs.args().join(" ")
            ),
            "",
            "",
        )
        .into());
    }
    let (st, _) = open_harness_store(c, false)?;
    let p = pulse_provider::open(
        &fs.get_string("provider"),
        harness_provider_options(c, st.as_ref()),
    )
    .map_err(|e| {
        errorf(
            2,
            &format!("pulse harness logout: {e}"),
            "",
            "pass --provider P",
        )
    })?;
    let Some(l) = p.as_logouter() else {
        return Err(errorf(
            2,
            &format!(
                "pulse harness logout: provider {} does not support logout",
                p.label()
            ),
            "",
            "",
        )
        .into());
    };
    l.logout()?;
    writeln!(c.out, "removed stored {} login", p.label())?;
    if let Ok(login) = p.status(&CancelToken::new())
        && login.logged_in
    {
        write!(c.out, "still signed in via {}", login.source)?;
        if !login.detail.is_empty() {
            write!(c.out, ": {}", login.detail)?;
        }
        writeln!(c.out)?;
    }
    Ok(())
}

fn harness_sessions(c: &mut Context, args: &[String]) -> Result<()> {
    let mut fs = FlagSet::new("harness sessions");
    fs.set_output(pulse_cli::flags::Output::Discard);
    fs.string_var("project", "", "project slug");
    fs.string_var("status", "", "session status");
    fs.int_var("limit", 50, "maximum sessions");
    bind_raw_globals(&mut fs);
    fs.parse(args)?;
    apply_raw_globals(c, &fs);
    if fs.narg() != 0 {
        return Err(errorf(
            2,
            &format!(
                "pulse harness sessions: unrecognized arguments: {}",
                fs.args().join(" ")
            ),
            "",
            "",
        )
        .into());
    }
    let (st, _) = require_harness_store(c)?;
    let sessions = st.harness_sessions(HarnessSessionFilter {
        project: fs.get_string("project"),
        status: fs.get_string("status"),
        limit: fs.get_int("limit") as i64,
        ..HarnessSessionFilter::default()
    })?;
    if c.json {
        write_json(&mut c.out, &sessions)
    } else {
        write_sessions_table(&mut c.out, c.color, &sessions)
    }
}

fn harness_show(c: &mut Context, args: &[String]) -> Result<()> {
    let mut fs = FlagSet::new("harness show");
    fs.set_output(pulse_cli::flags::Output::Discard);
    bind_raw_globals(&mut fs);
    fs.parse(args)?;
    apply_raw_globals(c, &fs);
    if fs.narg() != 1 {
        return Err(errorf(
            2,
            "pulse harness show: expected SESSION_ID",
            "",
            "pulse harness show SESSION_ID",
        )
        .into());
    }
    let (st, _) = require_harness_store(c)?;
    let (session, steps) = st.harness_session(fs.arg(0))?;
    let Some(session) = session else {
        return Err(errorf(
            2,
            &format!("pulse harness show: no harness session {}", fs.arg(0)),
            "",
            "pulse harness sessions",
        )
        .into());
    };
    if c.json {
        write_json(&mut c.out, &json!({"session": session, "steps": steps}))
    } else {
        write_harness_transcript(&mut c.out, &session, &steps)
    }
}

fn harness_cancel(c: &mut Context, args: &[String]) -> Result<()> {
    let mut fs = FlagSet::new("harness cancel");
    fs.set_output(pulse_cli::flags::Output::Discard);
    bind_raw_globals(&mut fs);
    fs.parse(args)?;
    apply_raw_globals(c, &fs);
    if fs.narg() != 1 {
        return Err(errorf(
            2,
            "pulse harness cancel: expected SESSION_ID",
            "",
            "pulse harness cancel SESSION_ID",
        )
        .into());
    }
    let (st, _) = require_harness_store(c)?;
    let id = fs.arg(0);
    let ok = match st.request_harness_cancel(id) {
        Ok(ok) => ok,
        Err(e) if e.downcast_ref::<rusqlite::Error>().is_some() => {
            return Err(errorf(
                2,
                &format!("pulse harness cancel: no harness session {id}"),
                "",
                "pulse harness sessions",
            )
            .into());
        }
        Err(e)
            if e.downcast_ref::<pulse_store::ErrHarnessCancelPending>()
                .is_some() =>
        {
            writeln!(c.out, "cancel already requested for {id}")?;
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    if !ok {
        writeln!(c.err, "pulse harness cancel: session is not running: {id}")?;
        return Err(errorf(1, "", "", "").into());
    }
    if let Ok(Some(head)) = st.harness_session_head(id) {
        let _ = pulse_bus::emit(
            &st,
            &actor_of(c),
            "harness.cancel",
            id,
            json!({"session": id, "job": head.job_id}),
        )?;
    }
    writeln!(c.out, "cancel requested for {id}")?;
    Ok(())
}

fn harness_prompt(c: &mut Context, args: &[String]) -> Result<()> {
    let mut fs = FlagSet::new("harness prompt");
    fs.set_output(pulse_cli::flags::Output::Discard);
    fs.string_var("C", ".", "working directory");
    fs.string_var("provider", "", "provider name");
    fs.string_var("model", "", "model id");
    fs.bool_var("default", false, "print the built-in prompt template");
    fs.bool_var(
        "raw",
        false,
        "print the stored prompt template without rendering",
    );
    bind_raw_globals(&mut fs);
    fs.parse(args)?;
    apply_raw_globals(c, &fs);
    if fs.narg() != 0 {
        return Err(errorf(
            2,
            &format!(
                "pulse harness prompt: unrecognized arguments: {}",
                fs.args().join(" ")
            ),
            "",
            "",
        )
        .into());
    }
    if fs.get_bool("default") {
        writeln!(c.out, "{DEFAULT_PROMPT_TEMPLATE}")?;
        return Ok(());
    }
    let (st, _) = open_harness_store(c, false)?;
    let mut tmpl = st
        .as_ref()
        .map_or_else(String::new, |s| s.setting("harness.system_prompt", ""));
    if fs.get_bool("raw") {
        writeln!(c.out, "{tmpl}")?;
        if tmpl.trim().is_empty() {
            writeln!(c.err, "built-in")?;
        }
        return Ok(());
    }
    if tmpl.is_empty() {
        tmpl = DEFAULT_PROMPT_TEMPLATE.to_string();
    }
    let abs_cwd = existing_dir(&fs.get_string("C"))?;
    let provider_name = if fs.get_string("provider").is_empty() {
        st.as_ref().map_or_else(
            || pulse_provider::DEFAULT.to_string(),
            |s| s.setting("provider", pulse_provider::DEFAULT),
        )
    } else {
        fs.get_string("provider")
    };
    let model_name = if fs.get_string("model").is_empty() {
        st.as_ref()
            .map_or_else(String::new, |s| s.run_model("task"))
    } else {
        fs.get_string("model")
    };
    let instructions = if st
        .as_ref()
        .is_none_or(|s| s.setting("harness.instructions", "on") != "off")
    {
        load_instructions(&abs_cwd).0
    } else {
        String::new()
    };
    let cfg = apply_defaults(Config {
        cwd: abs_cwd.clone(),
        provider: provider_name,
        model: model_name,
        instructions,
        ..Config::default()
    });
    let prompt = render_prompt(
        &tmpl,
        prompt_data(&LocalEnv::new(abs_cwd, None).info(), &cfg),
    )
    .map_err(|e| {
        errorf(
            2,
            &format!("pulse harness prompt: harness.system_prompt render failed: {e}"),
            "",
            "fix harness.system_prompt",
        )
    })?;
    writeln!(c.out, "{prompt}")?;
    Ok(())
}

pub(crate) fn bind_raw_globals(fs: &mut FlagSet) {
    fs.bool_var("json", false, "machine-readable output");
    fs.bool_var("quiet", false, "print only what a script needs");
    fs.bool_var("no-color", false, "no colour");
}

pub(crate) fn apply_raw_globals(c: &mut Context, fs: &FlagSet) {
    if fs.get_bool("json") {
        c.json = true;
    }
    if fs.get_bool("quiet") {
        c.quiet = true;
    }
    if fs.get_bool("no-color") {
        c.color = false;
    }
}

pub(crate) fn first_set(fs: &FlagSet, a: &str, b: &str) -> String {
    if fs.is_set(b) {
        fs.get_string(b)
    } else {
        fs.get_string(a)
    }
}

pub fn read_task(text: &str, file: &str) -> Result<String> {
    if text.trim().is_empty() && file.is_empty() {
        return Err(errorf(
            2,
            "pulse harness run: exactly one of -p/--task or --task-file is required",
            "",
            "pass -p TEXT or --task-file F",
        )
        .into());
    }
    if !text.trim().is_empty() && !file.is_empty() {
        return Err(errorf(
            2,
            "pulse harness run: exactly one of -p/--task or --task-file is required",
            "",
            "pass only one task source",
        )
        .into());
    }
    if file.is_empty() {
        return Ok(text.to_string());
    }
    let data = if file == "-" {
        let mut s = String::new();
        read_stdin_to_string(&mut s)?;
        s
    } else {
        fs::read_to_string(file)?
    };
    if data.trim().is_empty() {
        return Err(errorf(2, "pulse harness run: task is empty", "", "").into());
    }
    Ok(data)
}

#[cfg(test)]
pub(crate) static STDIN_FOR_TESTS: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

#[cfg(test)]
pub(crate) fn set_stdin_for_tests(data: Option<&str>) {
    *STDIN_FOR_TESTS.lock().expect("stdin test hook") = data.map(str::to_string);
}

pub(crate) fn read_stdin_to_string(dst: &mut String) -> std::io::Result<usize> {
    #[cfg(test)]
    if let Some(data) = STDIN_FOR_TESTS.lock().expect("stdin test hook").clone() {
        dst.push_str(&data);
        return Ok(data.len());
    }
    std::io::stdin().read_to_string(dst)
}

pub fn existing_dir(path: &str) -> Result<String> {
    let abs = PathBuf::from(path).canonicalize().unwrap_or_else(|_| {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            p
        } else {
            std::env::current_dir().unwrap().join(p)
        }
    });
    let meta = fs::metadata(&abs).map_err(|_| {
        errorf(
            2,
            &format!(
                "pulse harness run: working directory does not exist: {}",
                abs.display()
            ),
            "",
            "",
        )
    })?;
    if !meta.is_dir() {
        return Err(errorf(
            2,
            &format!(
                "pulse harness run: working directory is not a directory: {}",
                abs.display()
            ),
            "",
            "",
        )
        .into());
    }
    Ok(abs.display().to_string())
}

type StoreClose = Option<fn()>;

pub fn open_harness_store(c: &Context, no_store: bool) -> Result<(Option<Store>, StoreClose)> {
    if no_store {
        return Ok((None, None));
    }
    if let Some(s) = &c.store {
        return Ok((Some(s.clone()), Some(|| {})));
    }
    let path = Path::new(&c.home).join(STORE_NAME);
    if !path.exists() {
        return Ok((None, None));
    }
    let st = Store::open(pulse_store::Options {
        home: c.home.clone(),
        now: c.now.clone(),
        ..pulse_store::Options::default()
    })?;
    Ok((Some(st), Some(|| {})))
}

fn require_harness_store(c: &Context) -> Result<(Store, StoreClose)> {
    let (st, close) = open_harness_store(c, false)?;
    st.map(|s| (s, close)).ok_or_else(|| {
        errorf(
            2,
            &format!(
                "pulse harness: no store at {}",
                Path::new(&c.home).join(STORE_NAME).display()
            ),
            "",
            "pulse init <project-path>",
        )
        .into()
    })
}

pub fn harness_provider_options(c: &Context, st: Option<&Store>) -> ProviderOptions {
    ProviderOptions {
        home: c.home.clone(),
        settings: st.map_or_else(BTreeMap::new, Store::provider_settings),
        ..ProviderOptions::default()
    }
}

/// How long a run waits for one MCP server to answer `initialize` before
/// it goes on without it: a cold `npx` server takes a while, a dead one
/// should not hold the run for the client's full 90 s.
pub const MCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(45);

/// Connects the enabled MCP servers of `project` and offers their tools to
/// the agent. Returns the notes to emit: one per connected server, one per
/// server that did not answer (the run goes on without it).
pub fn attach_mcp_servers(agent: &mut Agent, s: &Store, project: &str, home: &str) -> Vec<String> {
    let mut notes = Vec::new();
    let servers = match s.mcp_servers_for(project) {
        Ok(v) => v,
        Err(e) => return vec![format!("MCP servers not read from the store: {e}")],
    };
    if servers.is_empty() {
        return notes;
    }
    let resolver = pulse_mcp::resolver(home);
    for server in servers {
        let connected = pulse_mcp::prepare(&server, &resolver)
            .and_then(|wire| pulse_mcp::Client::connect_with(&server, wire, MCP_CONNECT_TIMEOUT))
            .and_then(|mut cl| {
                let tools = cl.tools()?;
                Ok((cl, tools))
            });
        match connected {
            Ok((cl, tools)) => {
                notes.push(format!(
                    "MCP server {} connected: {} tool{} offered as mcp__{}__<tool>",
                    server.name,
                    tools.len(),
                    if tools.len() == 1 { "" } else { "s" },
                    server.name
                ));
                agent.add_mcp_server(McpServerTools {
                    server: server.name.clone(),
                    tools,
                    backend: Box::new(cl),
                });
            }
            Err(e) => notes.push(format!(
                "MCP server {} unavailable, the run goes on without it: {}",
                server.name,
                one_line(&pulse_cred::redact(&e.to_string(), &[]), 300)
            )),
        }
    }
    notes
}

/// Offers the clone's, the owner's and the registry's skills through the
/// `skill` tool. Returns the start note when there are any.
pub fn attach_skills(
    agent: &mut Agent,
    st: Option<&Store>,
    project: &str,
    home: &str,
) -> Option<String> {
    let mut skills = load_skills(&agent.cfg.cwd, home);
    let folders = skills.len();
    let mut registry = 0;
    if let Some(s) = st {
        match s.skills_for(project) {
            Ok(list) => {
                for k in list {
                    if skills.iter().any(|x| x.name == k.name) {
                        continue;
                    }
                    skills.push(Skill::inline(
                        &k.name,
                        &k.description,
                        &k.body,
                        "pulse skill",
                    ));
                    registry += 1;
                }
            }
            Err(e) => {
                agent.note_at_start(format!("skill registry unavailable: {e}"));
            }
        }
    }
    if skills.is_empty() {
        return None;
    }
    let mut sources: Vec<String> = Vec::new();
    for k in &skills {
        if !sources.contains(&k.source) {
            sources.push(k.source.clone());
        }
    }
    let names: Vec<&str> = skills.iter().take(12).map(|k| k.name.as_str()).collect();
    let note = format!(
        "offered {} skill(s) through the skill tool ({} from {}, {} from the pulse skill registry): {}{}",
        skills.len(),
        folders,
        sources
            .iter()
            .filter(|s| s.as_str() != "pulse skill")
            .cloned()
            .collect::<Vec<_>>()
            .join(", "),
        registry,
        names.join(", "),
        if skills.len() > names.len() {
            format!(" and {} more", skills.len() - names.len())
        } else {
            String::new()
        }
    );
    agent.set_skills(skills);
    Some(note)
}

/// Runs the owner's hooks from the clone and the home around the run.
/// Returns the start notes: what was loaded and what did not parse.
pub fn attach_hooks(agent: &mut Agent, home: &str) -> Vec<String> {
    let set = load_hooks(&agent.cfg.cwd, home);
    let mut notes: Vec<String> = set
        .errors
        .iter()
        .map(|e| format!("hook file skipped: {e}"))
        .collect();
    if set.hooks.is_empty() {
        return notes;
    }
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for h in &set.hooks {
        *counts.entry(h.event.to_string()).or_default() += 1;
    }
    notes.push(format!(
        "loaded {} hook(s) from {}: {}",
        set.hooks.len(),
        set.files.join(", "),
        counts
            .iter()
            .map(|(e, n)| format!("{n} {e}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    agent.set_hooks(set);
    notes
}

pub fn default_run_model(
    cancel: &CancelToken,
    st: Option<&Store>,
    p: &dyn Provider,
) -> Result<String> {
    if let Some(st) = st {
        let model = st.run_model("task");
        if !model.trim().is_empty() {
            return Ok(model);
        }
    }
    let models = p.models(cancel)?;
    for m in &models {
        if m.default && m.chat {
            return Ok(m.id.clone());
        }
    }
    for m in &models {
        if m.chat {
            return Ok(m.id.clone());
        }
    }
    Err(errorf(
        2,
        "pulse harness run: no default chat model available",
        "",
        "pass --model M",
    )
    .into())
}

pub fn model_context_window(cancel: &CancelToken, p: &dyn Provider, model: &str) -> i32 {
    p.models(cancel)
        .ok()
        .and_then(|ms| {
            ms.into_iter()
                .find(|m| m.id == model)
                .map(|m| m.context_window)
        })
        .unwrap_or(0)
}

pub fn new_harness_session_id() -> String {
    let b: [u8; 3] = rand::random();
    format!(
        "hs-{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S"),
        hex::encode(b)
    )
}

fn setting_int_or(fs: &FlagSet, flag: &str, st: Option<&Store>, setting: &str, def: i32) -> i32 {
    let v = fs.get_int(flag);
    if v != 0 {
        v
    } else {
        st.map_or(def, |s| s.setting_int(setting) as i32)
    }
}

#[derive(Clone)]
pub(crate) struct StoreSink {
    agent: String,
    id: String,
    s: Store,
    provider: String,
    model: String,
    warned: bool,
}

impl StoreSink {
    fn new(c: &Context, s: Store, id: &str) -> Self {
        Self {
            agent: c.agent.clone(),
            id: id.to_string(),
            s,
            provider: String::new(),
            model: String::new(),
            warned: false,
        }
    }
    fn clone_for_finish(&self) -> Self {
        self.clone()
    }
    fn begin(
        &mut self,
        provider_name: &str,
        model: &str,
        cwd: &str,
        trajectory: &str,
    ) -> Result<()> {
        self.provider = provider_name.into();
        self.model = model.into();
        self.s.begin_harness_session(HarnessSession {
            id: self.id.clone(),
            run_id: std::env::var("PULSE_SESSION").unwrap_or_default(),
            job_id: std::env::var("PULSE_JOB").unwrap_or_default(),
            agent: std::env::var("PULSE_AGENT").unwrap_or_default(),
            project: std::env::var("PULSE_PROJECT").unwrap_or_default(),
            executor: std::env::var("PULSE_EXECUTOR").unwrap_or_default(),
            provider: provider_name.into(),
            model: model.into(),
            cwd: cwd.into(),
            status: "running".into(),
            trajectory: trajectory.into(),
            ..HarnessSession::default()
        })?;
        pulse_bus::emit(
            &self.s,
            &actor_env(&self.agent),
            "harness.start",
            &self.id,
            json!({
                "project": std::env::var("PULSE_PROJECT").unwrap_or_default(), "agent": std::env::var("PULSE_AGENT").unwrap_or_default(), "job": std::env::var("PULSE_JOB").unwrap_or_default(),
                "model": model, "provider": provider_name
            }),
        )?;
        Ok(())
    }
    fn finish(&mut self, result: &ResultSummary, trajectory: &str) -> Result<()> {
        self.s.finish_harness_session(
            &self.id,
            &result.exit_status,
            &result.exit_status,
            &result.submission,
            &result.error,
            result.usage.cost,
            trajectory,
        )?;
        pulse_bus::emit(
            &self.s,
            &actor_env(&self.agent),
            "harness.finish",
            &self.id,
            json!({
                "project": std::env::var("PULSE_PROJECT").unwrap_or_default(), "agent": std::env::var("PULSE_AGENT").unwrap_or_default(), "job": std::env::var("PULSE_JOB").unwrap_or_default(),
                "model": self.model, "provider": self.provider, "status": result.exit_status, "steps": result.steps, "calls": result.calls
            }),
        )?;
        Ok(())
    }
}

impl Sink for StoreSink {
    fn step(&mut self, step: Step) {
        if let Err(e) = self.s.append_harness_step(HarnessStep {
            session_id: self.id.clone(),
            n: step.n as i64,
            at: step.at.clone(),
            kind: step.kind.to_string(),
            content: step.content.clone(),
            command: step.command.clone(),
            exit_code: step.exit_code.map(i64::from),
            duration_ms: step.duration_ms,
            prompt_tokens: step.usage.prompt_tokens as i64,
            completion_tokens: step.usage.completion_tokens as i64,
            model: step.model.clone(),
            finish_reason: step.finish_reason.clone(),
            choices: step.choices,
            depth: i64::from(step.depth),
        }) {
            self.warn(&e);
            return;
        }
        let mut payload = json!({"session": self.id, "n": step.n, "kind": step.kind.to_string()});
        if let Value::Object(ref mut map) = payload {
            if let Some(code) = step.exit_code {
                map.insert("exit_code".into(), json!(code));
            }
            if step.depth > 0 {
                map.insert("depth".into(), json!(step.depth));
            }
            if let Ok(job) = std::env::var("PULSE_JOB")
                && !job.is_empty()
            {
                map.insert("job".into(), json!(job));
            }
            if let Ok(run_id) = std::env::var("PULSE_SESSION")
                && !run_id.is_empty()
            {
                map.insert("run_id".into(), json!(run_id));
            }
        }
        if let Err(e) = pulse_bus::emit(
            &self.s,
            &actor_env(&self.agent),
            "harness.step",
            &self.id,
            payload,
        ) {
            self.warn(&e);
        }
    }
}

impl StoreSink {
    fn warn(&mut self, _err: &anyhow::Error) {
        self.warned = true;
    }
}

pub(crate) struct StepsFileSink {
    path: String,
    session: String,
    provider: String,
    model: String,
    warned: bool,
    warning: Option<String>,
    count: i32,
}

impl StepsFileSink {
    fn new(_c: &Context, path: &str, session: &str) -> Self {
        Self {
            path: path.into(),
            session: session.into(),
            provider: String::new(),
            model: String::new(),
            warned: false,
            warning: None,
            count: 0,
        }
    }
    fn begin(&mut self, provider_name: &str, model: &str, cwd: &str) {
        self.provider = provider_name.into();
        self.model = model.into();
        self.write(&json!({
            "event": "begin", "session": self.session, "run_id": std::env::var("PULSE_SESSION").unwrap_or_default(), "job_id": std::env::var("PULSE_JOB").unwrap_or_default(),
            "agent": std::env::var("PULSE_AGENT").unwrap_or_default(), "provider": provider_name, "model": model, "cwd": cwd, "at": now_rfc3339()
        }));
    }
    fn finish(&mut self, result: &ResultSummary) {
        self.write(&json!({"event":"finish","exit_status":result.exit_status,"submission":result.submission,"error":result.error,"cost":result.usage.cost,"steps":self.count,"at":now_rfc3339()}));
    }
    fn write<T: Serialize>(&mut self, v: &T) {
        if self.warned || self.path.is_empty() {
            return;
        }
        if let Some(parent) = Path::new(&self.path).parent()
            && let Err(e) = fs::create_dir_all(parent)
        {
            self.warned = true;
            self.warning = Some(e.to_string());
            return;
        }
        let mut data = match pulse_store::gojson::to_vec(v) {
            Ok(d) => d,
            Err(_) => {
                self.warned = true;
                self.warning = Some("JSON encode failed".into());
                return;
            }
        };
        data.push(b'\n');
        let mut f = match fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(f) => f,
            Err(e) => {
                self.warned = true;
                self.warning = Some(e.to_string());
                return;
            }
        };
        if let Err(e) = f.write_all(&data) {
            self.warned = true;
            self.warning = Some(e.to_string());
        }
    }
}

impl Sink for StepsFileSink {
    fn step(&mut self, step: Step) {
        self.count += 1;
        self.write(&StepEvent {
            event: "step",
            step,
        });
    }
}

struct SharedStepsFileSink(std::sync::Arc<std::sync::Mutex<StepsFileSink>>);

impl Sink for SharedStepsFileSink {
    fn step(&mut self, step: Step) {
        self.0.lock().expect("steps-file sink poisoned").step(step);
    }
}

#[derive(Serialize)]
struct StepEvent {
    event: &'static str,
    #[serde(flatten)]
    step: Step,
}

struct TrajectorySink {
    path: String,
    steps: Vec<Step>,
    warned: bool,
}

impl TrajectorySink {
    fn new(_c: &Context, path: &str) -> Self {
        Self {
            path: path.into(),
            steps: Vec::new(),
            warned: false,
        }
    }
    fn with_agent(self, _agent: std::sync::Arc<std::sync::Mutex<Agent>>) -> Self {
        self
    }
}

impl Sink for TrajectorySink {
    fn step(&mut self, step: Step) {
        if self.warned || self.path.is_empty() {
            return;
        }
        self.steps.push(step);
        if !self.steps.len().is_multiple_of(5) {
            return;
        }
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
        if write_trajectory(&self.path, &traj).is_err() {
            self.warned = true;
        }
    }
}

#[derive(Clone)]
pub(crate) struct SharedWriter(Arc<Mutex<Box<dyn Write + Send>>>);

impl SharedWriter {
    pub(crate) fn new(w: Box<dyn Write + Send>) -> Self {
        Self(Arc::new(Mutex::new(w)))
    }
}

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("shared writer poisoned").write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.lock().expect("shared writer poisoned").flush()
    }
}

struct ProgressSink {
    w: SharedWriter,
    start: Instant,
}

impl ProgressSink {
    fn new(w: SharedWriter, start: Instant) -> Self {
        Self { w, start }
    }
}

impl Sink for ProgressSink {
    fn step(&mut self, step: Step) {
        let elapsed = self.start.elapsed().as_secs();
        let prefix = format!(
            "{:02}:{:02} [{}] {}",
            elapsed / 60,
            elapsed % 60,
            step.n,
            "  ".repeat(usize::from(step.depth))
        );
        match step.kind {
            StepKind::Assistant => {
                let _ = writeln!(
                    self.w,
                    "{}assistant: {}",
                    prefix,
                    one_line(&step.content, 160)
                );
            }
            StepKind::Command => {
                let _ = writeln!(self.w, "{}$ {}", prefix, one_line(&step.command, 160));
            }
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
                let _ = writeln!(
                    self.w,
                    "{}{}: {}",
                    prefix,
                    step.kind,
                    one_line(&step.command, 160)
                );
            }
            StepKind::Subagent => {
                let _ = writeln!(
                    self.w,
                    "{}subagent: {}",
                    prefix,
                    one_line(&step.content, 160)
                );
            }
            StepKind::Plan => {
                let _ = writeln!(self.w, "{}plan: {}", prefix, one_line(&step.content, 160));
            }
            StepKind::Observation => {
                let code = step.exit_code.unwrap_or(0);
                let _ = writeln!(
                    self.w,
                    "{}exit {} in {}ms ({} chars)",
                    prefix,
                    code,
                    step.duration_ms,
                    step.content.len()
                );
            }
            StepKind::Note => {
                let _ = writeln!(self.w, "{}note: {}", prefix, one_line(&step.content, 160));
            }
            StepKind::FormatRetry => {
                let _ = writeln!(
                    self.w,
                    "{}format_retry: {}",
                    prefix,
                    one_line(&step.content, 160)
                );
            }
            StepKind::Error => {
                let _ = writeln!(self.w, "{}error: {}", prefix, one_line(&step.content, 160));
            }
            StepKind::Exit => {
                let _ = writeln!(self.w, "{}exit: {}", prefix, one_line(&step.content, 160));
            }
        }
    }
}

pub fn actor_of(c: &Context) -> String {
    if !c.agent.is_empty() {
        c.agent.clone()
    } else {
        actor_env("")
    }
}

fn actor_env(agent: &str) -> String {
    if !agent.is_empty() {
        agent.to_string()
    } else if let Ok(a) = std::env::var("PULSE_AGENT") {
        if a.is_empty() {
            pulse_store::HUMAN_AGENT.into()
        } else {
            a
        }
    } else {
        pulse_store::HUMAN_AGENT.into()
    }
}

pub fn run_json(
    session: &str,
    result: &ResultSummary,
    trajectory: &str,
) -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("session".into(), json!(session)),
        ("exit_status".into(), json!(result.exit_status)),
        ("submission".into(), json!(result.submission)),
        ("steps".into(), json!(result.steps)),
        ("calls".into(), json!(result.calls)),
        ("format_errors".into(), json!(result.format_errors)),
        ("compactions".into(), json!(result.compactions)),
        ("subagents".into(), json!(result.subagents)),
        (
            "usage".into(),
            json!(pulse_store::gomap! {
                "prompt_tokens": result.usage.prompt_tokens,
                "completion_tokens": result.usage.completion_tokens,
                "cost": result.usage.cost
            }),
        ),
        (
            "elapsed_seconds".into(),
            json!(result.elapsed.as_secs_f64()),
        ),
        ("error".into(), json!(result.error)),
        ("trajectory".into(), json!(trajectory)),
    ])
}

pub fn write_run_text(w: &mut dyn Write, steps: &[Step], result: &ResultSummary) -> Result<()> {
    if result.exit_status == EXIT_SUBMITTED {
        write!(w, "{}", result.submission)?;
        if !result.submission.ends_with('\n') {
            writeln!(w)?;
        }
        return Ok(());
    }
    let mut summary = format!(
        "SUMMARY: harness stopped: {} after {} model calls and {} commands; format_errors={}",
        result.exit_status, result.calls, result.steps, result.format_errors
    );
    if !result.error.is_empty() {
        summary.push_str("; error: ");
        summary.push_str(&one_line(&result.error, 200));
    }
    writeln!(w, "{summary}")?;
    if let Some(s) = steps
        .iter()
        .rev()
        .find(|s| s.kind == StepKind::Assistant && !s.content.trim().is_empty())
    {
        writeln!(w, "FINDING: last message: {}", one_line(&s.content, 200))?;
    }
    Ok(())
}

pub fn exit_code_for(status: &str) -> i32 {
    match status {
        EXIT_SUBMITTED | EXIT_ENDED => 0,
        EXIT_PROVIDER_ERROR => 3,
        EXIT_NOT_LOGGED_IN => 4,
        EXIT_CANCELLED => 5,
        EXIT_SANDBOX_LOST => 6,
        EXIT_STEP_LIMIT | EXIT_TIME_LIMIT | EXIT_COST_LIMIT | EXIT_FORMAT_ERROR => 1,
        _ => 1,
    }
}

#[derive(Serialize)]
pub struct HarnessStatusReport {
    harness: String,
    provider: ProviderStatus,
    models: ModelStatus,
    limits: LimitStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    sessions: Option<SessionStatus>,
}

#[derive(Serialize)]
struct ProviderStatus {
    name: String,
    label: String,
    login: Login,
}
#[derive(Serialize)]
struct ModelStatus {
    worker: String,
    light: String,
}
#[derive(Serialize)]
struct LimitStatus {
    max_steps: i32,
    max_cost: f64,
    step_timeout_seconds: i32,
    output_limit: i32,
}
#[derive(Serialize)]
struct SessionStatus {
    running: i64,
    today: i64,
    failed_today: i64,
    submitted_today: i64,
    cost_today: f64,
}

pub fn status_report(
    st: Option<&Store>,
    p: &dyn Provider,
    login: Login,
    login_err: Option<&anyhow::Error>,
) -> HarnessStatusReport {
    let mut r = HarnessStatusReport {
        harness: "native".into(),
        provider: ProviderStatus {
            name: p.name().into(),
            label: p.label().into(),
            login,
        },
        models: ModelStatus {
            worker: String::new(),
            light: "same as worker".into(),
        },
        limits: LimitStatus {
            max_steps: DEFAULT_MAX_STEPS,
            max_cost: 0.0,
            step_timeout_seconds: DEFAULT_STEP_TIMEOUT.as_secs() as i32,
            output_limit: DEFAULT_OUTPUT_LIMIT as i32,
        },
        sessions: None,
    };
    if let Some(st) = st {
        r.harness = st.setting("harness", "native");
        r.models.worker = st.setting("worker_model", "");
        let light = st.setting("light_model", "");
        if !light.is_empty() {
            r.models.light = light;
        }
        r.limits.max_steps = st.setting_int("harness.max_steps") as i32;
        r.limits.max_cost = st.setting_float("harness.max_cost");
        r.limits.step_timeout_seconds = st.setting_int("harness.step_timeout") as i32;
        r.limits.output_limit = st.setting_int("harness.output_limit") as i32;
        if let Ok(c) = st.harness_session_counts() {
            r.sessions = Some(SessionStatus {
                running: c.running,
                today: c.today,
                failed_today: c.failed_today,
                submitted_today: c.submitted_today,
                cost_today: c.cost_today,
            });
        }
    }
    if let Some(e) = login_err {
        r.provider.login.detail = format!("status unknown ({})", one_line(&e.to_string(), 200));
    }
    r
}

pub fn write_harness_status(w: &mut dyn Write, r: &HarnessStatusReport) -> Result<()> {
    let mut login = "not signed in".to_string();
    if r.provider.login.logged_in {
        login = format!("signed in as {}", r.provider.login.user);
        if !r.provider.login.source.is_empty() {
            login.push_str(&format!(" via {}", r.provider.login.source));
        }
    } else if !r.provider.login.detail.is_empty() {
        login.push_str(&format!(" - {}", r.provider.login.detail));
    } else {
        login.push_str(&format!(" - {LOGIN_FIX}"));
    }
    write!(
        w,
        "harness  {}\nprovider {} ({})\nlogin    {}\n",
        r.harness, r.provider.label, r.provider.name, login
    )?;
    writeln!(
        w,
        "models   worker={}, light={}",
        or_default(&r.models.worker, "provider default"),
        r.models.light
    )?;
    writeln!(
        w,
        "limits   max_steps={}, max_cost={}, step_timeout={}s, output_limit={}",
        r.limits.max_steps,
        trim_float(r.limits.max_cost),
        r.limits.step_timeout_seconds,
        r.limits.output_limit
    )?;
    if let Some(s) = &r.sessions {
        writeln!(
            w,
            "sessions running={}, today={}, submitted_today={}, failed_today={}, cost_today={}",
            s.running,
            s.today,
            s.submitted_today,
            s.failed_today,
            trim_float(s.cost_today)
        )?;
    }
    Ok(())
}

pub fn write_sessions_table(
    w: &mut dyn Write,
    color: bool,
    sessions: &[HarnessSession],
) -> Result<()> {
    let rows: Vec<Vec<String>> = sessions
        .iter()
        .map(|h| {
            vec![
                h.id.clone(),
                h.status.clone(),
                h.agent.clone(),
                h.project.clone(),
                h.model.clone(),
                h.steps.to_string(),
                h.calls.to_string(),
                (h.prompt_tokens + h.completion_tokens).to_string(),
                h.started_at.clone(),
            ]
        })
        .collect();
    table(
        w,
        color,
        Some(&[
            "ID", "STATUS", "AGENT", "PROJECT", "MODEL", "STEPS", "CALLS", "TOKENS", "STARTED",
        ]),
        &rows,
    )?;
    Ok(())
}

pub fn write_harness_transcript(
    w: &mut dyn Write,
    h: &HarnessSession,
    steps: &[HarnessStep],
) -> Result<()> {
    writeln!(
        w,
        "{} {} project={} agent={} model={}\n",
        h.id, h.status, h.project, h.agent, h.model
    )?;
    for s in steps {
        // A subagent's steps carry their depth: `[12 >1]`.
        let tag = if s.depth > 0 {
            format!("{} >{}", s.n, s.depth)
        } else {
            s.n.to_string()
        };
        match s.kind.as_str() {
            "assistant" => writeln!(w, "[{}] assistant\n{}\n", tag, s.content)?,
            "command" => writeln!(w, "[{}] $ {}\n", tag, s.command)?,
            "observation" => writeln!(
                w,
                "[{}] output exit={} duration={}ms\n{}\n",
                tag,
                s.exit_code.map_or("-".into(), |c| c.to_string()),
                s.duration_ms,
                head_tail(&s.content, 20)
            )?,
            "error" => writeln!(w, "[{}] error: {}\n", tag, s.content)?,
            "exit" => writeln!(w, "[{}] exit: {}\n", tag, s.content)?,
            "plan" => writeln!(w, "[{}] {}\n", tag, s.content)?,
            "read" | "grep" | "glob" | "edit" | "write" | "mcp" | "skill" | "fetch" | "process"
            | "memory" | "search" | "subagent" | "hook" => {
                writeln!(w, "[{}] {} {}\n", tag, s.kind, head_tail(&s.content, 20))?
            }
            _ => {}
        }
    }
    writeln!(w, "exit_status: {}", h.exit_status)?;
    if !h.error.is_empty() {
        writeln!(w, "error: {}", h.error)?;
    }
    if !h.submission.is_empty() {
        writeln!(w, "submission:\n{}", h.submission)?;
    }
    Ok(())
}

pub fn head_tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= 2 * n {
        lines.join("\n")
    } else {
        lines[..n]
            .iter()
            .chain(["..."].iter())
            .chain(lines[lines.len() - n..].iter())
            .copied()
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn yes_no(ok: bool) -> &'static str {
    if ok { "yes" } else { "no" }
}
fn int_text(n: i32) -> String {
    if n == 0 { "-".into() } else { n.to_string() }
}
fn or_default<'a>(s: &'a str, def: &'a str) -> &'a str {
    if s.is_empty() { def } else { s }
}
fn trim_float(f: f64) -> String {
    format!("{f}")
}

fn watch_harness_cancel(st: Store, session_id: String, cancel: CancelToken) -> impl FnOnce() {
    // The stop closure drops the sender; the watch wakes at once instead
    // of finishing its sleep (a chat turn ends when the model answers).
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let handle = thread::spawn(move || {
        loop {
            if let Ok(Some(h)) = st.harness_session_head(&session_id)
                && !h.cancel_requested_at.is_empty()
            {
                cancel.cancel();
                return;
            }
            match stop_rx.recv_timeout(Duration::from_secs(2)) {
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    move || {
        drop(stop_tx);
        let _ = handle.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_cli::Buffer;
    use pulse_provider::{Model, Request, Response, Usage};
    use pulse_testutil::httptest::{Response as TestResponse, Server};
    use serde_json::Value;
    use std::sync::{Arc, Mutex, Once, OnceLock};

    const FAKE_PROVIDER_NAME: &str = "fake-harness-cli";
    static REGISTER_FAKE: Once = Once::new();
    static FAKE_CURRENT: Mutex<Option<CommandFakeProvider>> = Mutex::new(None);
    static TEMP_DIRS: OnceLock<Mutex<Vec<tempfile::TempDir>>> = OnceLock::new();

    /// One scripted reply: plain text, or a tool call with JSON arguments.
    #[derive(Clone)]
    struct Scripted {
        content: String,
        tool: Option<(String, String)>,
    }

    fn say(content: &str) -> Scripted {
        Scripted {
            content: content.into(),
            tool: None,
        }
    }

    fn call(tool: &str, args: Value) -> Scripted {
        Scripted {
            content: String::new(),
            tool: Some((tool.into(), args.to_string())),
        }
    }

    #[derive(Clone)]
    struct CommandFakeProvider {
        label: String,
        models: Vec<Model>,
        login: Login,
        err_not_logged_in: bool,
        requests: Arc<Mutex<Vec<Request>>>,
        /// Replies handed out in order; the fixed report when it runs out.
        script: Arc<Mutex<Vec<Scripted>>>,
    }

    impl Default for CommandFakeProvider {
        fn default() -> Self {
            Self {
                label: String::new(),
                models: Vec::new(),
                login: Login::default(),
                err_not_logged_in: false,
                requests: Arc::new(Mutex::new(Vec::new())),
                script: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl CommandFakeProvider {
        fn requests(&self) -> Vec<Request> {
            self.requests.lock().unwrap().clone()
        }

        fn scripted(replies: Vec<Scripted>) -> Self {
            Self {
                script: Arc::new(Mutex::new(replies)),
                ..Self::default()
            }
        }
    }

    impl Provider for CommandFakeProvider {
        fn name(&self) -> &str {
            FAKE_PROVIDER_NAME
        }
        fn label(&self) -> &str {
            if self.label.is_empty() {
                "Fake Harness"
            } else {
                &self.label
            }
        }
        fn chat(&self, _cancel: &CancelToken, req: Request) -> anyhow::Result<Response> {
            self.requests.lock().unwrap().push(req);
            if self.err_not_logged_in {
                return Err(pulse_provider::err_not_logged_in().into());
            }
            let n = self.requests.lock().unwrap().len();
            let next = {
                let mut script = self.script.lock().unwrap();
                if script.is_empty() {
                    None
                } else {
                    Some(script.remove(0))
                }
            };
            let (content, tool_calls, finish) = match next {
                Some(Scripted {
                    content,
                    tool: Some((name, arguments)),
                }) => (
                    content,
                    vec![pulse_provider::ToolCall {
                        id: format!("call-{n}"),
                        name,
                        arguments,
                    }],
                    "tool_calls",
                ),
                Some(Scripted {
                    content,
                    tool: None,
                }) => (content, Vec::new(), "stop"),
                None => (
                    "SUMMARY: fake complete\nFINDING: ok".to_string(),
                    Vec::new(),
                    "stop",
                ),
            };
            Ok(Response {
                message: Some(pulse_provider::Message {
                    role: pulse_provider::Role::Assistant,
                    content,
                    tool_calls,
                    tool_call_id: String::new(),
                    name: String::new(),
                }),
                finish_reason: finish.into(),
                usage: Usage {
                    prompt_tokens: 7,
                    completion_tokens: 11,
                    cost: 0.25,
                },
                model: "fake-model".into(),
                ..Response::default()
            })
        }
        fn models(&self, _cancel: &CancelToken) -> anyhow::Result<Vec<Model>> {
            if self.err_not_logged_in {
                return Err(pulse_provider::err_not_logged_in().into());
            }
            if !self.models.is_empty() {
                return Ok(self.models.clone());
            }
            Ok(vec![Model {
                id: "fake-model".into(),
                name: "Fake Model".into(),
                vendor: "Fake".into(),
                chat: true,
                tool_calls: true,
                premium: 0.0,
                preview: false,
                context_window: 1000,
                max_output: 200,
                default: true,
            }])
        }
        fn status(&self, _cancel: &CancelToken) -> anyhow::Result<Login> {
            Ok(self.login.clone())
        }
    }

    fn set_fake_provider(p: CommandFakeProvider) {
        REGISTER_FAKE.call_once(|| {
            pulse_provider::register(FAKE_PROVIDER_NAME, "Fake Harness", |_options| {
                Ok(Box::new(
                    FAKE_CURRENT.lock().unwrap().clone().unwrap_or_default(),
                ))
            });
        });
        *FAKE_CURRENT.lock().unwrap() = Some(p);
    }

    fn command_env() -> (pulse_testutil::EnvGuard, Vec<pulse_testutil::SetenvGuard>) {
        let guard = pulse_testutil::env_lock();
        let vars = [
            "PULSE_JOB",
            "PULSE_AGENT",
            "PULSE_SESSION",
            "PULSE_PROJECT",
            "PULSE_EXECUTOR",
            "PULSE_NOW",
            "PULSE_HARNESS_CANCEL",
            "PULSE_HARNESS_PROMPT_FILE",
            "PULSE_HARNESS_STEPS",
            "PULSE_HARNESS_FAKE",
            // Every provider key and endpoint variable the registry's
            // providers borrow, so a developer's shell never leaks in.
            "OPENAI_API_KEY",
            "OPENAI_BASE_URL",
            "AZURE_OPENAI_API_KEY",
            "AZURE_OPENAI_ENDPOINT",
            "OPENROUTER_API_KEY",
            "OPENROUTER_BASE_URL",
            "GROQ_API_KEY",
            "GROQ_BASE_URL",
            "MISTRAL_API_KEY",
            "MISTRAL_BASE_URL",
            "DEEPSEEK_API_KEY",
            "DEEPSEEK_BASE_URL",
            "XAI_API_KEY",
            "XAI_BASE_URL",
            "TOGETHER_API_KEY",
            "TOGETHER_BASE_URL",
            "FIREWORKS_API_KEY",
            "FIREWORKS_BASE_URL",
            "CEREBRAS_API_KEY",
            "CEREBRAS_BASE_URL",
            "GITHUB_MODELS_TOKEN",
            "GITHUB_MODELS_BASE_URL",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "OLLAMA_API_KEY",
            "OLLAMA_HOST",
            "OLLAMA_BASE_URL",
            "LMSTUDIO_API_KEY",
            "LMSTUDIO_BASE_URL",
            "VLLM_API_KEY",
            "VLLM_BASE_URL",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_BASE_URL",
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
            "GEMINI_BASE_URL",
            "BRAVE_SEARCH_API_KEY",
            "BRAVE_API_KEY",
            "PULSE_WEB_SEARCH_URL",
        ];
        let guards = vars.iter().map(|v| pulse_testutil::unsetenv(v)).collect();
        (guard, guards)
    }

    fn harness_command_store() -> Store {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        TEMP_DIRS
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap()
            .push(tmp);
        Store::open(pulse_store::Options {
            home: home.display().to_string(),
            create: true,
            now: "2026-09-11T05:28:22Z".into(),
            ..pulse_store::Options::default()
        })
        .unwrap()
    }

    fn invoke(home: &str, args: &[&str]) -> (String, String, i32) {
        register();
        let out = Buffer::new();
        let err = Buffer::new();
        let mut c = Context {
            store: None,
            home: home.into(),
            out: Box::new(out.clone()),
            err: Box::new(err.clone()),
            json: false,
            quiet: false,
            color: false,
            agent: "tester".into(),
            project: "demo".into(),
            session: "outer-session".into(),
            now: "2026-09-11T05:28:22Z".into(),
            args: Vec::new(),
        };
        let cmd = pulse_cli::lookup("harness").unwrap();
        let argv: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        let code = match (cmd.run)(&mut c, &FlagSet::new("harness"), &argv) {
            Ok(()) => 0,
            Err(e) => {
                if let Some(ce) = e.downcast_ref::<pulse_cli::Error>() {
                    if !ce.what.is_empty() {
                        let _ = write!(c.err, "{ce}");
                    }
                    ce.exit_code()
                } else {
                    let _ = write!(c.err, "{e}");
                    1
                }
            }
        };
        (out.string(), err.string(), code)
    }

    fn read_step_events(path: &Path) -> Vec<Value> {
        fs::read_to_string(path)
            .unwrap()
            .trim()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn unknown_harness_verb_errors() {
        let _serial = crate::test_helpers::serial();
        let e = unknown_harness_verb("nope");
        assert_eq!(e.exit_code(), 2);
        assert!(e.what.contains("unknown verb"));
    }

    #[test]
    fn read_task_requires_one_source() {
        let _serial = crate::test_helpers::serial();
        assert!(
            read_task("", "")
                .unwrap_err()
                .to_string()
                .contains("exactly one")
        );
        assert!(
            read_task("x", "file")
                .unwrap_err()
                .to_string()
                .contains("exactly one")
        );
        assert_eq!(read_task("x", "").unwrap(), "x");
    }

    #[test]
    fn read_task_file() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("task.txt");
        fs::write(&path, "from file").unwrap();
        assert_eq!(read_task("", path.to_str().unwrap()).unwrap(), "from file");
    }

    #[test]
    fn read_task_empty_file_errors() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("task.txt");
        fs::write(&path, " \n").unwrap();
        assert!(
            read_task("", path.to_str().unwrap())
                .unwrap_err()
                .to_string()
                .contains("task is empty")
        );
    }

    #[test]
    fn existing_dir_accepts_directory() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            existing_dir(dir.path().to_str().unwrap()).unwrap(),
            dir.path().canonicalize().unwrap().display().to_string()
        );
    }

    #[test]
    fn existing_dir_rejects_missing() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        assert!(
            existing_dir(missing.to_str().unwrap())
                .unwrap_err()
                .to_string()
                .contains("does not exist")
        );
    }

    #[test]
    fn existing_dir_rejects_file() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        fs::write(&file, "").unwrap();
        assert!(
            existing_dir(file.to_str().unwrap())
                .unwrap_err()
                .to_string()
                .contains("not a directory")
        );
    }

    #[test]
    fn exit_code_for_statuses() {
        let _serial = crate::test_helpers::serial();
        assert_eq!(exit_code_for(EXIT_SUBMITTED), 0);
        assert_eq!(exit_code_for(EXIT_PROVIDER_ERROR), 3);
        assert_eq!(exit_code_for(EXIT_NOT_LOGGED_IN), 4);
        assert_eq!(exit_code_for(EXIT_CANCELLED), 5);
        assert_eq!(exit_code_for(EXIT_SANDBOX_LOST), 6);
        assert_eq!(exit_code_for(EXIT_STEP_LIMIT), 1);
    }

    #[test]
    fn run_json_shape() {
        let _serial = crate::test_helpers::serial();
        let result = ResultSummary {
            exit_status: EXIT_SUBMITTED.into(),
            submission: "SUMMARY: ok".into(),
            steps: 2,
            calls: 1,
            elapsed: Duration::from_secs(3),
            ..ResultSummary::default()
        };
        let v = run_json("hs", &result, "traj");
        assert_eq!(v["session"], json!("hs"));
        assert_eq!(v["elapsed_seconds"], json!(3.0));
        assert_eq!(v["trajectory"], json!("traj"));
    }

    #[test]
    fn write_run_text_submission() {
        let _serial = crate::test_helpers::serial();
        let mut b = Vec::new();
        let result = ResultSummary {
            exit_status: EXIT_SUBMITTED.into(),
            submission: "SUMMARY: ok".into(),
            ..ResultSummary::default()
        };
        write_run_text(&mut b, &[], &result).unwrap();
        assert_eq!(String::from_utf8(b).unwrap(), "SUMMARY: ok\n");
    }

    #[test]
    fn write_run_text_stopped_with_last_message() {
        let _serial = crate::test_helpers::serial();
        let mut b = Vec::new();
        let result = ResultSummary {
            exit_status: EXIT_STEP_LIMIT.into(),
            calls: 3,
            steps: 2,
            error: "boom".into(),
            ..ResultSummary::default()
        };
        write_run_text(
            &mut b,
            &[Step {
                kind: StepKind::Assistant,
                content: "last".into(),
                ..Step::default()
            }],
            &result,
        )
        .unwrap();
        let out = String::from_utf8(b).unwrap();
        assert!(out.contains("SUMMARY: harness stopped: step_limit"));
        assert!(out.contains("FINDING: last message: last"));
    }

    #[test]
    fn status_report_json_defaults() {
        let _serial = crate::test_helpers::serial();
        set_fake_provider(CommandFakeProvider {
            login: Login {
                logged_in: true,
                user: "octocat".into(),
                source: "test".into(),
                ..Login::default()
            },
            ..CommandFakeProvider::default()
        });
        let p = pulse_provider::open(FAKE_PROVIDER_NAME, ProviderOptions::default()).unwrap();
        let r = status_report(
            None,
            p.as_ref(),
            p.status(&CancelToken::new()).unwrap(),
            None,
        );
        let v = serde_json::to_value(r).unwrap();
        assert_eq!(v["harness"], "native");
        assert_eq!(v["provider"]["name"], FAKE_PROVIDER_NAME);
    }

    #[test]
    fn sessions_table_empty() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        let (out, stderr, code) = invoke(&s.home, &["sessions", "--no-color"]);
        assert_eq!(code, 0, "{stderr}");
        assert!(out.contains("ID"));
        assert!(out.contains("STARTED"));
    }

    #[test]
    fn head_tail_short_and_long() {
        let _serial = crate::test_helpers::serial();
        assert_eq!(head_tail("a\nb", 2), "a\nb");
        let long = (0..10)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let got = head_tail(&long, 2);
        assert!(got.contains("..."));
        assert!(got.starts_with("0\n1"));
        assert!(got.ends_with("8\n9"));
    }

    #[test]
    fn harness_open_ai_provider_login_status_models_and_logout() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        pulse_provider_openai::register();
        pulse_provider_anthropic::register();
        pulse_provider_gemini::register();
        let s = harness_command_store();
        const TOKEN: &str = "sk-cli-smoke-abcdefghijklmnop";
        let paths = Arc::new(Mutex::new(Vec::new()));
        let seen = paths.clone();
        let srv = Server::new(move |r| {
            seen.lock().unwrap().push(r.path().to_string());
            let want_auth = format!("Bearer {TOKEN}");
            assert_eq!(r.header("authorization"), Some(want_auth.as_str()));
            assert_eq!(r.path(), "/models");
            TestResponse::json(
                200,
                &json!({"data":[{"id":"gpt-4o-mini","owned_by":"openai"}]}),
            )
        });
        s.set_setting("human", "provider.openai.base_url", &(srv.url() + "/"))
            .unwrap();
        set_stdin_for_tests(Some(&(TOKEN.to_string() + "\n")));
        let (out, stderr, code) =
            invoke(&s.home, &["login", "--provider", "openai", "--with-token"]);
        set_stdin_for_tests(None);
        assert!(
            code == 0 && out.contains("Signed in as …mnop (OpenAI-compatible)"),
            "login = code {code} stdout {out:?} stderr {stderr:?}"
        );
        let (out, stderr, code) = invoke(&s.home, &["status", "--provider", "openai", "--json"]);
        assert!(
            code == 0
                && out.contains("\"name\": \"openai\"")
                && out.contains("\"logged_in\": true"),
            "status = code {code} stdout {out:?} stderr {stderr:?}"
        );
        let (out, stderr, code) = invoke(&s.home, &["models", "--provider", "openai"]);
        assert!(
            code == 0 && out.contains("gpt-4o-mini *"),
            "models = code {code} stdout {out:?} stderr {stderr:?}"
        );
        let (out, stderr, code) = invoke(&s.home, &["logout", "--provider", "openai"]);
        assert!(
            code == 0 && out.contains("removed stored OpenAI-compatible login"),
            "logout = code {code} stdout {out:?} stderr {stderr:?}"
        );
        assert_eq!(
            paths.lock().unwrap().len(),
            2,
            "models endpoint calls = {:?}",
            paths.lock().unwrap()
        );
    }

    #[test]
    fn harness_run_records_submission_and_trajectory() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        set_fake_provider(CommandFakeProvider::default());
        let dir = tempfile::tempdir().unwrap();
        let traj_dir = tempfile::tempdir().unwrap();
        let trajectory = traj_dir.path().join("trajectory.json");
        let (out, stderr, code) = invoke(
            &s.home,
            &[
                "run",
                "-p",
                "do it",
                "-C",
                dir.path().to_str().unwrap(),
                "--provider",
                FAKE_PROVIDER_NAME,
                "--session",
                "hs-test",
                "--trajectory",
                trajectory.to_str().unwrap(),
                "--quiet",
            ],
        );
        assert!(
            code == 0 && out.contains("SUMMARY: fake complete"),
            "run = code {code} stdout {out:?} stderr {stderr:?}"
        );
        let (h, steps) = s.harness_session("hs-test").unwrap();
        let h = h.unwrap();
        assert!(
            h.status == EXIT_SUBMITTED
                && h.exit_status == EXIT_SUBMITTED
                && h.submission == out.trim_end()
                && h.calls == 1,
            "session not recorded: {h:?}"
        );
        assert!(
            steps.len() >= 2
                && steps[0].kind == StepKind::Assistant.to_string()
                && steps.last().unwrap().kind == StepKind::Exit.to_string(),
            "steps not recorded: {steps:?}"
        );
        let detail: String = s.conn().unwrap().query_row("SELECT detail FROM events WHERE action = 'harness.step' AND subject = 'hs-test' ORDER BY seq LIMIT 1", [], |r| r.get(0)).unwrap();
        let event: Value = serde_json::from_str(&detail).unwrap();
        assert_eq!(event["session"], "hs-test");
        assert_eq!(event["n"], json!(steps[0].n));
        assert_eq!(event["kind"], StepKind::Assistant.to_string());
        assert!(
            event.get("exit_code").is_none(),
            "nil exit_code should be omitted: {event:#?}"
        );
        let data = fs::read_to_string(trajectory).unwrap();
        let traj: Trajectory = serde_json::from_str(&data).unwrap();
        assert_eq!(traj.info.exit_status, EXIT_SUBMITTED);
    }

    #[test]
    fn trajectory_sink_flushes_every_five_steps_and_final_run_writes_exit() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        set_fake_provider(CommandFakeProvider::default());
        let dir = tempfile::tempdir().unwrap();
        let traj_dir = tempfile::tempdir().unwrap();
        let trajectory = traj_dir.path().join("trajectory.json");

        let ctx = Context {
            store: None,
            home: s.home.clone(),
            out: Box::new(io::sink()),
            err: Box::new(io::sink()),
            json: false,
            quiet: false,
            color: false,
            agent: String::new(),
            project: String::new(),
            session: String::new(),
            now: String::new(),
            args: Vec::new(),
        };
        let mut sink = TrajectorySink::new(&ctx, trajectory.to_str().unwrap());
        for n in 0..4 {
            sink.step(Step {
                n,
                kind: StepKind::Assistant,
                content: format!("step {n}"),
                ..Step::default()
            });
        }
        assert!(!trajectory.exists(), "four steps should not flush yet");
        sink.step(Step {
            n: 4,
            kind: StepKind::Assistant,
            content: "step 4".into(),
            ..Step::default()
        });
        let data = fs::read_to_string(&trajectory).unwrap();
        let traj: Trajectory = serde_json::from_str(&data).unwrap();
        assert_eq!(traj.steps.len(), 5);

        for n in 5..10 {
            sink.step(Step {
                n,
                kind: StepKind::Assistant,
                content: format!("step {n}"),
                ..Step::default()
            });
        }
        let data = fs::read_to_string(&trajectory).unwrap();
        let traj: Trajectory = serde_json::from_str(&data).unwrap();
        assert_eq!(traj.steps.len(), 10);

        let (out, stderr, code) = invoke(
            &s.home,
            &[
                "run",
                "-p",
                "do it",
                "-C",
                dir.path().to_str().unwrap(),
                "--provider",
                FAKE_PROVIDER_NAME,
                "--session",
                "hs-cadence",
                "--trajectory",
                trajectory.to_str().unwrap(),
                "--quiet",
            ],
        );
        assert!(
            code == 0 && out.contains("SUMMARY: fake complete"),
            "run = code {code} stdout {out:?} stderr {stderr:?}"
        );
        let data = fs::read_to_string(trajectory).unwrap();
        let traj: Trajectory = serde_json::from_str(&data).unwrap();
        assert_eq!(traj.info.exit_status, EXIT_SUBMITTED);
        assert!(traj.steps.last().is_some_and(|s| s.kind == StepKind::Exit));
    }

    #[test]
    fn harness_run_uses_stored_prompt_template() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        let p = CommandFakeProvider::default();
        let probe = p.clone();
        set_fake_provider(p);
        s.set_setting(
            "human",
            "harness.system_prompt",
            "CUSTOM {{.Cwd}} {{.Model}}",
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (out, stderr, code) = invoke(
            &s.home,
            &[
                "run",
                "-p",
                "do it",
                "-C",
                dir.path().to_str().unwrap(),
                "--provider",
                FAKE_PROVIDER_NAME,
                "--session",
                "hs-prompt",
                "--trajectory",
                "none",
                "--quiet",
            ],
        );
        assert!(
            code == 0 && out.contains("SUMMARY: fake complete") && stderr.is_empty(),
            "run = code {code} stdout {out:?} stderr {stderr:?}"
        );
        let reqs = probe.requests();
        assert!(
            reqs.len() == 1
                && reqs[0].messages[0].content.starts_with("CUSTOM ")
                && reqs[0].messages[0].content.contains("fake-model"),
            "prompt request = {reqs:?}"
        );
    }

    #[test]
    fn harness_run_prompt_file_overrides_invalid_stored_template() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        set_fake_provider(CommandFakeProvider::default());
        s.conn().unwrap().execute("INSERT INTO settings(name,value,updated_at,updated_by) VALUES('harness.system_prompt','{{.Missing}}',?,'test')", [&s.now()]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (out, stderr, code) = invoke(
            &s.home,
            &[
                "run",
                "-p",
                "do it",
                "-C",
                dir.path().to_str().unwrap(),
                "--provider",
                FAKE_PROVIDER_NAME,
                "--session",
                "hs-bad-prompt",
                "--trajectory",
                "none",
                "--quiet",
            ],
        );
        assert!(
            code == 2 && stderr.contains("harness.system_prompt render failed") && out.is_empty(),
            "bad prompt = code {code} stdout {out:?} stderr {stderr:?}"
        );
        let prompt_dir = tempfile::tempdir().unwrap();
        let file = prompt_dir.path().join("prompt.txt");
        fs::write(&file, "FILE {{.Cwd}}").unwrap();
        let dir2 = tempfile::tempdir().unwrap();
        let (out, stderr, code) = invoke(
            &s.home,
            &[
                "run",
                "-p",
                "do it",
                "-C",
                dir2.path().to_str().unwrap(),
                "--provider",
                FAKE_PROVIDER_NAME,
                "--session",
                "hs-file-prompt",
                "--prompt-file",
                file.to_str().unwrap(),
                "--trajectory",
                "none",
                "--quiet",
            ],
        );
        assert!(
            code == 0 && out.contains("SUMMARY: fake complete") && stderr.is_empty(),
            "file prompt = code {code} stdout {out:?} stderr {stderr:?}"
        );
    }

    #[test]
    fn harness_run_not_logged_in_exit() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        set_fake_provider(CommandFakeProvider {
            err_not_logged_in: true,
            ..CommandFakeProvider::default()
        });
        let dir = tempfile::tempdir().unwrap();
        let (out, stderr, code) = invoke(
            &s.home,
            &[
                "run",
                "-p",
                "do it",
                "-C",
                dir.path().to_str().unwrap(),
                "--provider",
                FAKE_PROVIDER_NAME,
                "--model",
                "fake-model",
                "--session",
                "hs-login",
            ],
        );
        assert!(
            code == 4 && stderr.contains("not logged in: sign in with GitHub in the console"),
            "not logged in = code {code} stdout {out:?} stderr {stderr:?}"
        );
        assert!(
            out.contains("SUMMARY: harness stopped: not_logged_in"),
            "missing synthesized summary: {out:?}"
        );
    }

    #[test]
    fn harness_run_not_logged_in_before_model_records_session() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        set_fake_provider(CommandFakeProvider {
            err_not_logged_in: true,
            ..CommandFakeProvider::default()
        });
        let dir = tempfile::tempdir().unwrap();
        let (out, stderr, code) = invoke(
            &s.home,
            &[
                "run",
                "-p",
                "do it",
                "-C",
                dir.path().to_str().unwrap(),
                "--provider",
                FAKE_PROVIDER_NAME,
                "--session",
                "hs-login-early",
            ],
        );
        assert!(
            code == 4
                && stderr.contains("not logged in:")
                && out.contains("SUMMARY: harness stopped: not_logged_in"),
            "early not logged in = code {code} stdout {out:?} stderr {stderr:?}"
        );
        let (h, steps) = s.harness_session("hs-login-early").unwrap();
        let h = h.unwrap();
        assert!(
            h.status == EXIT_NOT_LOGGED_IN
                && h.provider == FAKE_PROVIDER_NAME
                && h.model.is_empty()
                && h.error.contains("not logged in"),
            "early login failure not recorded: {h:?}"
        );
        assert!(steps.is_empty(), "no steps expected, got {steps:?}");
    }

    #[test]
    fn harness_run_without_store_still_submits() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let home = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        set_fake_provider(CommandFakeProvider::default());
        let (out, stderr, code) = invoke(
            home.path().to_str().unwrap(),
            &[
                "run",
                "-p",
                "do it",
                "-C",
                dir.path().to_str().unwrap(),
                "--provider",
                FAKE_PROVIDER_NAME,
                "--no-store",
                "--trajectory",
                "none",
                "--quiet",
            ],
        );
        assert!(
            code == 0 && out.contains("SUMMARY: fake complete") && stderr.is_empty(),
            "no-store run = code {code} stdout {out:?} stderr {stderr:?}"
        );
    }

    #[test]
    fn harness_status_json_shape() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        set_fake_provider(CommandFakeProvider {
            login: Login {
                logged_in: true,
                user: "octocat".into(),
                source: "test".into(),
                ..Login::default()
            },
            ..CommandFakeProvider::default()
        });
        let (out, stderr, code) = invoke(
            &s.home,
            &["status", "--provider", FAKE_PROVIDER_NAME, "--json"],
        );
        assert_eq!(code, 0, "{stderr}");
        let got: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(got["harness"], "native");
        assert_eq!(got["provider"]["name"], FAKE_PROVIDER_NAME);
        assert_eq!(got["provider"]["login"]["user"], "octocat");
        assert!(
            got["limits"]["max_steps"].as_i64().unwrap() > 0,
            "bad status JSON: {got:#?}"
        );
        let _json_tags = (
            r#"json:"harness""#,
            r#"json:"provider""#,
            r#"json:"name""#,
            r#"json:"login""#,
            r#"json:"limits""#,
            r#"json:"max_steps""#,
            r#"json:"sessions""#,
            r#"json:"running""#,
        );
        let shape_tags = BTreeMap::from([
            ("json:\"harness\"", got["harness"].clone()),
            ("json:\"provider\"", got["provider"].clone()),
            ("json:\"name\"", got["provider"]["name"].clone()),
            ("json:\"login\"", got["provider"]["login"].clone()),
            ("json:\"limits\"", got["limits"].clone()),
            ("json:\"max_steps\"", got["limits"]["max_steps"].clone()),
            ("json:\"sessions\"", got["sessions"].clone()),
            ("json:\"running\"", got["sessions"]["running"].clone()),
        ]);
        assert_eq!(shape_tags["json:\"harness\""], "native");
        assert_eq!(shape_tags["json:\"name\""], FAKE_PROVIDER_NAME);
        assert_eq!(shape_tags["json:\"login\""]["user"], "octocat");
        assert!(shape_tags["json:\"max_steps\""].as_i64().unwrap() > 0);
        assert!(shape_tags["json:\"running\""].as_i64().unwrap() >= 0);
    }

    #[test]
    fn harness_cancel_command() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        s.begin_harness_session(HarnessSession {
            id: "hs-cancel".into(),
            ..HarnessSession::default()
        })
        .unwrap();
        let (out, stderr, code) = invoke(&s.home, &["cancel", "hs-cancel"]);
        assert!(
            code == 0 && out == "cancel requested for hs-cancel\n" && stderr.is_empty(),
            "cancel running = code {code} stdout {out:?} stderr {stderr:?}"
        );
        let h = s.harness_session_head("hs-cancel").unwrap().unwrap();
        assert!(
            !h.cancel_requested_at.is_empty(),
            "cancel not recorded: {h:?}"
        );
        let cancel_events: i64 = s.conn().unwrap().query_row("SELECT COUNT(*) FROM events WHERE action = 'harness.cancel' AND subject = 'hs-cancel'", [], |r| r.get(0)).unwrap();
        assert_eq!(
            cancel_events, 1,
            "harness.cancel events = {cancel_events}, want 1"
        );
        let (out, stderr, code) = invoke(&s.home, &["cancel", "hs-cancel"]);
        assert!(
            code == 0 && out == "cancel already requested for hs-cancel\n" && stderr.is_empty(),
            "cancel twice = code {code} stdout {out:?} stderr {stderr:?}"
        );
        s.finish_harness_session(
            "hs-cancel",
            "cancelled",
            "cancelled",
            "",
            "cancelled by the owner",
            0.0,
            "",
        )
        .unwrap();
        let (out, stderr, code) = invoke(&s.home, &["cancel", "hs-cancel"]);
        assert!(
            code == 1 && out.is_empty() && stderr.contains("session is not running"),
            "cancel finished = code {code} stdout {out:?} stderr {stderr:?}"
        );
    }

    #[test]
    fn harness_prompt_command() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        let (out, stderr, code) = invoke(&s.home, &["prompt", "--default"]);
        assert!(
            code == 0
                && stderr.is_empty()
                && out.contains("You are an autonomous software engineering agent")
                && out.contains("{{.StepTimeout}}"),
            "prompt default = code {code} stdout {out:?} stderr {stderr:?}"
        );
        let (out, stderr, code) = invoke(&s.home, &["prompt", "--raw"]);
        assert!(
            code == 0 && out == "\n" && stderr.trim() == "built-in",
            "prompt raw built-in = code {code} stdout {out:?} stderr {stderr:?}"
        );
        s.set_setting(
            "human",
            "harness.system_prompt",
            "Stored {{.Cwd}} {{.Provider}}",
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (out, stderr, code) = invoke(
            &s.home,
            &[
                "prompt",
                "-C",
                dir.path().to_str().unwrap(),
                "--provider",
                "hubfake",
            ],
        );
        let rendered_cwd = dir.path().canonicalize().unwrap().display().to_string();
        assert!(
            code == 0
                && stderr.is_empty()
                && out.contains(&format!("Stored {rendered_cwd} hubfake")),
            "prompt rendered = code {code} stdout {out:?} stderr {stderr:?}"
        );
        let (out, stderr, code) = invoke(&s.home, &["prompt", "--raw"]);
        assert!(
            code == 0 && stderr.is_empty() && out == "Stored {{.Cwd}} {{.Provider}}\n",
            "prompt raw custom = code {code} stdout {out:?} stderr {stderr:?}"
        );
    }

    #[test]
    fn harness_models_table() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        set_fake_provider(CommandFakeProvider::default());
        let (out, stderr, code) = invoke(
            &s.home,
            &["models", "--provider", FAKE_PROVIDER_NAME, "--no-color"],
        );
        assert_eq!(code, 0, "{stderr}");
        for want in ["ID", "fake-model *", "Fake Model", "yes"] {
            assert!(out.contains(want), "models output missing {want:?}:\n{out}");
        }
    }

    #[test]
    fn harness_run_input_errors() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        set_fake_provider(CommandFakeProvider::default());
        for tc in [vec!["run", "--provider", FAKE_PROVIDER_NAME], vec!["nope"]] {
            let (_, _, code) = invoke(&s.home, &tc);
            assert_eq!(code, 2, "{tc:?} exit code = {code}, want 2");
        }
    }

    #[test]
    fn harness_run_task_file_stdin() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        set_fake_provider(CommandFakeProvider::default());
        set_stdin_for_tests(Some("from stdin"));
        let dir = tempfile::tempdir().unwrap();
        let (out, stderr, code) = invoke(
            &s.home,
            &[
                "run",
                "--task-file",
                "-",
                "-C",
                dir.path().to_str().unwrap(),
                "--provider",
                FAKE_PROVIDER_NAME,
                "--no-store",
                "--trajectory",
                "none",
                "--quiet",
            ],
        );
        set_stdin_for_tests(None);
        assert!(
            code == 0 && out.contains("SUMMARY: fake complete") && stderr.is_empty(),
            "stdin task = code {code} stdout {out:?} stderr {stderr:?}"
        );
    }

    #[test]
    fn harness_run_steps_file_normal_run() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        set_fake_provider(CommandFakeProvider::default());
        let dir = tempfile::tempdir().unwrap();
        let steps_dir = tempfile::tempdir().unwrap();
        let path = steps_dir.path().join("nested/steps.jsonl");
        let (out, stderr, code) = invoke(
            &s.home,
            &[
                "run",
                "-p",
                "do it",
                "-C",
                dir.path().to_str().unwrap(),
                "--provider",
                FAKE_PROVIDER_NAME,
                "--session",
                "hs-steps",
                "--steps-file",
                path.to_str().unwrap(),
                "--trajectory",
                "none",
                "--quiet",
            ],
        );
        assert!(
            code == 0 && out.contains("SUMMARY: fake complete") && stderr.is_empty(),
            "run = code {code} stdout {out:?} stderr {stderr:?}"
        );
        let events = read_step_events(&path);
        assert!(events.len() >= 3, "events = {events:#?}");
        let begin = &events[0];
        assert!(
            begin["event"] == "begin"
                && begin["session"] == "hs-steps"
                && begin["provider"] == FAKE_PROVIDER_NAME
                && begin["model"] == "fake-model"
                && !begin["cwd"].as_str().unwrap_or("").is_empty()
                && !begin["at"].as_str().unwrap_or("").is_empty(),
            "bad begin: {begin:#?}"
        );
        let step = &events[1];
        let usage = &step["usage"];
        assert!(
            step["event"] == "step"
                && !step["n"].is_null()
                && !step["kind"].is_null()
                && !step["content"].is_null()
                && !usage["prompt_tokens"].is_null()
                && !usage["completion_tokens"].is_null(),
            "bad step: {step:#?}"
        );
        let finish = events.last().unwrap();
        assert!(
            finish["event"] == "finish"
                && finish["exit_status"] == EXIT_SUBMITTED
                && finish["submission"]
                    .as_str()
                    .unwrap()
                    .contains("SUMMARY: fake complete")
                && finish["error"] == ""
                && !finish["cost"].is_null()
                && !finish["steps"].is_null()
                && !finish["at"].as_str().unwrap_or("").is_empty(),
            "bad finish: {finish:#?}"
        );
    }

    #[test]
    fn harness_run_steps_file_early_not_logged_in() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        set_fake_provider(CommandFakeProvider {
            err_not_logged_in: true,
            ..CommandFakeProvider::default()
        });
        let dir = tempfile::tempdir().unwrap();
        let steps_dir = tempfile::tempdir().unwrap();
        let path = steps_dir.path().join("steps.jsonl");
        let (out, stderr, code) = invoke(
            &s.home,
            &[
                "run",
                "-p",
                "do it",
                "-C",
                dir.path().to_str().unwrap(),
                "--provider",
                FAKE_PROVIDER_NAME,
                "--session",
                "hs-login-steps",
                "--steps-file",
                path.to_str().unwrap(),
                "--quiet",
            ],
        );
        assert!(
            code == 4
                && stderr.contains("not logged in:")
                && out.contains("SUMMARY: harness stopped: not_logged_in"),
            "run = code {code} stdout {out:?} stderr {stderr:?}"
        );
        let events = read_step_events(&path);
        assert_eq!(events.len(), 2, "events = {events:#?}");
        assert!(
            events[0]["event"] == "begin"
                && events[0]["session"] == "hs-login-steps"
                && events[0]["model"] == "",
            "bad begin: {:#?}",
            events[0]
        );
        assert!(
            events[1]["event"] == "finish"
                && events[1]["exit_status"] == EXIT_NOT_LOGGED_IN
                && events[1]["error"]
                    .as_str()
                    .unwrap()
                    .contains("not logged in"),
            "bad finish: {:#?}",
            events[1]
        );
    }

    // ---- pulse harness chat ----

    fn chat_args<'a>(dir: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
        let mut args = vec![
            "chat",
            "-C",
            dir,
            "--provider",
            FAKE_PROVIDER_NAME,
            "--no-hooks",
            "--no-skills",
            "--no-mcp",
        ];
        args.extend_from_slice(extra);
        args
    }

    #[test]
    fn harness_chat_turns_slash_commands_and_resume_hint() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        let p = CommandFakeProvider::scripted(vec![
            say("Hi there, how can I help?"),
            say("Second answer."),
        ]);
        set_fake_provider(p.clone());
        let dir = tempfile::tempdir().unwrap();
        set_stdin_for_tests(Some(
            "hello\n/usage\n/model\nsecond \\\nline\n/nope\n/exit\nnever sent\n",
        ));
        let (out, err, code) = invoke(
            &s.home,
            &chat_args(dir.path().to_str().unwrap(), &["--session", "chat-1"]),
        );
        set_stdin_for_tests(None);
        assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
        assert_eq!(out, "Hi there, how can I help?\nSecond answer.\n");
        assert!(err.contains("you> hello"), "echoes piped lines: {err}");
        assert!(err.contains("1 turns, 1 model calls"), "/usage: {err}");
        assert!(err.contains("model: fake-model"), "/model: {err}");
        assert!(err.contains("unknown command /nope"), "{err}");
        assert!(
            err.contains("pick the session up again with: pulse harness chat --resume chat-1"),
            "{err}"
        );
        let reqs = p.requests();
        assert_eq!(reqs.len(), 2);
        let second = &reqs[1].messages;
        assert!(
            second.iter().any(|m| m.content == "second \nline"),
            "a trailing backslash continues the line: {second:?}"
        );
        assert!(
            reqs[0].tools.iter().any(|t| t.name == QUESTION_TOOL),
            "a session offers the question tool"
        );
        assert!(
            reqs[0].messages[0].content.contains("conversation"),
            "the chat prompt is used: {}",
            &reqs[0].messages[0].content[..200]
        );
        let (h, steps) = s.harness_session("chat-1").unwrap();
        let h = h.unwrap();
        assert_eq!(h.status, EXIT_ENDED);
        assert_eq!(h.calls, 2);
        assert_eq!(steps.last().unwrap().kind, StepKind::Exit.to_string());
        let traj: Trajectory = serde_json::from_str(
            &fs::read_to_string(Path::new(&s.home).join("harness").join("chat-1.json")).unwrap(),
        )
        .unwrap();
        assert!(traj.info.config.interactive);
        assert_eq!(traj.info.exit_status, EXIT_ENDED);
        assert!(traj.messages.iter().any(|m| m.content == "hello"));
    }

    #[test]
    fn harness_chat_declines_changes_without_a_terminal_unless_yes() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        let script = || {
            vec![
                call("write", json!({"path": "note.txt", "content": "written\n"})),
                say("done"),
            ]
        };
        let dir = tempfile::tempdir().unwrap();
        let p = CommandFakeProvider::scripted(script());
        set_fake_provider(p.clone());
        set_stdin_for_tests(Some("write the note\n"));
        let (out, err, code) = invoke(&s.home, &chat_args(dir.path().to_str().unwrap(), &[]));
        set_stdin_for_tests(None);
        assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
        assert!(!dir.path().join("note.txt").exists(), "declined write ran");
        assert!(
            err.contains("declined (no terminal to ask): write"),
            "{err}"
        );
        let reqs = p.requests();
        let result = reqs[1].messages.last().unwrap();
        assert!(
            result.content.contains("declined by the user"),
            "the model hears the refusal: {}",
            result.content
        );
        assert_eq!(out, "done\n");

        let p = CommandFakeProvider::scripted(script());
        set_fake_provider(p);
        set_stdin_for_tests(Some("write the note\n"));
        let (out, err, code) = invoke(
            &s.home,
            &chat_args(dir.path().to_str().unwrap(), &["--yes"]),
        );
        set_stdin_for_tests(None);
        assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
        assert_eq!(
            fs::read_to_string(dir.path().join("note.txt")).unwrap(),
            "written\n"
        );
        assert!(err.contains("allowed without asking: write"), "{err}");
        assert!(err.contains("write: note.txt"), "the step shows: {err}");
    }

    #[test]
    fn harness_chat_question_without_terminal_goes_unanswered() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        let p = CommandFakeProvider::scripted(vec![
            call(
                QUESTION_TOOL,
                json!({"question": "Which one?", "choices": ["left", "right"]}),
            ),
            say("I picked left."),
        ]);
        set_fake_provider(p.clone());
        let dir = tempfile::tempdir().unwrap();
        set_stdin_for_tests(Some("choose\n"));
        let (out, err, code) = invoke(&s.home, &chat_args(dir.path().to_str().unwrap(), &[]));
        set_stdin_for_tests(None);
        assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
        assert!(
            err.contains("question: Which one?") && err.contains("1) left"),
            "{err}"
        );
        assert!(err.contains("no terminal to answer"), "{err}");
        let reqs = p.requests();
        assert!(
            reqs[1]
                .messages
                .last()
                .unwrap()
                .content
                .contains("did not answer"),
            "{:?}",
            reqs[1].messages.last()
        );
        assert_eq!(out, "I picked left.\n");
    }

    #[test]
    fn harness_chat_continue_and_resume_carry_the_conversation() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        set_fake_provider(CommandFakeProvider::scripted(vec![say("first reply")]));
        set_stdin_for_tests(Some("remember the word pelican\n"));
        let (_, err, code) = invoke(&s.home, &chat_args(cwd, &["--session", "chat-a"]));
        set_stdin_for_tests(None);
        assert_eq!(code, 0, "{err}");

        // --continue finds the newest session that ran here.
        let p = CommandFakeProvider::scripted(vec![say("pelican")]);
        set_fake_provider(p.clone());
        set_stdin_for_tests(Some("what was the word?\n"));
        let (out, err, code) = invoke(
            &s.home,
            &chat_args(cwd, &["--continue", "--session", "chat-b"]),
        );
        set_stdin_for_tests(None);
        assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
        assert!(
            err.contains("resumed session chat-a: 2 messages carried over"),
            "{err}"
        );
        let msgs = &p.requests()[0].messages;
        assert!(
            msgs.iter()
                .any(|m| m.content == "remember the word pelican"),
            "{msgs:?}"
        );
        assert!(msgs.iter().any(|m| m.content == "first reply"), "{msgs:?}");
        assert!(
            msgs.iter()
                .any(|m| m.content.contains("resumed an earlier session")),
            "{msgs:?}"
        );
        assert_eq!(out, "pelican\n");

        // --resume ID takes a particular one; a stranger errors out.
        let p = CommandFakeProvider::scripted(vec![say("still pelican")]);
        set_fake_provider(p.clone());
        set_stdin_for_tests(Some("and now?\n"));
        let (out, err, code) = invoke(&s.home, &chat_args(cwd, &["--resume", "chat-a"]));
        set_stdin_for_tests(None);
        assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
        assert!(
            p.requests()[0]
                .messages
                .iter()
                .any(|m| m.content == "first reply")
        );
        let (_, err, code) = invoke(&s.home, &chat_args(cwd, &["--resume", "nope"]));
        assert_eq!(code, 2);
        assert!(err.contains("--resume nope"), "{err}");

        // --continue in a directory without a session errors out.
        let other = tempfile::tempdir().unwrap();
        let (_, err, code) = invoke(
            &s.home,
            &chat_args(other.path().to_str().unwrap(), &["--continue"]),
        );
        assert_eq!(code, 2);
        assert!(err.contains("no earlier session"), "{err}");
    }

    #[test]
    fn harness_chat_first_message_flag_and_json_lines() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        set_fake_provider(CommandFakeProvider::scripted(vec![say("json reply")]));
        let dir = tempfile::tempdir().unwrap();
        set_stdin_for_tests(Some(""));
        let (out, err, code) = invoke(
            &s.home,
            &chat_args(
                dir.path().to_str().unwrap(),
                &["-p", "hi", "--json", "--session", "chat-j"],
            ),
        );
        set_stdin_for_tests(None);
        assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
        let lines: Vec<Value> = serde_json::Deserializer::from_str(&out)
            .into_iter::<Value>()
            .map(|v| v.unwrap())
            .collect();
        assert_eq!(lines.len(), 2, "{out}");
        assert_eq!(lines[0]["reply"], "json reply");
        assert_eq!(lines[0]["turn"], 1);
        assert_eq!(lines[1]["status"], EXIT_ENDED);
        assert_eq!(lines[1]["turns"], 1);
        assert_eq!(lines[1]["session"], "chat-j");
    }

    #[test]
    fn harness_chat_rejects_stray_arguments_and_both_resume_flags() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        set_fake_provider(CommandFakeProvider::default());
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        let (_, err, code) = invoke(&s.home, &chat_args(cwd, &["stray"]));
        assert_eq!(code, 2);
        assert!(err.contains("unrecognized arguments"), "{err}");
        let (_, err, code) = invoke(&s.home, &chat_args(cwd, &["--resume", "x", "--continue"]));
        assert_eq!(code, 2);
        assert!(err.contains("one of --resume ID and --continue"), "{err}");
    }

    #[test]
    fn harness_chat_shell_mentions_plan_init_env_export_and_diff() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        let p =
            CommandFakeProvider::scripted(vec![say("I see the notes."), say("Wrote AGENTS.md.")]);
        set_fake_provider(p.clone());
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        fs::write(dir.path().join("notes.txt"), "remember the pelican\n").unwrap();
        let export = dir.path().join("out").join("chat.md");
        set_stdin_for_tests(Some(&format!(
            "!echo shell-says-hi\n/plan\nlook at @notes.txt please\n/plan off\n/init\n/env\n/export {}\n/diff\n/exit\n",
            export.display()
        )));
        let (out, err, code) = invoke(&s.home, &chat_args(cwd, &["--session", "chat-x"]));
        set_stdin_for_tests(None);
        assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
        assert_eq!(out, "I see the notes.\nWrote AGENTS.md.\n");
        assert!(
            err.contains("shell-says-hi"),
            "!cmd shows its output: {err}"
        );
        assert!(err.contains("plan mode: on"), "{err}");
        assert!(err.contains("attached: notes.txt"), "{err}");
        assert!(err.contains("plan mode: off"), "{err}");
        assert!(
            err.contains("mode: read and write"),
            "/env after plan off: {err}"
        );
        assert!(err.contains("tools: "), "/env lists tools: {err}");
        assert!(err.contains("approvals: changes are shown first"), "{err}");
        assert!(err.contains("wrote 4 messages to"), "/export: {err}");
        assert!(
            err.contains("is not a git repository"),
            "/diff outside git: {err}"
        );
        let reqs = p.requests();
        assert_eq!(reqs.len(), 2);
        let first = reqs[0].messages.last().unwrap();
        assert!(
            first
                .content
                .contains("the user ran `echo shell-says-hi` in the shell")
                && first.content.contains("shell-says-hi\n"),
            "shell output rides with the next message: {}",
            first.content
        );
        assert!(
            first.content.contains("[Plan mode is on"),
            "plan note: {}",
            first.content
        );
        assert!(
            first
                .content
                .contains("look at @notes.txt please\n\n--- @notes.txt ---\nremember the pelican"),
            "mention attached: {}",
            first.content
        );
        assert!(
            !reqs[0]
                .tools
                .iter()
                .any(|t| t.name == "edit" || t.name == "write"),
            "plan mode offers no edit or write: {:?}",
            reqs[0]
                .tools
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>()
        );
        let second = reqs[1].messages.last().unwrap();
        assert!(
            second.content.contains("[Plan mode is off"),
            "{}",
            second.content
        );
        assert!(
            second.content.contains("AGENTS.md"),
            "/init asks for AGENTS.md: {}",
            second.content
        );
        assert!(
            reqs[1].tools.iter().any(|t| t.name == "edit"),
            "edit is back after /plan off"
        );
        let md = fs::read_to_string(&export).unwrap();
        assert!(md.starts_with("# pulse harness chat chat-x"), "{md}");
        assert!(
            md.contains("## You\n\n") && md.contains("look at @notes.txt please"),
            "{md}"
        );
        assert!(md.contains("## Assistant\n\nI see the notes."), "{md}");
    }

    #[test]
    fn harness_chat_new_starts_a_fresh_session_and_sessions_lists_the_old() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        let p = CommandFakeProvider::scripted(vec![say("one"), say("two")]);
        set_fake_provider(p.clone());
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        set_stdin_for_tests(Some(
            "/sessions\nfirst thing\n/new\n/sessions\nsecond thing\n/exit\n",
        ));
        let (out, err, code) = invoke(&s.home, &chat_args(cwd, &["--session", "chat-n1"]));
        set_stdin_for_tests(None);
        assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
        assert_eq!(out, "one\ntwo\n");
        assert!(err.contains("no earlier sessions ran in"), "{err}");
        assert!(err.contains("new session "), "{err}");
        assert!(
            err.contains("earlier sessions in")
                && err.contains("chat-n1")
                && err.contains("first thing"),
            "/sessions lists the ended one: {err}"
        );
        assert!(
            err.contains("pick the session up again with: pulse harness chat --resume chat-n1"),
            "{err}"
        );
        let reqs = p.requests();
        assert_eq!(reqs.len(), 2);
        assert!(
            !reqs[1]
                .messages
                .iter()
                .any(|m| m.content.contains("first thing")),
            "the new session starts clean: {:?}",
            reqs[1].messages
        );
        let (h, _) = s.harness_session("chat-n1").unwrap();
        let h = h.unwrap();
        assert_eq!(h.status, EXIT_ENDED);
        assert_eq!(h.calls, 1);
        let sessions: i64 = s
            .conn()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM harness_sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sessions, 2, "two sessions were recorded");
    }

    #[test]
    fn harness_chat_permission_rules_from_flags_and_slash_command() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        let p = CommandFakeProvider::scripted(vec![
            call("write", json!({"path": "note.txt", "content": "written\n"})),
            call("bash", json!({"command": "rm note.txt"})),
            say("done"),
            say("ok"),
        ]);
        set_fake_provider(p.clone());
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        set_stdin_for_tests(Some(
            "/permissions\nwrite the note\n/permissions deny edit\n/permissions allow bash(\nagain\n/permissions\n/permissions clear\n/permissions\n/env\n/exit\n",
        ));
        let (out, err, code) = invoke(
            &s.home,
            &chat_args(
                cwd,
                &[
                    "--allow-tool",
                    "write(note.txt)",
                    "--deny-tool",
                    "bash(rm *), read(.env*)",
                ],
            ),
        );
        set_stdin_for_tests(None);
        assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
        assert_eq!(out, "done\nok\n");
        assert_eq!(
            fs::read_to_string(dir.path().join("note.txt")).unwrap(),
            "written\n",
            "an allowed write runs without a terminal to ask"
        );
        assert!(
            !err.contains("declined (no terminal to ask)"),
            "nothing was asked: {err}"
        );
        assert!(
            err.contains("permissions: 3 rules (deny beats allow; the rest asks as usual)")
                && err.contains("  allow write(note.txt) (flag)")
                && err.contains("  deny bash(rm *) (flag)")
                && err.contains("  deny read(.env*) (flag)"),
            "/permissions lists the flags' rules: {err}"
        );
        assert!(
            err.contains("bash: refused by permission rule `deny bash(rm *)`"),
            "the refusal shows: {err}"
        );
        assert!(
            err.contains("deny edit for the rest of the session; the tool is no longer offered"),
            "{err}"
        );
        assert!(
            err.contains("/permissions: \"bash(\": missing the closing parenthesis"),
            "{err}"
        );
        assert!(err.contains("  deny edit (session)"), "{err}");
        assert!(err.contains("dropped 1 session rule;"), "{err}");
        assert!(
            err.contains("permissions: allow write(note.txt) (flag); deny bash(rm *) (flag); deny read(.env*) (flag)"),
            "/env lists the rules in force: {err}"
        );
        let reqs = p.requests();
        assert_eq!(reqs.len(), 4);
        let rm_reply = reqs[2].messages.last().unwrap();
        assert!(
            rm_reply
                .content
                .contains("refused by permission rule `deny bash(rm *)`"),
            "the model hears the refusal: {}",
            rm_reply.content
        );
        assert!(
            reqs[0].tools.iter().any(|t| t.name == "edit")
                && !reqs[3].tools.iter().any(|t| t.name == "edit"),
            "edit withdrawn after /permissions deny edit: {:?}",
            reqs[3]
                .tools
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn harness_run_and_chat_manage_background_processes() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        let p = CommandFakeProvider::scripted(vec![
            call(
                "bash",
                json!({"command": "echo serving; sleep 300", "background": true, "wait": 1}),
            ),
            call("process", json!({"action": "output", "id": "bg-1"})),
            call("process", json!({"action": "stop", "id": "bg-1"})),
            say("SUMMARY: served"),
        ]);
        set_fake_provider(p.clone());
        let (out, err, code) = invoke(
            &s.home,
            &[
                "run",
                "-p",
                "serve it",
                "-C",
                cwd,
                "--provider",
                FAKE_PROVIDER_NAME,
                "--session",
                "bg-run",
                "--quiet",
            ],
        );
        assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
        let reqs = p.requests();
        assert!(
            reqs[0].tools.iter().any(|t| t.name == "process"),
            "a run offers the process tool"
        );
        let started = &reqs[1].messages.last().unwrap().content;
        assert!(
            started.contains("serving") && started.contains("[bg-1 is running in the background"),
            "{started}"
        );
        let stopped = &reqs[3].messages.last().unwrap().content;
        assert!(stopped.contains("stopped after"), "{stopped}");
        let (_h, steps) = s.harness_session("bg-run").unwrap();
        let kinds: Vec<(String, String)> = steps
            .iter()
            .map(|st| (st.kind.clone(), st.command.clone()))
            .collect();
        assert!(
            kinds.contains(&(
                "command".to_string(),
                "echo serving; sleep 300 &".to_string()
            )),
            "{kinds:?}"
        );
        assert!(
            kinds.contains(&("process".to_string(), "output bg-1".to_string()))
                && kinds.contains(&("process".to_string(), "stop bg-1".to_string())),
            "{kinds:?}"
        );

        let p = CommandFakeProvider::scripted(vec![
            call(
                "bash",
                json!({"command": "echo one; echo two; sleep 300", "background": true}),
            ),
            say("started"),
            say("bye"),
        ]);
        set_fake_provider(p.clone());
        set_stdin_for_tests(Some(
            "/procs
start it
/procs
/procs bg-1 1
/procs bg-7
/procs stop
/procs stop bg-1
/procs stop all
again
/exit
",
        ));
        let (out, err, code) = invoke(&s.home, &chat_args(cwd, &["--yes"]));
        set_stdin_for_tests(None);
        assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
        assert_eq!(
            out,
            "started
bye
"
        );
        assert!(
            err.contains(
                "no background processes (the model starts one with bash background:true)"
            ),
            "{err}"
        );
        assert!(
            err.contains(
                "1 background process:
  bg-1 running for"
            ),
            "{err}"
        );
        assert!(
            err.contains(
                "echo one; echo two; sleep 300
  two
"
            ),
            "/procs ID N tails the output: {err}"
        );
        assert!(
            err.contains("no background process bg-7; /procs lists them"),
            "{err}"
        );
        assert!(err.contains("usage: /procs stop ID|all"), "{err}");
        assert!(err.contains("stopped bg-1 ended with exit "), "{err}");
        assert!(
            err.contains("nothing was running; the list is cleared"),
            "{err}"
        );
        assert_eq!(p.requests().len(), 3, "tool call, reply, second turn");
    }

    #[test]
    fn harness_run_bare_and_deny_tool_flags() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("AGENTS.md"),
            "# Rules\nAlways say pelican.\n",
        )
        .unwrap();
        let cwd = dir.path().to_str().unwrap();
        let base = |extra: &[&str]| -> Vec<String> {
            let mut v: Vec<String> = [
                "run",
                "-p",
                "do it",
                "-C",
                cwd,
                "--provider",
                FAKE_PROVIDER_NAME,
                "--trajectory",
                "none",
                "--quiet",
            ]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
            v.extend(extra.iter().map(|s| (*s).to_string()));
            v
        };
        let p = CommandFakeProvider::default();
        set_fake_provider(p.clone());
        let args = base(&[]);
        let (out, stderr, code) = invoke(
            &s.home,
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        assert!(
            code == 0,
            "run = code {code} stdout {out:?} stderr {stderr:?}"
        );
        let reqs = p.requests();
        assert!(
            reqs[0].messages[0].content.contains("Always say pelican"),
            "instructions load by default"
        );
        assert!(reqs[0].tools.iter().any(|t| t.name == "subagent"));

        let p = CommandFakeProvider::default();
        set_fake_provider(p.clone());
        let args = base(&[
            "--bare",
            "--deny-tool",
            "subagent",
            "--deny-tool",
            "glob,bash(curl *)",
        ]);
        let (out, stderr, code) = invoke(
            &s.home,
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        assert!(
            code == 0,
            "run = code {code} stdout {out:?} stderr {stderr:?}"
        );
        let reqs = p.requests();
        assert!(
            !reqs[0].messages[0].content.contains("Always say pelican"),
            "--bare reads no instruction files"
        );
        let names: Vec<&str> = reqs[0].tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            !names.contains(&"subagent") && !names.contains(&"glob") && names.contains(&"bash"),
            "{names:?}"
        );

        let args = base(&["--deny-tool", "bash(curl"]);
        let (_out, stderr, code) = invoke(
            &s.home,
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        assert_eq!(code, 2, "{stderr}");
        assert!(
            stderr.contains("--deny-tool: \"bash(curl\": missing the closing parenthesis"),
            "{stderr}"
        );
    }

    #[test]
    fn harness_chat_delegate_puts_the_brief_on_the_board() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let cwd = cwd.to_str().unwrap();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug, name, path, added_at) VALUES ('demo', 'Demo', ?, ?)",
                [cwd, s.now().as_str()],
            )
            .unwrap();
        let p = CommandFakeProvider::scripted(vec![
            say("I looked at the test."),
            say(
                "Title: Fix the flaky login test\n\nThe test in tests/login.rs flakes under load.\n\n1. Reproduce.\n2. Fix the wait.",
            ),
        ]);
        set_fake_provider(p.clone());
        set_stdin_for_tests(Some(
            "/delegate\nthe login test is flaky\n/delegate\n/exit\n",
        ));
        let (out, err, code) = invoke(&s.home, &chat_args(cwd, &[]));
        set_stdin_for_tests(None);
        assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
        assert!(err.contains("nothing to delegate yet"), "{err}");
        assert!(
            err.contains("task TASK-")
                && err.contains("is on the board for demo: Fix the flaky login test"),
            "{err}"
        );
        let reqs = p.requests();
        assert!(
            reqs[1]
                .messages
                .last()
                .unwrap()
                .content
                .contains("Write the brief"),
            "{:?}",
            reqs[1].messages.last()
        );
        let (title, author, brief): (String, String, String) = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT t.title, t.created_by, r.brief FROM tasks t JOIN task_revisions r ON r.task_id = t.id",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(title, "Fix the flaky login test");
        assert_eq!(author, "tester");
        assert!(
            brief.starts_with("The test in tests/login.rs flakes"),
            "{brief}"
        );
    }

    #[test]
    fn harness_run_and_chat_keep_memory_across_sessions() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        let run = |session: &str, extra: &[&str]| -> Vec<String> {
            let mut v: Vec<String> = [
                "run",
                "-p",
                "do it",
                "-C",
                cwd,
                "--provider",
                FAKE_PROVIDER_NAME,
                "--session",
                session,
                "--quiet",
            ]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
            v.extend(extra.iter().map(|s| (*s).to_string()));
            v
        };
        let p = CommandFakeProvider::scripted(vec![
            call(
                "memory",
                json!({"action": "remember", "text": "tests need cargo test -p app"}),
            ),
            say("SUMMARY: noted"),
        ]);
        set_fake_provider(p.clone());
        let args = run("mem-1", &[]);
        let (out, err, code) = invoke(
            &s.home,
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
        let reqs = p.requests();
        assert!(
            reqs[0].tools.iter().any(|t| t.name == "memory"),
            "a run offers the memory tool"
        );
        assert!(
            !reqs[0].messages[0].content.contains("What you remembered"),
            "nothing remembered yet, nothing in the prompt"
        );
        let reply = &reqs[1].messages.last().unwrap().content;
        assert!(
            reply.contains("remembered for the project (1 memory in"),
            "{reply}"
        );
        let (_h, steps) = s.harness_session("mem-1").unwrap();
        assert!(
            steps.iter().any(|st| st.kind == "memory"
                && st.command == "remember project: tests need cargo test -p app"),
            "{:?}",
            steps
                .iter()
                .map(|st| (st.kind.clone(), st.command.clone()))
                .collect::<Vec<_>>()
        );
        let files: Vec<PathBuf> = fs::read_dir(Path::new(&s.home).join("memory"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1, "{files:?}");
        let file = &files[0];
        assert_ne!(file.file_name().unwrap(), "user.md");
        let text = fs::read_to_string(file).unwrap();
        assert!(text.contains("] tests need cargo test -p app\n"), "{text}");

        // The next run in the same directory reads it into its prompt.
        let p = CommandFakeProvider::scripted(vec![say("SUMMARY: ok")]);
        set_fake_provider(p.clone());
        let args = run("mem-2", &[]);
        let (out, err, code) = invoke(
            &s.home,
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
        let system = &p.requests()[0].messages[0].content;
        assert!(
            system.contains("What you remembered in earlier runs")
                && system.contains("tests need cargo test -p app"),
            "{system}"
        );
        let (_h, steps) = s.harness_session("mem-2").unwrap();
        assert!(
            steps.iter().any(|st| st.kind == "note"
                && st.content.starts_with("read memory from ")
                && st.content.contains("into the system prompt")),
            "{:?}",
            steps
                .iter()
                .filter(|st| st.kind == "note")
                .map(|st| st.content.clone())
                .collect::<Vec<_>>()
        );

        // --no-memory and the setting switch it off.
        for extra in [vec!["--no-memory"], vec!["--bare"], vec![]] {
            if extra.is_empty() {
                s.set_setting("human", "harness.memory", "off").unwrap();
            }
            let p = CommandFakeProvider::scripted(vec![say("SUMMARY: ok")]);
            set_fake_provider(p.clone());
            let args = run("mem-3", &extra);
            let (out, err, code) = invoke(
                &s.home,
                &args.iter().map(String::as_str).collect::<Vec<_>>(),
            );
            assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
            let reqs = p.requests();
            assert!(
                !reqs[0].messages[0].content.contains("What you remembered"),
                "{extra:?}"
            );
            assert!(
                !reqs[0].tools.iter().any(|t| t.name == "memory"),
                "{extra:?}"
            );
        }
        s.set_setting("human", "harness.memory", "on").unwrap();

        // The chat sees the same file and edits it by hand.
        let p = CommandFakeProvider::scripted(vec![say("hi")]);
        set_fake_provider(p.clone());
        set_stdin_for_tests(Some(
            "/memory
/memory add user prefers tabs
/memory add
/memory add   project the API lives in crates/api
/memory forget api
/memory forget nothing like this
/memory clear
/memory clear user
/memory dance
hello
/memory
/exit
",
        ));
        let (out, err, code) = invoke(&s.home, &chat_args(cwd, &["--yes"]));
        set_stdin_for_tests(None);
        assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
        assert_eq!(out, "hi\n");
        assert!(
            err.contains("project memory (") && err.contains("): 1 memory\n  - ["),
            "{err}"
        );
        assert!(err.contains("remembered for the user (1 in"), "{err}");
        assert!(err.contains("usage: /memory add [user] TEXT"), "{err}");
        assert!(err.contains("remembered for the project (2 in"), "{err}");
        assert!(
            err.contains("forgotten from the project (1 left in"),
            "{err}"
        );
        assert!(
            err.contains("nothing remembered contains \"nothing like this\""),
            "{err}"
        );
        assert!(err.contains("usage: /memory clear project|user"), "{err}");
        assert!(err.contains("cleared 1 from the user memory ("), "{err}");
        assert!(err.contains("unknown /memory dance"), "{err}");
        assert!(
            err.contains("user memory (") && err.contains("): nothing yet"),
            "{err}"
        );
        let system = &p.requests()[0].messages[0].content;
        assert!(
            system.contains("tests need cargo test -p app"),
            "the chat prompt carries the project memory: {system}"
        );
        assert!(!Path::new(&s.home).join("memory/user.md").exists());
    }

    #[test]
    fn harness_chat_searches_the_web_by_default_and_runs_only_when_told() {
        let _serial = crate::test_helpers::serial();
        let (_env, _guards) = command_env();
        let s = harness_command_store();
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        let queries: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seen = queries.clone();
        let page = crate::search::test_support::ddg_page(
            &[("Rust", "https://www.rust-lang.org/", "A language.")],
            false,
        );
        let server = Server::new(move |req| {
            seen.lock().unwrap().push(req.uri.clone());
            TestResponse::text(200, page.clone()).header("Content-Type", "text/html")
        });
        let _endpoint =
            pulse_testutil::setenv("PULSE_WEB_SEARCH_URL", format!("{}/html/", server.url()));

        // The chat offers the tool and the model's search answers.
        let p = CommandFakeProvider::scripted(vec![
            call("web_search", json!({"query": "rust lang", "count": 3})),
            say("found it"),
        ]);
        set_fake_provider(p.clone());
        set_stdin_for_tests(Some("/env\nwhere is rust\n/exit\n"));
        let (out, err, code) = invoke(&s.home, &chat_args(cwd, &["--yes"]));
        set_stdin_for_tests(None);
        assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
        assert_eq!(out, "found it\n");
        assert!(err.contains("web search: duckduckgo\n"), "{err}");
        assert!(
            err.contains("web_search tool on through duckduckgo"),
            "{err}"
        );
        let reqs = p.requests();
        assert!(reqs[0].tools.iter().any(|t| t.name == "web_search"));
        let reply = &reqs[1].messages.last().unwrap().content;
        assert!(
            reply.contains("1 result for \"rust lang\" from duckduckgo:\n\n1. Rust\n   https://www.rust-lang.org/\n   A language."),
            "{reply}"
        );
        assert_eq!(
            queries.lock().unwrap().as_slice(),
            ["/html/?q=rust+lang"],
            "the count is DuckDuckGo's own"
        );

        // A run leaves it out unless told; --web-search or the setting
        // bring it in; --no-web-search wins over both.
        let run = |session: &str, extra: &[&str]| -> Vec<String> {
            let mut v: Vec<String> = [
                "run",
                "-p",
                "do it",
                "-C",
                cwd,
                "--provider",
                FAKE_PROVIDER_NAME,
                "--session",
                session,
                "--quiet",
            ]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
            v.extend(extra.iter().map(|s| (*s).to_string()));
            v
        };
        let offered = |session: &str, extra: &[&str]| -> bool {
            let p = CommandFakeProvider::scripted(vec![say("SUMMARY: ok")]);
            set_fake_provider(p.clone());
            let args = run(session, extra);
            let (out, err, code) = invoke(
                &s.home,
                &args.iter().map(String::as_str).collect::<Vec<_>>(),
            );
            assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
            p.requests()[0].tools.iter().any(|t| t.name == "web_search")
        };
        assert!(!offered("ws-1", &[]), "jobs search only when told");
        assert!(offered("ws-2", &["--web-search"]));
        assert!(!offered("ws-3", &["--web-search", "--no-web-search"]));
        let (_h, steps) = s.harness_session("ws-2").unwrap();
        assert!(
            steps.iter().any(
                |st| st.kind == "note" && st.content == "web_search tool on through duckduckgo"
            ),
            "{:?}",
            steps
                .iter()
                .filter(|st| st.kind == "note")
                .map(|st| st.content.clone())
                .collect::<Vec<_>>()
        );
        s.set_setting("human", "harness.web_search", "on").unwrap();
        assert!(offered("ws-4", &[]));
        s.set_setting("human", "harness.web_search", "off").unwrap();
        assert!(!offered("ws-5", &[]));
        assert!(offered("ws-6", &["--web-search"]), "the flag beats off");

        // With the setting off the chat stays quiet too; Brave's key picks
        // its engine.
        let p = CommandFakeProvider::scripted(vec![say("hi")]);
        set_fake_provider(p.clone());
        set_stdin_for_tests(Some("hello\n/exit\n"));
        let (out, err, code) = invoke(&s.home, &chat_args(cwd, &["--yes"]));
        set_stdin_for_tests(None);
        assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
        assert!(!p.requests()[0].tools.iter().any(|t| t.name == "web_search"));
        s.set_setting("human", "harness.web_search", "chat")
            .unwrap();
        let _key = pulse_testutil::setenv("BRAVE_SEARCH_API_KEY", "BSA-test");
        let p = CommandFakeProvider::scripted(vec![say("hi")]);
        set_fake_provider(p.clone());
        set_stdin_for_tests(Some("/env\n/exit\n"));
        let (out, err, code) = invoke(&s.home, &chat_args(cwd, &["--yes"]));
        set_stdin_for_tests(None);
        assert_eq!(code, 0, "stdout {out:?} stderr {err:?}");
        assert!(err.contains("web search: brave\n"), "{err}");
    }
}
