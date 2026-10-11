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
    "amend_majority"=>"2/3","amend_quorum"=>"40","claim_hours"=>"4","expiry_days"=>"14","jobs_per_day"=>"2","inbox.retire_days"=>"7","landing"=>"agents","landing.parallel"=>"4","max_ai_credits"=>"30","minutes_per_job"=>"10","parallel_per_project"=>"1","prune_after_days"=>"14","quorum"=>"2","records.auto_adopt"=>"0","runner_cmd"=>"copilot","stale_days"=>"30","stop"=>"0","worker_model"=>"","workers"=>"1",
    }
});
pub static SETTING_RULES: LazyLock<BTreeMap<String, SettingRule>> = LazyLock::new(|| {
    rules! {
        "jobs_per_day"=>("count-or-none",Some(0),[],None,"worker runs the runner may start per UTC day on every executor together; 0 or none means no daily cap"),
        "inbox.retire_days"=>("count",Some(0),[],None,"days before daemon maintenance may close an open owner question only when it is an older duplicate of a newer open question or its job was cancelled; 0 turns the owner-inbox retire sweep off"),
        "minutes_per_job"=>("count",Some(0),[],None,"wall-clock minutes one worker run may take before the runner stops it"),
        "landing.parallel"=>("count",Some(1),[],None,"projects whose finished branches the daemon may land in parallel; 1 keeps the serial maintenance-pass behaviour"),
        "workers"=>("count",Some(0),[],None,"parallel slots on this machine (the local executor); 0 keeps it idle while remote hands exist"),
        "parallel_per_project"=>("count",Some(1),[],None,"queued or leased jobs one project may have in flight at once (experiments aside); the planner fills every free lane with voted cards each loop, so this is also how many cards a project works in parallel; 1 is one at a time"),
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
pub static MODEL_RULES: LazyLock<BTreeMap<String, SettingRule>> = LazyLock::new(|| {
    rules! {
        "light_model"=>("text",None,[],None,"the model of the light runs - lead, answer, ballot, reflect, onboard and clean jobs, the sealed held-out call and the login check; empty means worker_model"),
    }
});
pub static HARNESS_SETTINGS: LazyLock<BTreeMap<String, String>> = LazyLock::new(
    || map! {"harness"=>"native","provider"=>"copilot","provider.openai.base_url"=>"https://api.openai.com/v1","provider.retry_after_cap_s"=>"60","provider.max_body_mb"=>"8","harness.max_steps"=>"60","harness.max_cost"=>"0","harness.keep_observations"=>"5","harness.system_prompt"=>"","harness.step_timeout"=>"300","harness.output_limit"=>"20000","harness.format_retries"=>"1",},
);
pub static HARNESS_RULES: LazyLock<BTreeMap<String, SettingRule>> = LazyLock::new(|| {
    rules! {
        "harness"=>("choice",None,["native","copilot-cli"],None,"native: the built-in harness runs each job through the model provider (no copilot CLI needed); copilot-cli: the legacy path starting runner_cmd in each clone"),
        "provider"=>("choice",None,["copilot","openai"],None,"the model provider of the native harness; copilot signs in with GitHub; openai takes an API key and a base URL; the login lives on this machine, never in the store"),
        "provider.openai.base_url"=>("text",None,[],None,"the OpenAI-compatible endpoint the openai provider calls: OpenAI, Azure OpenAI, Ollama (http://127.0.0.1:11434/v1), LM Studio, OpenRouter"),
        "provider.retry_after_cap_s"=>("count",Some(0),[],None,"maximum seconds a provider Retry-After or rate-limit reset header may hold one retry before jitter; the backoff still floors the wait"),
        "provider.max_body_mb"=>("count",Some(1),[],None,"maximum MiB read from a successful provider response body before the call fails as non-retryable"),
        "harness.max_steps"=>("count",Some(1),[],None,"model calls one native-harness run may make before it stops"),
        "harness.max_cost"=>("number",Some(0),[],None,"provider-reported cost one native-harness run may spend before it stops (0 = no cap); the unit is the provider's - Copilot reports usage.cost only when the vendor sends it"),
        "harness.keep_observations"=>("count",Some(1),[],None,"observations kept verbatim in the model's context; older ones collapse to their first and last lines"),
        "harness.system_prompt"=>("text",None,[],None,"Go text/template for the harness system prompt; empty = the built-in one (pulse harness prompt --default prints it)"),
        "harness.step_timeout"=>("count",Some(10),[],None,"seconds one command of a native-harness run may take before it is killed"),
        "harness.output_limit"=>("count",Some(1000),[],None,"characters of one command's output kept for the model; longer output is cut in the middle"),
        "harness.format_retries"=>("count",Some(0),[],None,"prose-only native-harness replies corrected without consuming harness.max_steps before later format errors count normally"),
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
    let mut m = rules! {
        "compute.azure.subscription"=>("identifier",None,[],None,"Azure subscription id or name the runner provisions in (auth is az login on this machine)"),
        "compute.azure.resource_group"=>("identifier",None,[],None,"Azure resource group that holds the provisioned hands"),
        "compute.azure.location"=>("identifier",None,[],None,"Azure region, e.g. westeurope"),
        "compute.azure.vm_size"=>("identifier",None,[],None,"VM size for a provisioned hand, e.g. Standard_B2ms"),
        "compute.azure.image"=>("identifier",None,[],None,"VM image alias or URN, e.g. Ubuntu2404"),
        "compute.aws.profile"=>("identifier",None,[],None,"AWS CLI profile the runner provisions with; empty means the CLI's default (auth is aws configure or aws sso login on this machine)"),
        "compute.aws.region"=>("identifier",None,[],None,"AWS region the instances are created in, e.g. eu-west-1"),
        "compute.aws.instance_type"=>("identifier",None,[],None,"EC2 instance type for a provisioned hand, e.g. t3.medium"),
        "compute.aws.image"=>("identifier",None,[],None,"AMI id (ami-...) or an SSM public parameter path resolved at launch; the default is Canonical's current Ubuntu 24.04 amd64 image"),
        "compute.aws.subnet"=>("identifier",None,[],None,"subnet id the instances join; empty means the default VPC's default subnet in the region"),
        "compute.aws.security_group"=>("identifier",None,[],None,"security group id the instances get; empty means a group named pulse-hands that allows ssh in, created when missing"),
        "compute.gcp.project"=>("identifier",None,[],None,"Google Cloud project the instances are created in; empty means gcloud's current project (auth is gcloud auth login on this machine)"),
        "compute.gcp.zone"=>("identifier",None,[],None,"Compute Engine zone, e.g. europe-west4-a"),
        "compute.gcp.machine_type"=>("identifier",None,[],None,"machine type for a provisioned hand, e.g. e2-medium"),
        "compute.gcp.image"=>("identifier",None,[],None,"Ubuntu image family in ubuntu-os-cloud, e.g. ubuntu-2404-lts-amd64, or a full image path projects/P/global/images/I"),
        "compute.gcp.network"=>("identifier",None,[],None,"VPC network the instances join; empty means default. A firewall rule pulse-hands-ssh (tcp 22 to the tag pulse-hands) is created on it when missing"),
        "compute.multipass.size"=>("identifier",None,[],None,"size of a local multipass hand as cpus=N,mem=SIZE,disk=SIZE, e.g. cpus=2,mem=4G,disk=32G"),
        "compute.multipass.image"=>("identifier",None,[],None,"multipass image a local hand launches from, e.g. 24.04"),
        "compute.max_vms"=>("count",Some(0),[],None,"provisioned machines that may exist at once; 0 means agents cannot get compute"),
        "compute.max_hours_per_day"=>("count",Some(0),[],None,"machine-hours agents may consume per UTC day across provisioned hands; 0 means none"),
        "compute.auto_approve"=>("switch",None,["0","1"],None,"1: requests under the caps are approved without the owner; 0: every request becomes a proposal"),
        "compute.budget_credits"=>("count",Some(0),[],None,"AI credits the provisioned hands may spend per UTC day together; 0 means no extra budget"),
        "compute.self_heal"=>("switch",None,["0","1"],None,"1: the daemon places the executor token (pulse executor token) on a drained executor whose login is refused, once per drain; 0: it leaves the login to you (ADR-034)"),
        "compute.self_retire"=>("switch",None,["0","1"],None,"1: the daemon deallocates a VM it created whose login stays refused and retires its executor row, then deletes the VM after compute.retire_grace_hours; 0: it only reports (ADR-034)"),
        "compute.retire_grace_hours"=>("count",Some(1),[],None,"hours a retired VM stays stopped before the daemon deletes it; starting it again at the provider (the finding names the command) and `pulse executor activate NAME` within them keep it"),
    };
    m.insert("compute.provider".to_string(), SettingRule::new("choice", None, COMPUTE_PROVIDERS, None, "where agents get more machines from: azure (az login), aws (aws configure / aws sso login), gcp (gcloud auth login) or multipass (local VMs on the hub machine); each provider reads its own compute.<provider>.* settings"));
    m
});
pub static EVOLVE_SETTINGS: LazyLock<BTreeMap<String, String>> = LazyLock::new(
    || map! {"evolve.loop"=>"0","evolve.limits"=>"none","evolve.tolerance"=>"0.02","evolve.heldout_dir"=>"","evolve.heldout"=>"auto","evolve.draft_hours"=>"24","evolve.measure_parallel"=>"4","evolve.flat_cycles"=>"6","evolve.baseline_ttl_hours"=>"24","evolve.parent_rate"=>"0.5","evolve.outcome_hours"=>"24","lane.retry_minutes"=>"20","lane.short_retry_minutes"=>"2","baseline.retry_hours"=>"6",},
);
pub static EVOLVE_RULES: LazyLock<BTreeMap<String, SettingRule>> = LazyLock::new(|| {
    rules! {
        "evolve.loop"=>("switch",None,["0","1"],None,"1: pulse evolve cycle may run experiments; 0: the loop is off (STOP is the stop setting)"),
        "evolve.limits"=>("text",None,[],None,"none, or a budget such as cycles=20,hours=48 after which the loop stops by itself"),
        "evolve.tolerance"=>("decimal",None,[],Some(1.0),"how far a variant's score may fall below the baseline and still be kept, a share between 0 and 1 such as 0.02; a tie is kept and the project's own check decides it at landing, only a larger regression is discarded"),
        "evolve.heldout_dir"=>("identifier",None,[],None,"owner-private directory outside every repository holding held-out fixtures; never quoted to agents"),
        "evolve.heldout"=>("choice",None,["auto","owner"],None,"auto: the daemon writes the private held-out set itself, sealed and labelled generated; owner: you write it from the console (ADR-033)"),
        "evolve.draft_hours"=>("count",Some(0),[],None,"hours between the drafting jobs the planner queues for a project whose constitution or evaluators is still the seeded template text (ADR-046); 0 leaves drafting to the console's Draft it for me button"),
        "evolve.measure_parallel"=>("count",Some(0),[],None,"fixtures one measurement may run at once; 0 or 1 keeps fixture measurement sequential. Each fixture's build gets the cores divided by this (CARGO_BUILD_JOBS) and every measurement process runs at nice 10"),
        "evolve.flat_cycles"=>("count",Some(0),[],None,"experiment cycles in a row that all tie (variant score = baseline score) before the loop pauses with one owner question on the project's evolution/loop-gate thread; a fixture enabled since, or an answer or decision on that question, lets it run again; at least 2, and 0 turns the gate off"),
        "evolve.baseline_ttl_hours"=>("count",Some(0),[],None,"hours an experiment lane may reuse a measured default-branch baseline for the same commit and fixture hash; 0 means cached baselines never expire"),
        "evolve.parent_rate"=>("decimal",None,[],Some(1.0),"share of experiments the planner builds on an archived variant (a discarded branch's patch, chosen by its lineage's productivity) instead of the default branch; 0 keeps every experiment on the default branch, 1 always builds on the archive when it has a candidate"),
        "evolve.outcome_hours"=>("count",Some(1),[],None,"hours after a landing that changed the improver's own crates before the daemon measures its outcome - landings per worker-hour, first pass, lead time across all projects - against the same window before it, and posts the delta as a result on the landing's thread"),
        "lane.retry_minutes"=>("count",Some(1),[],None,"minutes the experiment lane waits before retrying a failed or deferred measured experiment"),
        "lane.short_retry_minutes"=>("count",Some(1),[],None,"minutes the experiment lane waits before retrying a transient lane failure, lock or STOP hold"),
        "baseline.retry_hours"=>("count",Some(1),[],None,"hours the daemon waits before planning another baseline measurement after a failed non-transient baseline job"),
    }
});
pub static DAEMON_SETTINGS: LazyLock<BTreeMap<String, String>> = LazyLock::new(
    || map! {"verify.lock"=>"off","daemon.self_update"=>"on","daemon.self_update_retry_minutes"=>"60","daemon.self_update_settle_minutes"=>"15","daemon.recover"=>"on","daemon.recover_limit"=>"3","daemon.recover_days"=>"7","daemon.min_free_mb"=>"10240","daemon.setup_mb"=>"1024","daemon.unfit_reprobe_minutes"=>"60","daemon.sweep"=>"on","daemon.sweep_grace_minutes"=>"30","daemon.sweep_branch_days"=>"7","daemon.sweep_keep_days"=>"30","daemon.transcript_keep_days"=>"14","daemon.log_keep_days"=>"14","daemon.log_max_mb"=>"32","daemon.vacuum_free_mb"=>"256","daemon.build_cache"=>"on","daemon.build_cache_keep_days"=>"7","landing.push"=>"off","landing.auto_stash"=>"on","landing.base_red_retry_minutes"=>"10","hygiene.cargo_target"=>"on","hygiene.cargo_target_hours"=>"24","landing.verify_memo"=>"on","landing.verify_memo_hours"=>"24","push.credential"=>"","heartbeat.dead_after"=>"300","board.stale_days"=>"7","daemon.planner_cache_ttl_seconds"=>"600","daemon.restart_drain_minutes"=>"30","daemon.pass_seconds"=>"60","daemon.max_jobs"=>"0","daemon.max_load"=>"0","daemon.min_free_mem_mb"=>"1024","daemon.nice"=>"5","daemon.build_jobs"=>"0",},
);
pub static DAEMON_RULES: LazyLock<BTreeMap<String, SettingRule>> = LazyLock::new(|| {
    rules! {
        "daemon.self_update"=>("choice",None,["on","off"],None,"on: after a landing in the brain the daemon rebuilds pulse from the clean checkout, smoke-tests it, installs it and restarts after its runs in flight; off: builds are yours (ADR-035; the launcher turns it off after a crash-loop)"),
        "daemon.self_update_retry_minutes"=>("count",Some(1),[],None,"minutes the daemon waits before building again after a failed build or smoke test"),
        "daemon.self_update_settle_minutes"=>("count",Some(0),[],None,"minutes the brain's HEAD must stand unchanged before the daemon rebuilds pulse onto it, so a burst of landings minutes apart builds once rather than once per landing; 0 builds as soon as HEAD differs"),
        "daemon.recover"=>("choice",None,["on","off"],None,"on: the daemon recovers failed jobs by itself - a transient failure runs again after a growing pause, a failure in the work becomes one corrected job carrying the failure, a structural one waits for its own fix - and writes the repeated causes down as memory records that later briefs carry; off: a failed job stays failed until `pulse job retry` (ADR-036)"),
        "daemon.recover_limit"=>("count",Some(0),[],None,"recoveries one failed job may receive (the same job run again or a corrected job queued for it) before the daemon holds it for the lead and the owner; 0 holds every failure at once"),
        "daemon.recover_days"=>("count",Some(1),[],None,"days a failed job stays eligible for recovery and counts toward a repeated-failure lesson"),
        "daemon.min_free_mb"=>("count",Some(0),[],None,"megabytes an executor's workdir filesystem must have free for its probe to pass and for a run's verify to start; under it the probe note says `under N free on <workdir>` and the hand takes no job until space returns; the default leaves room for a build and the clones beside it, since a run that hits ENOSPC mid-build is lost work; 0 turns the check off"),
        "daemon.setup_mb"=>("count",Some(0),[],None,"megabytes of work-filesystem space one run's clone and setup are taken to need until runs of the project have measured it (runs.work_mb); each tick the daemon starts no more local runs than the free space above daemon.min_free_mb pays for at the largest footprint an active project has shown, one run always when none is in flight; 0 turns the admission off and leaves the floor alone"),
        "daemon.unfit_reprobe_minutes"=>("count",Some(0),[],None,"minutes before the daemon re-probes an executor/project held by an unfit mark while queued work waits; repeated failures double this wait up to 24h, and 0 turns the daemon re-probe off"),
        "daemon.sweep"=>("choice",None,["on","off"],None,"on: every ten minutes the daemon removes what runs, landings and agents left on this machine and nothing holds any more - clones, keys and bundles of finished jobs under PULSE_HOME/work, landing worktrees whose process is gone, holding refs, run logs and patches past daemon.sweep_keep_days, pulse/* or evolve/* branches no job will land (saved as a patch first), and worktrees a project checkout lists whose work is on the default branch (its merged branch goes with it) or that sat untouched for daemon.sweep_branch_days (the branch stays); a run in flight, a branch waiting to land, a locked worktree, uncommitted changes and the owner's own files are never touched. off: `pulse sweep` by hand"),
        "daemon.sweep_grace_minutes"=>("count",Some(0),[],None,"minutes a finished job's clone, key or bundle, a dead landing's worktree and a clean project worktree whose every commit is on the default branch must sit untouched before the sweep removes them; a lost run's leftovers go at once"),
        "daemon.sweep_branch_days"=>("count",Some(0),[],None,"days since its last commit before a pulse/* or evolve/* branch that no job will land is removed, its commits saved under PULSE_HOME/patches first unless they are already on the default branch; also the days a clean worktree of a project with commits not on the default branch sits untouched before the worktree alone goes (its branch stays; a detached HEAD is saved as a patch first)"),
        "daemon.sweep_keep_days"=>("count",Some(1),[],None,"days saved patches and work files of jobs this store does not know are kept before the sweep removes them"),
        "daemon.transcript_keep_days"=>("count",Some(0),[],None,"days a harness session's step transcript is kept after the session ends - its harness_steps rows, its harness.step notification events and its PULSE_HOME/harness/<session>.json file - before the sweep removes them in small batches; the session row (counts, tokens, cost, submission) stays for good; 0 keeps every transcript"),
        "daemon.log_keep_days"=>("count",Some(1),[],None,"days run logs and daemon stall/signal logs under PULSE_HOME/logs are kept before the sweep removes them"),
        "daemon.log_max_mb"=>("count",Some(0),[],None,"megabytes daemon.log and hub.log may grow to before the sweep copies each to <name>.1 and truncates it in place (the launcher opens them in append mode, so the live process keeps writing); 0 never caps them"),
        "daemon.vacuum_free_mb"=>("count",Some(0),[],None,"megabytes of free pages the store may hold before the sweep gives them back to the file system: a store in incremental auto-vacuum mode frees a slice each pass, one that is not is converted with one full VACUUM only while no run or harness session is in flight and the disk has room for a copy; 0 never vacuums"),
        "daemon.build_cache"=>("choice",None,["on","off"],None,"on: a local run of a Cargo project builds into a shared per-project target directory under PULSE_HOME/cache/cargo-target/<slug>/<lane> (one lane per run in flight, so concurrent runs never share one) instead of a cold target inside its throwaway clone, unless the project's recipe sets CARGO_TARGET_DIR itself; off: every clone builds from scratch"),
        "daemon.build_cache_keep_days"=>("count",Some(1),[],None,"days a shared build-cache lane may sit unused before the sweep removes it; lanes of projects the store no longer lists go at once, and dead units and stale incremental caches inside a lane are pruned like any shared Cargo target"),
        "hygiene.cargo_target"=>("choice",None,["on","off"],None,"on: the daemon sweep prunes dead or idle workspace-crate units, stale incremental caches and debug objects no binary's debug map names from shared Cargo target directories named by active projects' CARGO_TARGET_DIR verify commands, only under the project checkout or PULSE_HOME and only while verify/cargo locks are free; off: leave Cargo target directories alone"),
        "hygiene.cargo_target_hours"=>("count",Some(1),[],None,"hours an incremental cache directory, or a workspace crate's compiled unit, must sit unmodified before the hygiene sweep removes it from a shared Cargo target; registry and git dependencies are never pruned for idleness"),
        "landing.push"=>("choice",None,["off","main"],None,"main: after a landing the daemon pushes the project's default branch to its upstream, fast-forward only, with the git credentials this machine already has - or with push.credential when it is set - and pushes what landed earlier and was not pushed yet; a failed push shows in the landing line of pulse status and the hub. off: you push (ADR-037)"),
        "landing.auto_stash"=>("choice",None,["on","off"],None,"on: before landing, the daemon stashes uncommitted changes in the checkout with a clear message so the landing can proceed; off: a dirty checkout defers landing (ADR-024)"),
        "verify.lock"=>("choice",None,["off","project","machine"],None,"off: a worker commit, a finished run and a landing each verify whenever they are ready; project: verifications of one project take turns on a machine, so checkouts sharing one build cache (a Cargo target directory, a Gradle cache) never rewrite each other's artifacts mid-run; machine: every verification on the machine takes turns. While verifications take turns, a project runs only as many improve and task jobs at once as its measured check time fits into harness.step_timeout (its idle line names the lanes)"),
        "landing.base_red_retry_minutes"=>("count",Some(1),[],None,"minutes a project waits before the daemon re-checks landings after the default branch itself fails verify"),
        "landing.verify_memo"=>("choice",None,["on","off"],None,"on: skip landing/base verifies when this user recently verified the identical Git tree with the identical verify command; off restores the old always-verify behaviour"),
        "landing.verify_memo_hours"=>("count",Some(0),[],None,"hours a verified-tree memo remains valid for landing/base verify skips; default 24"),
        "heartbeat.dead_after"=>("count",Some(30),[],None,"seconds an agent may go without a heartbeat before it counts as dead: every pulse command run as an agent beats, so do its hooks, its job lease and the runner supervising its run; a dead worker's run is closed as lost and its job queued again, a dead session simply shows as dead until it beats again; never under 30"),
        "board.stale_days"=>("count",Some(1),[],None,"days before the daemon retires an open suggestion whose non-empty files list names only paths absent at the project HEAD"),
        "daemon.planner_cache_ttl_seconds"=>("count",Some(60),[],None,"seconds an unchanged planner/tally snapshot may stay in memory; watermarks invalidate it earlier when board, job, landing, record or setting state changes"),
        "daemon.restart_drain_minutes"=>("count",Some(0),[],None,"minutes a daemon whose binary changed on disk keeps dispatching and landing while runs are in flight before it stops starting runs and exits with code 3 after them; it exits as soon as nothing is in flight, stops dispatching once less than minutes_per_job + 5 of the window is left, and 0 stops dispatching at once (the old behaviour)"),
        "daemon.pass_seconds"=>("count",Some(0),[],None,"least seconds between two planning passes of the daemon loop (leads, tasks, amendments, recoveries, lessons, evolution, schedules, planner, idle notes, executors, maintenance); the ticks in between, every 15 seconds while runs are in flight and whenever a run ends or the bus wakes the loop, only reap, watch the stop switch and the restart drain, start approved compute and refill free slots; 0 plans on every tick, and a shorter daemon interval caps it"),
        "daemon.max_jobs"=>("count",Some(0),[],None,"local runs the daemon on this machine starts at once, whatever `workers` says (remote executors keep their own); 0 derives it from the hardware - half the physical cores, one run per 4 GB of RAM, never more than four - and `pulse daemon --max-jobs N` overrides both for one daemon"),
        "daemon.max_load"=>("count",Some(0),[],None,"one-minute load average over which the daemon leases no new local run; the runs in flight finish and the idle line says `machine saturated`; 0 uses the number of CPUs"),
        "daemon.min_free_mem_mb"=>("count",Some(0),[],None,"megabytes of memory a new run could use without swapping (free, inactive and speculative pages on macOS, MemAvailable on Linux) under which the daemon leases no new local run and starts no self-update build; 0 turns the floor off"),
        "daemon.nice"=>("count",Some(0),[],None,"nice level the daemon's own process runs at from startup (setpriority), under the owner's foreground work at 0 and over its job children at 10, so planning, sweeps and landing passes yield to what the owner is doing while the loop still comes before the fleet's builds; 0 leaves the level the service started it at"),
        "daemon.build_jobs"=>("count",Some(0),[],None,"parallel compile jobs a build the daemon causes may use: exported as CARGO_BUILD_JOBS to its job children, their pre-commit verification and the landing verification when the environment does not set it, and passed as -j to the self-update build; 0 is half the physical cores, so no single build takes the whole machine"),
        "push.credential"=>("reference",None,[],None,"where the daemon's own SSH push key lives, as a reference and never the key itself: op://VAULT/ITEM/FIELD?ssh-format=openssh (1Password, read with a service-account token kept by `pulse cred token`), keychain://SERVICE/ACCOUNT (a macOS Keychain item) or file:///ABSOLUTE/PATH (a 0600 file). The key exists on disk only for the seconds of a push; when it cannot be resolved the landing is held with the reason and the daemon never falls back to your credentials. Empty: push with what git already uses. Set it with `pulse cred set`, which probes the remote first (ADR-041)"),
    }
});
pub static LEAD_SETTINGS: LazyLock<BTreeMap<String, String>> = LazyLock::new(
    || map! {"lead.idle_hours"=>"6","lead.idle_max_jobs"=>"3","lead.stuck_thread_jobs"=>"3","lead.research_minutes"=>"60","lead.research_per_day"=>"12","lead.idle_factor"=>"3","lead.ballot_hours"=>"1","lead.ballot_cap"=>"10","plan.operating_layer_per_day"=>"3","plan.operating_layer_fill"=>"on","schedule.min_minutes"=>"30","schedule.max_per_project"=>"10","schedule.script_timeout_minutes"=>"20","schedule.scripts_per_tick"=>"2",},
);
pub static LEAD_RULES: LazyLock<BTreeMap<String, SettingRule>> = LazyLock::new(|| {
    rules! {
        "lead.idle_hours"=>("count",Some(0),[],None,"hours a project may sit with no job queued, leased or created before its lead is asked to find the next work by itself and tell the owner; research, ballot and reflect jobs the planner seeds on its own do not count as the project moving; 0 turns the standing brief off"),
        "lead.idle_max_jobs"=>("count",Some(1),[],None,"worker jobs one idle lead run may queue"),
        "lead.stuck_thread_jobs"=>("count",Some(0),[],None,"finished worker jobs one board thread may seed in a row (from its suggestions, handoffs and questions) that commit nothing - read-only triage, claims, questions - before the planner stops seeding that thread and asks the lead to decide it; a job from the thread that commits resets the count; 0 turns the cap off"),
        "lead.research_minutes"=>("count",Some(0),[],None,"minutes a project must sit with nothing queued or leased, no research job and no job that decided work (ballots and reflects do not count) created, before the planner seeds one research job (while the fleet is idle a free lane under parallel_per_project is enough: research then runs beside in-flight work): a worker investigates one focus in turn (the product as its user sees it, the code that changes most, the checks, the loop's report, the docs, the dependencies, the board, the memory) and ends with one to three experiment cards and a vote on every card below quorum; 0 turns research off (ADR-045)"),
        "lead.research_per_day"=>("count",Some(1),[],None,"research jobs one project may receive per UTC day"),
        "lead.idle_factor"=>("count",Some(1),[],None,"while fewer than half the local worker slots hold a lease and no job waits in the queue the fleet is idle: research waits lead.research_minutes divided by this (never below 10 minutes) with lead.research_per_day grown by it, a ballot job may come every lead.ballot_hours divided by this (never below 10 minutes), and a project's lead gets its standing brief after lead.idle_hours divided by this (never below 1 hour), so idle slots turn into cards, votes and queued work instead of waiting; 1 keeps the plain cadences"),
        "lead.ballot_hours"=>("count",Some(1),[],None,"hours a card may wait below the vote quorum before the planner seeds a ballot job, at most one per project per that many hours, and only when nothing else is queued"),
        "lead.ballot_cap"=>("count",Some(1),[],None,"maximum number of below-quorum cards a single ballot job asks a worker to vote on; closest-to-quorum cards are listed first"),
        "plan.operating_layer_per_day"=>("count",Some(0),[],None,"operating-layer improve jobs the planner may create from suggestion cards for one project per UTC day; product cards are unaffected and are planned first; 0 turns this cap off"),
        "plan.operating_layer_fill"=>("choice",None,["on","off"],None,"on: once the day's operating-layer cap is reached, voted operating-layer cards are still planned while no product card of the project passes the plan gates, so idle workers do the voted work instead of waiting for tomorrow; the cap holds again the moment a product card competes. off: the cap holds whatever competes"),
        "schedule.min_minutes"=>("count",Some(5),[],None,"the shortest cadence a schedule may have (`pulse schedule add --every`); a standing job that runs more often than this is a loop, not a schedule"),
        "schedule.max_per_project"=>("count",Some(0),[],None,"active or paused schedules one project may hold at once; 0 turns the cap off"),
        "schedule.script_timeout_minutes"=>("count",Some(1),[],None,"minutes a check or watch script (`pulse schedule add --kind check|watch --script`) may run before the daemon kills it and counts the run as a failure (a red check, a broken watcher)"),
        "schedule.scripts_per_tick"=>("count",Some(1),[],None,"check and watch scripts the daemon runs in one tick, the longest overdue first; the rest wait for the next tick, and none runs while the machine is saturated"),
    }
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
pub static OWNER_RULES: LazyLock<BTreeMap<String, SettingRule>> = LazyLock::new(|| {
    rules! {
        "owner.presence"=>("choice",None,["present","away"],None,"present: proposals, compute requests and owner questions wait for you; away: each project's lead decides the board's proposals on your behalf with its best judgement, recorded as the lead's decision and never as yours - hard stops, money (compute), Tier 2 decisions (ADRs), the constitution's protected sections and the kill switch still wait for you"),
    }
});
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
    // A long run of token characters is a credential unless the word it sits
    // in is a path or a URL. The board's daily vocabulary is full of long
    // repository-relative paths (`src/web-app/lib/use-context-intake`), so
    // the word is judged, not the whole sentence, and a word made of
    // slash-separated segments is a path wherever it starts - provided it
    // has three or more segments or a `-`, `_` or `.` in it, which a base64
    // string never has next to a `/`.
    static LONG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9+/_=-]{40,}").unwrap());
    static PATH_OR_URL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(/|\./|\.\./|~|\$|[A-Za-z][A-Za-z0-9+.-]*://[^@\s]*$)").unwrap()
    });
    static SEGMENTS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_.@-]+(/[A-Za-z0-9_.@-]+)+/?$").unwrap());
    if !LONG.is_match(value) {
        return false;
    }
    let path_like = |word: &str| {
        PATH_OR_URL.is_match(word)
            || (SEGMENTS.is_match(word)
                && (word.matches('/').count() >= 2 || word.contains(['-', '_', '.'])))
    };
    value.split_whitespace().any(|word| {
        let word = word.trim_matches(|c: char| {
            matches!(
                c,
                '`' | '\''
                    | '"'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '<'
                    | '>'
                    | ','
                    | ';'
                    | ':'
                    | '.'
            )
        });
        LONG.is_match(word) && !path_like(word)
    })
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
            if name == "evolve.flat_cycles" && n == 1 {
                return Err(input_err(
                    "evolve.flat_cycles is a streak of at least 2 tied cycles, or 0 to turn the gate off; not 1",
                ));
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
        let _serial = crate::test_helpers::serial();
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

    #[test]
    fn secret_shape_spares_paths_and_keeps_tokens() {
        // Repository-relative paths, alone or inside a sentence, are the
        // board's vocabulary, not credentials - however long they run.
        for text in [
            "src/web-fachsprachtrainer/lib/use-context-intake",
            "Infrastructure/Voice/Realtime/AzureRealtimeApiClient",
            "File: src/web-fachsprachtrainer/components/user-menu.tsx (the menu)",
            "parity of `regions/redemittel/minutes/dashboard/recall-cards` with the FSP shim",
            "/usr/local/bin:/opt/homebrew/bin/some/very/long/path/that/keeps/going/on",
            "https://example.test/a/very/long/path/that/keeps/going/on/and/on?x=1",
        ] {
            assert!(!secret_shaped(text), "{text}");
        }
        // Long opaque tokens stay credentials, in a sentence or next to a slash.
        for text in [
            "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789AbCdEfGh",
            "token AbCdEfGhIjKlMnOpQrStUvWxYz0123456789AbCdEfGh= set",
            "AbCdEfGhIjKlMnOpQrSt/UvWxYz0123456789AbCdEfGh",
            "-----BEGIN RSA PRIVATE KEY-----",
            "https://user:pass@example.test/",
        ] {
            assert!(secret_shaped(text), "{text}");
        }
    }
}

#[cfg(test)]
mod go_settings_tests {
    use super::*;
    use crate::{Options, Store};
    use std::fs;
    use std::sync::Mutex;
    use tempfile::TempDir;

    static TEST_LOCK: Mutex<()> = Mutex::new(());
    fn set_job_empty() -> std::sync::MutexGuard<'static, ()> {
        let guard = TEST_LOCK.lock().unwrap();
        unsafe { std::env::set_var("PULSE_JOB", "") };
        guard
    }
    struct TestStore {
        _dir: TempDir,
        store: Store,
    }
    impl std::ops::Deref for TestStore {
        type Target = Store;
        fn deref(&self) -> &Self::Target {
            &self.store
        }
    }
    fn store(now: &str) -> TestStore {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(Options {
            home: dir.path().display().to_string(),
            create: true,
            now: now.into(),
            ..Options::default()
        })
        .unwrap();
        TestStore { _dir: dir, store }
    }

    #[test]
    fn decimal_setting() {
        let _serial = crate::test_helpers::serial();
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
        let _serial = crate::test_helpers::serial();
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
    fn flat_cycles_setting() {
        let _serial = crate::test_helpers::serial();
        let _guard = set_job_empty();
        let s = store("2026-09-08T10:00:00Z");
        assert_eq!(s.setting("evolve.flat_cycles", ""), "6");
        assert_eq!(s.setting_int("evolve.flat_cycles"), 6);
        for (input, want) in [("0", "0"), ("2", "2"), (" 12 ", "12")] {
            s.set_setting("human", "evolve.flat_cycles", input).unwrap();
            assert_eq!(s.setting("evolve.flat_cycles", ""), want);
        }
        for (input, message) in [
            (
                "1",
                "at least 2 tied cycles, or 0 to turn the gate off; not 1",
            ),
            ("-1", "evolve.flat_cycles must be at least 0, not -1"),
            ("six", "evolve.flat_cycles takes a whole number"),
            ("", "evolve.flat_cycles takes a whole number"),
        ] {
            let err = s
                .set_setting("human", "evolve.flat_cycles", input)
                .unwrap_err()
                .to_string();
            assert!(err.contains(message), "{input:?}: {err}");
        }
        assert_eq!(s.setting("evolve.flat_cycles", ""), "12");
        let rule = &EVOLVE_RULES["evolve.flat_cycles"];
        assert_eq!(rule.kind, "count");
        assert!(
            rule.description.contains("0 turns the gate off")
                && rule.description.contains("evolution/loop-gate"),
            "{}",
            rule.description
        );
    }

    #[test]
    fn daemon_self_update_settings() {
        let _serial = crate::test_helpers::serial();
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
        assert_eq!(s.setting("landing.base_red_retry_minutes", ""), "10");
        assert!(
            s.set_setting("human", "landing.base_red_retry_minutes", "0")
                .is_err()
        );
        s.set_setting("human", "landing.base_red_retry_minutes", "3")
            .unwrap();
        assert_eq!(s.setting("landing.base_red_retry_minutes", ""), "3");
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
        let _serial = crate::test_helpers::serial();
        let _guard = set_job_empty();
        let s = store("2026-09-08T10:00:00Z");
        for (key, want) in [
            ("daemon.sweep", "on"),
            ("daemon.sweep_grace_minutes", "30"),
            ("daemon.sweep_branch_days", "7"),
            ("daemon.sweep_keep_days", "30"),
            ("daemon.transcript_keep_days", "14"),
            ("daemon.log_keep_days", "14"),
            ("daemon.log_max_mb", "32"),
            ("daemon.vacuum_free_mb", "256"),
            ("daemon.build_cache", "on"),
            ("daemon.build_cache_keep_days", "7"),
        ] {
            assert_eq!(s.setting(key, ""), want);
            assert!(extra_setting_names().contains(&key.to_string()));
        }
        for key in [
            "daemon.transcript_keep_days",
            "daemon.log_max_mb",
            "daemon.vacuum_free_mb",
        ] {
            s.set_setting("human", key, "0").unwrap();
            assert!(s.set_setting("human", key, "-1").is_err());
        }
        for key in ["daemon.log_keep_days", "daemon.build_cache_keep_days"] {
            assert!(s.set_setting("human", key, "0").is_err());
            s.set_setting("human", key, "3").unwrap();
        }
        assert!(
            s.set_setting("human", "daemon.build_cache", "maybe")
                .is_err()
        );
        s.set_setting("human", "daemon.build_cache", "off").unwrap();
        assert_eq!(s.setting("daemon.build_cache", ""), "off");
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
        let _serial = crate::test_helpers::serial();
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
            &names[lead_start..lead_start + 4],
            [
                "lead.ballot_cap",
                "lead.ballot_hours",
                "lead.idle_factor",
                "lead.idle_hours"
            ]
        );
        assert_eq!(s.setting("baseline.retry_hours", ""), "6");
        assert_eq!(s.setting("lane.retry_minutes", ""), "20");
        assert_eq!(s.setting("lane.short_retry_minutes", ""), "2");
        for key in [
            "baseline.retry_hours",
            "lane.retry_minutes",
            "lane.short_retry_minutes",
        ] {
            assert!(s.set_setting("human", key, "0").is_err());
        }
        s.set_setting("human", "lane.retry_minutes", "7").unwrap();
        s.set_setting("human", "lane.short_retry_minutes", "3")
            .unwrap();
        s.set_setting("human", "baseline.retry_hours", "2").unwrap();
        assert_eq!(s.setting("lane.retry_minutes", ""), "7");
        assert_eq!(s.setting("lane.short_retry_minutes", ""), "3");
        assert_eq!(s.setting("baseline.retry_hours", ""), "2");
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
        let _serial = crate::test_helpers::serial();
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
        let _serial = crate::test_helpers::serial();
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
        let _serial = crate::test_helpers::serial();
        let Ok(raw) = fs::read_to_string("web/hub/mock/api/settings.json") else {
            return;
        };
        let doc: serde_json::Value = serde_json::from_str(&raw).unwrap_or_else(|e| {
            panic!(
                "{} {} {} {}: {e}",
                "json:\"settings\"", "json:\"name\"", "json:\"kind\"", "json:\"default\""
            )
        });
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
            for name in sorted_keys(settings) {
                let got = mock
                    .get(&name)
                    .unwrap_or_else(|| panic!("mock lacks {name} setting {name}"));
                let want_kind = &rules[&name].kind;
                if !want_kind.is_empty() {
                    assert_eq!(&got.0, want_kind, "{name}");
                }
                assert_eq!(&got.1, &settings[&name], "{name}");
            }
        }
        let mut known = BTreeMap::new();
        for name in DEFAULT_SETTINGS.keys() {
            known.insert(name.clone(), true);
        }
        for name in extra_setting_names() {
            known.insert(name, true);
        }
        for name in mock.keys() {
            assert!(
                known.contains_key(name),
                "mock lists {name} which the store does not know"
            );
        }
    }
}
