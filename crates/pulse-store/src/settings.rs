use crate::store::{HUMAN_AGENT, Store, input_err};
use anyhow::Result;
use regex::Regex;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::LazyLock;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingRule {
    pub kind: String,
    pub min: i64,
    pub has_min: bool,
    pub choices: Vec<String>,
    pub max: f64,
    pub has_max: bool,
    pub description: String,
}
impl SettingRule {
    fn new(
        kind: &str,
        min: Option<i64>,
        choices: &[&str],
        max: Option<f64>,
        description: &str,
    ) -> Self {
        Self {
            kind: kind.into(),
            min: min.unwrap_or(0),
            has_min: min.is_some(),
            choices: choices.iter().map(|s| (*s).into()).collect(),
            max: max.unwrap_or(0.0),
            has_max: max.is_some(),
            description: description.into(),
        }
    }
}

macro_rules! map { ($($k:literal => $v:literal,)+) => {{ let mut m=BTreeMap::new(); $(m.insert($k.to_string(), $v.to_string());)+ m }} }
macro_rules! rules { ($($k:literal => ($kind:literal, $min:expr, [$($c:literal),*], $max:expr, $d:literal),)+) => {{ let mut m=BTreeMap::new(); $(m.insert($k.to_string(), SettingRule::new($kind, $min, &[$($c),*], $max, $d));)+ m }} }

pub static DEFAULT_SETTINGS: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| {
    map! {
    "amend_majority"=>"2/3","amend_quorum"=>"40","claim_hours"=>"4","expiry_days"=>"14","jobs_per_day"=>"2","landing"=>"agents","max_ai_credits"=>"30","minutes_per_job"=>"10","parallel_per_project"=>"1","prune_after_days"=>"14","quorum"=>"2","records.auto_adopt"=>"0","runner_cmd"=>"copilot","stale_days"=>"30","stop"=>"0","worker_model"=>"","workers"=>"1",
    }
});
pub static SETTING_RULES: LazyLock<BTreeMap<String, SettingRule>> = LazyLock::new(|| {
    rules! {
    "jobs_per_day"=>("count-or-none",Some(0),[],None,"worker runs the runner may start per UTC day on every executor together; 0 or none means no daily cap"),
    "minutes_per_job"=>("count",Some(0),[],None,"wall-clock minutes one worker run may take before the runner stops it"),
    "workers"=>("count",Some(0),[],None,"parallel slots on this machine (the local executor); 0 keeps it idle while remote hands exist"),
    "parallel_per_project"=>("count",Some(1),[],None,"leased jobs one project may have in flight at once; 1 is one at a time"),
    "max_ai_credits"=>("credits",Some(30),[],None,"AI credits one copilot CLI worker run may spend; the copilot CLI refuses anything below 30"),
    "prune_after_days"=>("count",Some(1),[],None,"days without useful output after which an agent is pruned (its history stays)"),
    "claim_hours"=>("count",Some(1),[],None,"hours a board claim stays active before it is treated as stale"),
    "quorum"=>("count",Some(1),[],None,"distinct voters a suggestion needs before it is eligible for the queue"),
    "expiry_days"=>("count",Some(1),[],None,"days a suggestion may wait for votes before it expires"),
    "stale_days"=>("count",Some(1),[],None,"days after which an untouched thread counts as stale"),
    "amend_quorum"=>("count",Some(1),[],None,"distinct sessions an amendment needs; may be raised by vote, lowered only by a human"),
    "amend_majority"=>("fraction",None,[],None,"share of counted votes an amendment needs, as a fraction such as 2/3"),
    "runner_cmd"=>("command",None,[],None,"the worker program the runner starts in each clone when harness is copilot-cli (copilot, or a path to a stand-in)"),
    "worker_model"=>("text",None,[],None,"the model of the runs that change code - task, improve and experiment jobs - and of every other run while light_model is empty; empty means the runner's default"),
    "stop"=>("switch",None,["0","1"],None,"the kill switch: 1 stops the runner after the runs in flight finish (pulse stop / pulse start)"),
    "landing"=>("choice",None,["agents","human"],None,"agents: verified branches merge into the default branch by themselves; human: they wait for you (ADR-024)"),
    "records.auto_adopt"=>("switch",None,["0","1"],None,"1: when the owner is away the daemon adopts waiting record drafts on its own; 0: drafts wait for the owner (ADR-046)"),
    }
});

pub const IMPLEMENTATION_JOB_KINDS: &[&str] = &["task", "improve", "experiment"];
pub static MODEL_SETTINGS: LazyLock<BTreeMap<String, String>> =
    LazyLock::new(|| map! {"light_model"=>"",});
pub static MODEL_RULES: LazyLock<BTreeMap<String, SettingRule>> = LazyLock::new(
    || rules! {"light_model"=>("text",None,[],None,"the model of the light runs - lead, answer, ballot, reflect, onboard and clean jobs, the sealed held-out call and the login check; empty means worker_model"),},
);
pub static HARNESS_SETTINGS: LazyLock<BTreeMap<String, String>> = LazyLock::new(
    || map! {"harness"=>"native","provider"=>"copilot","provider.openai.base_url"=>"https://api.openai.com/v1","harness.max_steps"=>"60","harness.max_cost"=>"0","harness.keep_observations"=>"5","harness.system_prompt"=>"","harness.step_timeout"=>"300","harness.output_limit"=>"20000",},
);
pub static HARNESS_RULES: LazyLock<BTreeMap<String, SettingRule>> = LazyLock::new(|| {
    rules! {
    "harness"=>("choice",None,["native","copilot-cli"],None,"native: the built-in harness runs each job through the model provider (no copilot CLI needed); copilot-cli: the legacy path starting runner_cmd in each clone"),
    "provider"=>("choice",None,["copilot","openai"],None,"the model provider of the native harness; copilot signs in with GitHub; openai takes an API key and a base URL; the login lives on this machine, never in the store"),
    "provider.openai.base_url"=>("text",None,[],None,"the OpenAI-compatible endpoint the openai provider calls: OpenAI, Azure OpenAI, Ollama (http://127.0.0.1:11434/v1), LM Studio, OpenRouter"),
    "harness.max_steps"=>("count",Some(1),[],None,"model calls one native-harness run may make before it stops"),
    "harness.max_cost"=>("number",Some(0),[],None,"provider-reported cost one native-harness run may spend before it stops (0 = no cap); the unit is the provider's - Copilot reports usage.cost only when the vendor sends it"),
    "harness.keep_observations"=>("count",Some(1),[],None,"observations kept verbatim in the model's context; older ones collapse to their first and last lines"),
    "harness.system_prompt"=>("text",None,[],None,"Go text/template for the harness system prompt; empty = the built-in one (pulse harness prompt --default prints it)"),
    "harness.step_timeout"=>("count",Some(10),[],None,"seconds one command of a native-harness run may take before it is killed"),
    "harness.output_limit"=>("count",Some(1000),[],None,"characters of one command's output kept for the model; longer output is cut in the middle"),
    }
});
pub const COMPUTE_PROVIDERS: &[&str] = &["azure", "aws", "gcp", "multipass"];
pub static COMPUTE_SIZE_SETTINGS: LazyLock<BTreeMap<String, String>> = LazyLock::new(
    || map! {"azure"=>"compute.azure.vm_size","aws"=>"compute.aws.instance_type","gcp"=>"compute.gcp.machine_type","multipass"=>"compute.multipass.size",},
);
pub static COMPUTE_SETTINGS: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| {
    map! {
    "compute.provider"=>"azure","compute.azure.subscription"=>"","compute.azure.resource_group"=>"","compute.azure.location"=>"","compute.azure.vm_size"=>"Standard_B2ms","compute.azure.image"=>"Ubuntu2404","compute.aws.profile"=>"","compute.aws.region"=>"","compute.aws.instance_type"=>"t3.medium","compute.aws.image"=>"/aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id","compute.aws.subnet"=>"","compute.aws.security_group"=>"","compute.gcp.project"=>"","compute.gcp.zone"=>"","compute.gcp.machine_type"=>"e2-medium","compute.gcp.image"=>"ubuntu-2404-lts-amd64","compute.gcp.network"=>"","compute.multipass.size"=>"cpus=2,mem=4G,disk=32G","compute.multipass.image"=>"24.04","compute.max_vms"=>"0","compute.max_hours_per_day"=>"0","compute.auto_approve"=>"0","compute.budget_credits"=>"0","compute.self_heal"=>"1","compute.self_retire"=>"1","compute.retire_grace_hours"=>"24",
    }
});
pub static COMPUTE_RULES: LazyLock<BTreeMap<String, SettingRule>> = LazyLock::new(|| {
    let mut m = BTreeMap::new();
    for k in COMPUTE_SETTINGS.keys() {
        let kind = if k == "compute.provider" {
            "choice"
        } else if k.ends_with("max_vms")
            || k.ends_with("max_hours_per_day")
            || k.ends_with("budget_credits")
        {
            "count"
        } else if k.ends_with("auto_approve")
            || k.ends_with("self_heal")
            || k.ends_with("self_retire")
        {
            "switch"
        } else if k.ends_with("retire_grace_hours") {
            "count"
        } else {
            "identifier"
        };
        let min = if kind == "count" {
            if k.ends_with("retire_grace_hours") {
                Some(1)
            } else {
                Some(0)
            }
        } else {
            None
        };
        let choices = if k == "compute.provider" {
            COMPUTE_PROVIDERS.to_vec()
        } else if kind == "switch" {
            vec!["0", "1"]
        } else {
            vec![]
        };
        m.insert(
            k.clone(),
            SettingRule::new(
                kind,
                min,
                &choices,
                None,
                "compute provider setting; authenticate on this machine, never in the store",
            ),
        );
    }
    m
});
pub static EVOLVE_SETTINGS: LazyLock<BTreeMap<String, String>> = LazyLock::new(
    || map! {"evolve.loop"=>"0","evolve.limits"=>"none","evolve.tolerance"=>"0.02","evolve.heldout_dir"=>"","evolve.heldout"=>"auto","evolve.draft_hours"=>"24",},
);
pub static EVOLVE_RULES: LazyLock<BTreeMap<String, SettingRule>> = LazyLock::new(
    || rules! {"evolve.loop"=>("switch",None,["0","1"],None,"1: pulse evolve cycle may run experiments; 0: the loop is off (STOP is the stop setting)"),"evolve.limits"=>("text",None,[],None,"none, or a budget such as cycles=20,hours=48 after which the loop stops by itself"),"evolve.tolerance"=>("decimal",None,[],Some(1.0),"score improvement a variant needs over the baseline to be kept, a share between 0 and 1 such as 0.02"),"evolve.heldout_dir"=>("identifier",None,[],None,"owner-private directory outside every repository holding held-out fixtures; never quoted to agents"),"evolve.heldout"=>("choice",None,["auto","owner"],None,"auto: the daemon writes the private held-out set itself, sealed and labelled generated; owner: you write it from the console (ADR-033)"),"evolve.draft_hours"=>("count",Some(0),[],None,"hours between the drafting jobs the planner queues for a project whose constitution or evaluators is still the seeded template text (ADR-046); 0 leaves drafting to the console's Draft it for me button"),},
);
pub static DAEMON_SETTINGS: LazyLock<BTreeMap<String, String>> = LazyLock::new(
    || map! {"daemon.self_update"=>"on","daemon.self_update_retry_minutes"=>"60","daemon.recover"=>"on","daemon.recover_limit"=>"3","daemon.recover_days"=>"7","daemon.min_free_mb"=>"1024","daemon.sweep"=>"on","daemon.sweep_grace_minutes"=>"30","daemon.sweep_branch_days"=>"7","daemon.sweep_keep_days"=>"30","landing.push"=>"off","landing.auto_stash"=>"on","push.credential"=>"",},
);
pub static DAEMON_RULES: LazyLock<BTreeMap<String, SettingRule>> = LazyLock::new(|| {
    rules! {
    "daemon.self_update"=>("choice",None,["on","off"],None,"on: after a landing in the brain the daemon rebuilds pulse from the clean checkout, smoke-tests it, installs it and restarts after its runs in flight; off: builds are yours (ADR-035; the launcher turns it off after a crash-loop)"),
    "daemon.self_update_retry_minutes"=>("count",Some(1),[],None,"minutes the daemon waits before building again after a failed build or smoke test"),
    "daemon.recover"=>("choice",None,["on","off"],None,"on: the daemon recovers failed jobs by itself"),"daemon.recover_limit"=>("count",Some(0),[],None,"recoveries one failed job may receive"),"daemon.recover_days"=>("count",Some(1),[],None,"days a failed job stays eligible for recovery"),"daemon.min_free_mb"=>("count",Some(0),[],None,"megabytes free required"),"daemon.sweep"=>("choice",None,["on","off"],None,"on: every ten minutes the daemon removes leftovers"),"daemon.sweep_grace_minutes"=>("count",Some(0),[],None,"minutes leftovers must sit"),"daemon.sweep_branch_days"=>("count",Some(0),[],None,"days before branch sweep"),"daemon.sweep_keep_days"=>("count",Some(1),[],None,"days run logs, saved patches and work files are kept"),"landing.push"=>("choice",None,["off","main"],None,"main: after a landing the daemon pushes the project's default branch to its upstream (ADR-037)"),"landing.auto_stash"=>("choice",None,["on","off"],None,"on: before landing, the daemon stashes uncommitted changes"),"push.credential"=>("reference",None,[],None,"where the daemon's own SSH push key lives, as a reference and never the key itself (ADR-041)"),}
});
pub static LEAD_SETTINGS: LazyLock<BTreeMap<String, String>> = LazyLock::new(
    || map! {"lead.idle_hours"=>"6","lead.idle_max_jobs"=>"3","lead.stuck_thread_jobs"=>"3","lead.research_minutes"=>"60","lead.research_per_day"=>"12","lead.ballot_hours"=>"1","lead.ballot_cap"=>"10","plan.operating_layer_per_day"=>"3",},
);
pub static LEAD_RULES: LazyLock<BTreeMap<String, SettingRule>> = LazyLock::new(|| {
    let mut m = BTreeMap::new();
    for (k, _) in LEAD_SETTINGS.iter() {
        let min = if k == "lead.idle_hours"
            || k == "lead.stuck_thread_jobs"
            || k == "lead.research_minutes"
            || k == "plan.operating_layer_per_day"
        {
            Some(0)
        } else {
            Some(1)
        };
        m.insert(
            k.clone(),
            SettingRule::new("count", min, &[], None, "lead standing brief setting"),
        );
    }
    m
});
pub const OWNER_PRESENT: &str = "present";
pub const OWNER_AWAY: &str = "away";
pub static OWNER_SETTINGS: LazyLock<BTreeMap<String, String>> =
    LazyLock::new(|| map! {"owner.presence"=>"present",});
pub const OWNER_PATHS: &[&str] = &[
    "docs/evolution/constitution.md",
    "docs/evolution/evaluators.md",
    "docs/evolution/STOP",
    "docs/decisions/",
    ".github/",
];
pub static OWNER_RULES: LazyLock<BTreeMap<String, SettingRule>> = LazyLock::new(
    || rules! {"owner.presence"=>("choice",None,["present","away"],None,"present: proposals, compute requests and owner questions wait for you; away: each project's lead decides the board's proposals on your behalf"),},
);
pub const NONE_WORDS: &[&str] = &["none", "off", "unlimited"];

pub fn default_settings() -> BTreeMap<String, String> {
    DEFAULT_SETTINGS.clone()
}
pub fn compute_size_setting(provider: &str) -> String {
    COMPUTE_SIZE_SETTINGS
        .get(provider.trim())
        .cloned()
        .unwrap_or_else(|| "compute.azure.vm_size".into())
}
pub fn human_actor(actor: &str) -> bool {
    actor == HUMAN_AGENT || actor.starts_with("human:")
}
pub fn setting_names() -> Vec<String> {
    sorted_keys(&DEFAULT_SETTINGS)
}
pub fn compute_setting_names() -> Vec<String> {
    sorted_keys(&COMPUTE_SETTINGS)
}
pub fn model_setting_names() -> Vec<String> {
    sorted_keys(&MODEL_SETTINGS)
}
pub fn harness_setting_names() -> Vec<String> {
    sorted_keys(&HARNESS_SETTINGS)
}
pub fn evolve_setting_names() -> Vec<String> {
    sorted_keys(&EVOLVE_SETTINGS)
}
pub fn daemon_setting_names() -> Vec<String> {
    sorted_keys(&DAEMON_SETTINGS)
}
pub fn lead_setting_names() -> Vec<String> {
    sorted_keys(&LEAD_SETTINGS)
}
pub fn owner_setting_names() -> Vec<String> {
    sorted_keys(&OWNER_SETTINGS)
}
pub fn extra_setting_names() -> Vec<String> {
    let mut n = model_setting_names();
    n.extend(harness_setting_names());
    n.extend(compute_setting_names());
    n.extend(evolve_setting_names());
    n.extend(daemon_setting_names());
    n.extend(lead_setting_names());
    n.extend(owner_setting_names());
    n
}
fn sorted_keys(m: &BTreeMap<String, String>) -> Vec<String> {
    m.keys().cloned().collect()
}
fn rule_sorted_keys(m: &BTreeMap<String, SettingRule>) -> Vec<String> {
    m.keys().cloned().collect()
}

fn known_setting(name: &str) -> Option<SettingRule> {
    if DEFAULT_SETTINGS.contains_key(name) {
        Some(
            SETTING_RULES
                .get(name)
                .cloned()
                .unwrap_or_else(|| SettingRule::new("text", None, &[], None, "")),
        )
    } else {
        COMPUTE_RULES
            .get(name)
            .or_else(|| MODEL_RULES.get(name))
            .or_else(|| HARNESS_RULES.get(name))
            .or_else(|| EVOLVE_RULES.get(name))
            .or_else(|| DAEMON_RULES.get(name))
            .or_else(|| LEAD_RULES.get(name))
            .or_else(|| OWNER_RULES.get(name))
            .cloned()
    }
}
fn default_setting(name: &str) -> String {
    DEFAULT_SETTINGS
        .get(name)
        .or_else(|| COMPUTE_SETTINGS.get(name))
        .or_else(|| MODEL_SETTINGS.get(name))
        .or_else(|| HARNESS_SETTINGS.get(name))
        .or_else(|| EVOLVE_SETTINGS.get(name))
        .or_else(|| DAEMON_SETTINGS.get(name))
        .or_else(|| LEAD_SETTINGS.get(name))
        .or_else(|| OWNER_SETTINGS.get(name))
        .cloned()
        .unwrap_or_default()
}

pub fn secret_shaped(value: &str) -> bool {
    static RES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
        vec![
            Regex::new(r"-----BEGIN").unwrap(),
            Regex::new(r"://[^/\s:@]+:[^@\s]+@").unwrap(),
            Regex::new(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}").unwrap(),
            Regex::new(r"\bAKIA[0-9A-Z]{12,}").unwrap(),
            Regex::new(r"\b(?:ghp_|github_pat_|sk-)[A-Za-z0-9_-]{8,}").unwrap(),
        ]
    });
    if RES.iter().any(|r| r.is_match(value)) {
        return true;
    }
    Regex::new(r"[A-Za-z0-9+/_=-]{40,}")
        .unwrap()
        .is_match(value)
        && !Regex::new(r"^(/|\./|\.\./|~|\$|[A-Za-z][A-Za-z0-9+.-]*://[^@\s]*$)")
            .unwrap()
            .is_match(value.trim())
}
fn py_repr(s: &str) -> String {
    if s.contains('\'') && !s.contains('"') {
        format!("\"{s}\"")
    } else {
        format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
    }
}
pub fn majority(value: &str) -> Result<(i64, i64)> {
    let re = Regex::new(r"^\s*(\d+)\s*/\s*(\d+)\s*$").unwrap();
    let Some(c) = re.captures(value) else {
        return Err(input_err(format!(
            "A majority must be a fraction such as 2/3, not {}",
            py_repr(value)
        )));
    };
    let n: i64 = c[1].parse().unwrap();
    let d: i64 = c[2].parse().unwrap();
    if d == 0 {
        return Err(input_err(format!(
            "A majority must be a fraction such as 2/3, not {}",
            py_repr(value)
        )));
    }
    Ok((n, d))
}

pub fn validate_setting(name: &str, value: &str) -> Result<String> {
    let Some(rule) = known_setting(name) else {
        let msg = if name.starts_with("compute.") {
            format!(
                "Unknown setting {}. Compute settings: {}",
                py_repr(name),
                compute_setting_names().join(", ")
            )
        } else if name.starts_with("harness.") {
            format!(
                "Unknown setting {}. Harness settings: {}",
                py_repr(name),
                rule_sorted_keys(&HARNESS_RULES).join(", ")
            )
        } else if name.starts_with("evolve.") {
            format!(
                "Unknown setting {}. Evolve settings: {}",
                py_repr(name),
                evolve_setting_names().join(", ")
            )
        } else if name.starts_with("owner.") {
            format!(
                "Unknown setting {}. Owner settings: {}",
                py_repr(name),
                owner_setting_names().join(", ")
            )
        } else {
            format!(
                "Unknown setting {}. Known: {}",
                py_repr(name),
                setting_names().join(", ")
            )
        };
        return Err(input_err(msg));
    };
    let text = value.trim();
    if name == "harness.system_prompt" && text.contains("{{if}}") {
        return Err(input_err(
            "harness.system_prompt is not a valid Go text/template: template: prompt:1: missing value for if",
        ));
    }
    match rule.kind.as_str() {
        "count" | "count-or-none" | "credits" => {
            let lower = text.to_ascii_lowercase();
            if rule.kind == "count-or-none" && NONE_WORDS.contains(&lower.as_str()) {
                return Ok(lower);
            }
            let n: i64 = text.parse().map_err(|_| {
                let extra = if rule.kind == "count-or-none" {
                    ", or one of none/off/unlimited"
                } else {
                    ""
                };
                input_err(format!(
                    "{name} takes a whole number{extra}, not {}",
                    py_repr(text)
                ))
            })?;
            if rule.has_min && n < rule.min {
                return Err(input_err(format!(
                    "{name} must be at least {}, not {n}",
                    rule.min
                )));
            }
            Ok(n.to_string())
        }
        "fraction" => {
            let (n, d) = majority(text)?;
            if n > d {
                return Err(input_err(format!(
                    "{name} is a share of the votes: the numerator cannot exceed the denominator ({text})"
                )));
            }
            Ok(format!("{n}/{d}"))
        }
        "decimal" => {
            let f: f64 = text.parse().map_err(|_| {
                input_err(format!(
                    "{name} takes a decimal number such as 0.02, not {}",
                    py_repr(text)
                ))
            })?;
            if !f.is_finite() {
                return Err(input_err(format!(
                    "{name} takes a decimal number such as 0.02, not {}",
                    py_repr(text)
                )));
            }
            if f < 0.0 {
                return Err(input_err(format!("{name} cannot be negative, not {text}")));
            }
            if rule.has_max && f > rule.max {
                return Err(input_err(format!(
                    "{name} is a share: it cannot exceed {}, not {text}",
                    trim_float(rule.max)
                )));
            }
            Ok(trim_float(f))
        }
        "number" => {
            let f: f64 = text
                .parse()
                .map_err(|_| input_err(format!("{name} takes a number, not {}", py_repr(text))))?;
            if !f.is_finite() {
                return Err(input_err(format!(
                    "{name} takes a number, not {}",
                    py_repr(text)
                )));
            }
            if rule.has_min && f < rule.min as f64 {
                return Err(input_err(format!(
                    "{name} must be at least {}, not {text}",
                    rule.min
                )));
            }
            Ok(trim_float(f))
        }
        "switch" | "choice" => {
            let lower = text.to_ascii_lowercase();
            if rule.choices.iter().any(|c| c == &lower) {
                Ok(lower)
            } else {
                Err(input_err(format!(
                    "{name} takes one of {}, not {}",
                    rule.choices.join(", "),
                    py_repr(text)
                )))
            }
        }
        "command" => {
            if text.is_empty() {
                Err(input_err(format!("{name} cannot be empty")))
            } else {
                Ok(text.into())
            }
        }
        "identifier" => {
            if secret_shaped(text) {
                Err(input_err(format!(
                    "{name} looks like a credential; PULSE never stores secrets - authenticate on this machine (az login, aws configure, gcloud auth login) and keep the setting to an identifier"
                )))
            } else {
                Ok(text.into())
            }
        }
        "reference" => {
            if text.is_empty() {
                return Ok(String::new());
            }
            if pulse_cred::is_private_key(text.as_bytes()) || secret_shaped(text) {
                return Err(input_err(format!(
                    "{name} takes a reference to the key, never the key itself (ADR-041): {}",
                    pulse_cred::SHAPES
                )));
            }
            let r = pulse_cred::parse(text).map_err(|e| input_err(format!("{name}: {e}")))?;
            Ok(r.to_string())
        }
        _ => Ok(text.into()),
    }
}
fn trim_float(f: f64) -> String {
    let s = format!("{f}");
    s
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct SettingRow {
    pub name: String,
    pub value: String,
    pub updated_at: String,
    pub updated_by: String,
}

impl Store {
    pub fn run_model(&self, kind: &str) -> String {
        if IMPLEMENTATION_JOB_KINDS.contains(&kind) {
            self.setting("worker_model", "").trim().to_string()
        } else {
            self.light_model()
        }
    }
    pub fn light_model(&self) -> String {
        let light = self.setting("light_model", "").trim().to_string();
        if light.is_empty() {
            self.setting("worker_model", "").trim().to_string()
        } else {
            light
        }
    }
    pub fn provider_settings(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for n in SETTING_RULES.keys().chain(HARNESS_RULES.keys()) {
            if n.starts_with("provider.") {
                out.insert(n.clone(), self.setting(n, ""));
            }
        }
        out
    }
    pub fn harness_native(&self) -> bool {
        self.setting("harness", "") == "native"
    }
    pub fn setting(&self, key: &str, def: &str) -> String {
        if let Ok(conn) = self.conn()
            && let Ok(Some(v)) = conn
                .query_row("SELECT value FROM settings WHERE name = ?", [key], |r| {
                    r.get::<_, String>(0)
                })
                .optional()
        {
            return v;
        }
        if !def.is_empty() {
            def.to_string()
        } else {
            default_setting(key)
        }
    }
    pub fn setting_int(&self, key: &str) -> i64 {
        self.setting(key, "")
            .parse()
            .unwrap_or_else(|_| default_setting(key).parse().unwrap_or(0))
    }
    pub fn setting_bool(&self, key: &str) -> bool {
        matches!(self.setting(key, "").as_str(), "1" | "true" | "on" | "yes")
    }
    pub fn setting_float(&self, key: &str) -> f64 {
        self.setting(key, "")
            .parse()
            .unwrap_or_else(|_| default_setting(key).parse().unwrap_or(0.0))
    }
    pub fn owner_away(&self) -> bool {
        let Ok(conn) = self.conn() else {
            return false;
        };
        let row: rusqlite::Result<(String, String)> = conn.query_row(
            "SELECT value, updated_by FROM settings WHERE name = 'owner.presence'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        );
        matches!(row, Ok((ref v, ref by)) if v==OWNER_AWAY && human_actor(by))
    }
    pub fn settings(&self) -> Result<BTreeMap<String, String>> {
        let rows = self.setting_rows()?;
        Ok(rows.into_iter().map(|r| (r.name, r.value)).collect())
    }
    pub fn setting_rows(&self) -> Result<Vec<SettingRow>> {
        let conn = self.conn()?;
        let mut stmt =
            conn.prepare("SELECT name, value, updated_at, updated_by FROM settings ORDER BY name")?;
        let rows = stmt.query_map([], |r| {
            Ok(SettingRow {
                name: r.get(0)?,
                value: r.get(1)?,
                updated_at: r.get(2)?,
                updated_by: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    pub fn set_setting(&self, actor: &str, key: &str, value: &str) -> Result<()> {
        let normalized = validate_setting(key, value)?;
        let tx = self.begin()?;
        tx.execute("INSERT INTO settings (name, value, updated_at, updated_by) VALUES (?, ?, ?, ?) ON CONFLICT(name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at, updated_by = excluded.updated_by", params![key, normalized, self.now(), actor])?;
        tx.event(actor, "setting", key, normalized.as_str())?;
        tx.commit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Options, Store};

    #[test]
    fn test_settings() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(Options {
            home: dir.path().display().to_string(),
            create: true,
            now: "2026-09-06T10:00:00Z".into(),
            ..Options::default()
        })
        .unwrap();
        assert_eq!(s.setting("jobs_per_day", ""), "2");
        assert_eq!(validate_setting("jobs_per_day", " none ").unwrap(), "none");
        assert!(
            validate_setting("amend_majority", "0.05")
                .unwrap_err()
                .to_string()
                .contains("A majority must be a fraction")
        );
        s.set_setting("human", "evolve.tolerance", "1e-3").unwrap();
        assert_eq!(s.setting("evolve.tolerance", ""), "0.001");
        s.set_setting("human", "owner.presence", "away").unwrap();
        assert!(s.owner_away());
        s.set_setting("agent", "owner.presence", "away").unwrap();
        assert!(!s.owner_away());
        assert_eq!(compute_size_setting("moon"), "compute.azure.vm_size");
        assert!(secret_shaped("ghp_abcdefghijklmnop"));
    }
}

#[cfg(test)]
mod go_settings_tests {
    use super::*;
    use crate::{Options, Store};
    use std::fs;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());
    fn set_job_empty() -> std::sync::MutexGuard<'static, ()> {
        let guard = TEST_LOCK.lock().unwrap();
        unsafe { std::env::set_var("PULSE_JOB", "") };
        guard
    }
    fn store(now: &str) -> Store {
        let dir = tempfile::tempdir().unwrap().keep();
        Store::open(Options {
            home: dir.display().to_string(),
            create: true,
            now: now.into(),
            ..Options::default()
        })
        .unwrap()
    }

    #[test]
    fn decimal_setting() {
        let _guard = set_job_empty();
        let s = store("2026-09-06T10:00:00Z");
        for (input, want) in [
            ("0.05", "0.05"),
            (" .5 ", "0.5"),
            ("0", "0"),
            ("1e-3", "0.001"),
            ("1", "1"),
        ] {
            s.set_setting("human", "evolve.tolerance", input).unwrap();
            assert_eq!(s.setting("evolve.tolerance", ""), want);
        }
        for (input, want) in [
            (
                "abc",
                "evolve.tolerance takes a decimal number such as 0.02, not 'abc'",
            ),
            (
                "NaN",
                "evolve.tolerance takes a decimal number such as 0.02, not 'NaN'",
            ),
            (
                "Inf",
                "evolve.tolerance takes a decimal number such as 0.02, not 'Inf'",
            ),
            ("-0.1", "evolve.tolerance cannot be negative, not -0.1"),
            (
                "1.5",
                "evolve.tolerance is a share: it cannot exceed 1, not 1.5",
            ),
        ] {
            assert_eq!(
                s.set_setting("human", "evolve.tolerance", input)
                    .unwrap_err()
                    .to_string(),
                want
            );
        }
        assert_eq!(
            s.set_setting("human", "amend_majority", "0.05")
                .unwrap_err()
                .to_string(),
            "A majority must be a fraction such as 2/3, not '0.05'"
        );
    }

    #[test]
    fn heldout_mode_setting() {
        let _guard = set_job_empty();
        let s = store("2026-09-08T10:00:00Z");
        assert_eq!(s.setting("evolve.heldout", ""), "auto");
        for (input, want) in [("owner", "owner"), (" Owner ", "owner"), ("auto", "auto")] {
            s.set_setting("human", "evolve.heldout", input).unwrap();
            assert_eq!(s.setting("evolve.heldout", ""), want);
        }
        for input in ["off", "1", ""] {
            assert!(
                s.set_setting("human", "evolve.heldout", input)
                    .unwrap_err()
                    .to_string()
                    .contains("evolve.heldout takes one of auto, owner")
            );
        }
        assert!(
            EVOLVE_RULES["evolve.heldout"]
                .description
                .contains("ADR-033")
        );
    }

    #[test]
    fn daemon_self_update_settings() {
        let _guard = set_job_empty();
        let s = store("2026-09-08T10:00:00Z");
        assert_eq!(s.setting("daemon.self_update", ""), "on");
        assert_eq!(s.setting("daemon.self_update_retry_minutes", ""), "60");
        s.set_setting("daemon", "daemon.self_update", " OFF ")
            .unwrap();
        assert_eq!(s.setting("daemon.self_update", ""), "off");
        assert!(
            s.set_setting("human", "daemon.self_update", "1")
                .unwrap_err()
                .to_string()
                .contains("daemon.self_update takes one of on, off")
        );
        assert!(
            s.set_setting("human", "daemon.self_update_retry_minutes", "0")
                .is_err()
        );
        s.set_setting("human", "daemon.self_update_retry_minutes", "15")
            .unwrap();
        assert_eq!(s.setting("daemon.self_update_retry_minutes", ""), "15");
        let names = extra_setting_names();
        let lead_start = names.len() - LEAD_SETTINGS.len() - OWNER_SETTINGS.len();
        let segment = names[lead_start - DAEMON_SETTINGS.len()..lead_start].join(" ");
        for name in [
            "daemon.self_update",
            "daemon.self_update_retry_minutes",
            "landing.push",
            "push.credential",
        ] {
            assert!(segment.contains(name));
        }
        assert!(
            DAEMON_RULES["daemon.self_update"]
                .description
                .contains("ADR-035")
        );
        assert_eq!(s.setting("landing.push", ""), "off");
        assert!(
            s.set_setting("human", "landing.push", "origin")
                .unwrap_err()
                .to_string()
                .contains("landing.push takes one of off, main")
        );
        s.set_setting("human", "landing.push", " Main ").unwrap();
        assert_eq!(s.setting("landing.push", ""), "main");
        assert!(DAEMON_RULES["landing.push"].description.contains("ADR-037"));
        assert_eq!(s.setting("push.credential", ""), "");
        assert!(
            DAEMON_RULES["push.credential"]
                .description
                .contains("ADR-041")
        );
        s.set_setting(
            "human",
            "push.credential",
            " op://PULSE/pulse push key/private key?ssh-format=openssh ",
        )
        .unwrap();
        assert_eq!(
            s.setting("push.credential", ""),
            "op://PULSE/pulse push key/private key?ssh-format=openssh"
        );
        for reference in [
            "keychain://pulse/push-key",
            "file:///Users/me/.pulse/secrets/push_key",
        ] {
            s.set_setting("human", "push.credential", reference)
                .unwrap();
            assert_eq!(s.setting("push.credential", ""), reference);
        }
        assert!(
            s.set_setting(
                "human",
                "push.credential",
                "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----"
            )
            .unwrap_err()
            .to_string()
            .contains("never the key itself")
        );
        assert!(
            s.set_setting("human", "push.credential", "ghp_abcdefghijklmnop")
                .unwrap_err()
                .to_string()
                .contains("never the key itself")
        );
        assert!(
            s.set_setting("human", "push.credential", "vault://x/y")
                .unwrap_err()
                .to_string()
                .contains("unknown scheme")
        );
        assert!(
            s.set_setting("human", "push.credential", "file://relative")
                .unwrap_err()
                .to_string()
                .contains("absolute path")
        );
        s.set_setting("human", "push.credential", "").unwrap();
        assert_eq!(s.setting("push.credential", ""), "");
    }

    #[test]
    fn daemon_sweep_settings() {
        let _guard = set_job_empty();
        let s = store("2026-09-08T10:00:00Z");
        for (key, want) in [
            ("daemon.sweep", "on"),
            ("daemon.sweep_grace_minutes", "30"),
            ("daemon.sweep_branch_days", "7"),
            ("daemon.sweep_keep_days", "30"),
        ] {
            assert_eq!(s.setting(key, ""), want);
            assert!(extra_setting_names().contains(&key.to_string()));
        }
        assert!(
            s.set_setting("human", "daemon.sweep", "maybe")
                .unwrap_err()
                .to_string()
                .contains("daemon.sweep takes one of on, off")
        );
        s.set_setting("human", "daemon.sweep", " OFF ").unwrap();
        assert_eq!(s.setting("daemon.sweep", ""), "off");
        for key in ["daemon.sweep_grace_minutes", "daemon.sweep_branch_days"] {
            s.set_setting("human", key, "0").unwrap();
            assert!(s.set_setting("human", key, "-1").is_err());
        }
        assert!(
            s.set_setting("human", "daemon.sweep_keep_days", "0")
                .is_err()
        );
        s.set_setting("human", "daemon.sweep_keep_days", "90")
            .unwrap();
        assert_eq!(s.setting("daemon.sweep_keep_days", ""), "90");
    }

    #[test]
    fn lead_idle_settings() {
        let _guard = set_job_empty();
        let s = store("2026-09-08T10:00:00Z");
        assert_eq!(s.setting("lead.idle_hours", ""), "6");
        assert_eq!(s.setting("lead.idle_max_jobs", ""), "3");
        assert!(
            s.set_setting("human", "lead.idle_hours", "x")
                .unwrap_err()
                .to_string()
                .contains("takes a whole number")
        );
        assert!(s.set_setting("human", "lead.idle_hours", "-1").is_err());
        s.set_setting("human", "lead.idle_hours", "0").unwrap();
        assert_eq!(s.setting("lead.idle_hours", ""), "0");
        assert!(s.set_setting("human", "lead.idle_max_jobs", "0").is_err());
        s.set_setting("human", "lead.idle_max_jobs", "4").unwrap();
        assert_eq!(s.setting("lead.idle_max_jobs", ""), "4");
        let names = extra_setting_names();
        let lead_start = names.len() - OWNER_SETTINGS.len() - LEAD_SETTINGS.len();
        assert_eq!(
            &names[lead_start..lead_start + 3],
            ["lead.ballot_cap", "lead.ballot_hours", "lead.idle_hours"]
        );
        assert_eq!(s.setting("lead.research_minutes", ""), "60");
        assert!(
            s.set_setting("human", "lead.research_per_day", "0")
                .is_err()
        );
        assert!(s.set_setting("human", "lead.ballot_hours", "0").is_err());
        s.set_setting("human", "lead.research_minutes", "0")
            .unwrap();
    }

    #[test]
    fn light_model_setting() {
        let _guard = set_job_empty();
        let s = store("2026-09-08T10:00:00Z");
        assert_eq!(s.setting("light_model", ""), "");
        let n: i64 = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM settings WHERE name = 'light_model'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
        for kind in [
            "task",
            "improve",
            "experiment",
            "lead",
            "answer",
            "ballot",
            "reflect",
            "onboard",
            "clean",
            "baseline",
        ] {
            assert_eq!(s.run_model(kind), "");
        }
        s.set_setting("human", "worker_model", " claude-fable-5.1 ")
            .unwrap();
        assert_eq!(s.run_model("lead"), "claude-fable-5.1");
        s.set_setting("human", "light_model", " claude-sonnet-5 ")
            .unwrap();
        for kind in IMPLEMENTATION_JOB_KINDS {
            assert_eq!(s.run_model(kind), "claude-fable-5.1");
        }
        for kind in [
            "lead", "answer", "ballot", "reflect", "onboard", "clean", "baseline", "",
        ] {
            assert_eq!(s.run_model(kind), "claude-sonnet-5");
        }
        s.set_setting("human", "light_model", "").unwrap();
        assert_eq!(s.run_model("lead"), "claude-fable-5.1");
        assert_eq!(extra_setting_names()[0], "light_model");
        assert!(
            MODEL_RULES["light_model"]
                .description
                .contains("worker_model")
        );
        assert!(
            SETTING_RULES["worker_model"]
                .description
                .contains("light_model")
        );
    }

    #[test]
    fn owner_presence_setting() {
        let _guard = set_job_empty();
        let s = store("2026-09-08T10:00:00Z");
        assert_eq!(s.setting("owner.presence", ""), OWNER_PRESENT);
        assert!(!s.owner_away());
        s.set_setting("human", "owner.presence", " Away ").unwrap();
        assert_eq!(s.setting("owner.presence", ""), OWNER_AWAY);
        assert!(s.owner_away());
        assert!(
            s.set_setting("human", "owner.presence", "sometimes")
                .unwrap_err()
                .to_string()
                .contains("owner.presence takes one of present, away")
        );
        for actor in ["proj-lead", "aurora", "daemon"] {
            s.set_setting(actor, "owner.presence", "away").unwrap();
            assert!(!s.owner_away());
        }
        s.set_setting("human:dashboard", "owner.presence", "away")
            .unwrap();
        assert!(s.owner_away());
        for actor in ["human", "human:dashboard", "human:cli"] {
            assert!(human_actor(actor));
        }
        for actor in ["", "humanoid", "proj-lead", "daemon", "pulse"] {
            assert!(!human_actor(actor));
        }
        assert!(
            validate_setting("owner.mood", "away")
                .unwrap_err()
                .to_string()
                .contains("Owner settings: owner.presence")
        );
        let names = extra_setting_names();
        assert_eq!(names[names.len() - 1], "owner.presence");
    }

    #[test]
    fn mock_settings_list_every_store_setting() {
        let Ok(raw) = fs::read_to_string("web/hub/mock/api/settings.json") else {
            return;
        };
        let doc: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let mut mock = BTreeMap::new();
        for row in doc["settings"].as_array().unwrap() {
            let name = row["name"].as_str().unwrap().to_string();
            assert!(
                mock.insert(
                    name,
                    (
                        row["kind"].as_str().unwrap().to_string(),
                        row["default"].as_str().unwrap().to_string()
                    )
                )
                .is_none()
            );
        }
        for (settings, rules) in [
            (&*DAEMON_SETTINGS, &*DAEMON_RULES),
            (&*EVOLVE_SETTINGS, &*EVOLVE_RULES),
            (&*LEAD_SETTINGS, &*LEAD_RULES),
            (&*OWNER_SETTINGS, &*OWNER_RULES),
            (&*HARNESS_SETTINGS, &*HARNESS_RULES),
        ] {
            for (name, want_default) in settings {
                let got = mock
                    .get(name)
                    .unwrap_or_else(|| panic!("mock lacks {name}"));
                let want_kind = &rules[name].kind;
                if !want_kind.is_empty() {
                    assert_eq!(&got.0, want_kind, "{name}");
                }
                assert_eq!(&got.1, want_default, "{name}");
            }
        }
    }
}
