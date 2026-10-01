use regex::Regex;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::card_class::{CARD_CLASS_OPERATING_LAYER, card_class};
use crate::plan_context::{PlanContext, duplicate_gate, operating_layer_cap_gate};
use crate::tally::{Post, Suggestion};

static SIGNAL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)^\s*(?:-\s*)?`?signal`?\s*:\s*(.+?)\s*$|\bsignal:\s*(\S+)").unwrap()
});

pub const OWNER_PATHS: &[&str] = pulse_store::OWNER_PATHS;

pub const GATE_NET_VOTES: &str = "net votes <= 0: the planner takes score > 0 only";
pub const GATE_OWNER: &str = "addressed to the owner; not a worker job";
pub const GATE_NO_EVIDENCE: &str =
    "without evidence: no Signal: line names an existing file, post, job, record or check";
pub const GATE_OWNER_PATHS: &str =
    "points at owner-only paths (constitution, evaluators, decisions, .github)";
pub const GATE_UNLANDED: &str =
    "Signal path is absent from the default branch (only on an unlanded branch); waits for landing";
pub const GATE_DUPLICATE: &str = "duplicate of";
pub const GATE_OPERATING_LAYER_CAP: &str = "operating-layer cap";
pub const GATE_NO_OP_CHECK: &str =
    "check selects no test: its `go test -run` regex matches nothing in `go test -list`";
pub const GATE_FILE_GONE: &str = "File: names a path whose whole tree is gone from the checkout; the card predates a move or removal";

pub fn signal_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let split = Regex::new(r"[\s,;]+").unwrap();
    for m in SIGNAL_RE.captures_iter(text) {
        let raw = m.get(1).or_else(|| m.get(2)).map_or("", |m| m.as_str());
        let raw = raw
            .trim()
            .trim_matches(['`', '\'', '"', '(', ')', '[', ']', ',', ';']);
        for tok in split.split(raw) {
            // A trailing dot is punctuation; a leading one is a dotfile (`.nvmrc`,
            // `.github/workflows/ci.yml`) and stays.
            let tok = tok
                .trim_matches(['`', '\'', '"', '(', ')', '[', ']', ',', ';'])
                .trim_end_matches('.');
            if !tok.is_empty() {
                out.push(tok.to_string());
            }
        }
    }
    out
}

/// Prefixes a Signal token may carry in front of a store id (`post:2026...`, `job:job-...`).
const STORE_ID_PREFIXES: &[&str] = &["post:", "job:", "run:", "record:", "memory:"];

/// Whether `tok` names a row the store holds: a post, a job, a run or a record (an ADR, a
/// memory entry such as `MEM-...`). Such evidence outlives the run that produced it, unlike a
/// log under /tmp or a worker clone, and the gate accepts it as it accepts an existing file.
pub fn store_evidence(s: &pulse_store::Store, tok: &str) -> bool {
    let bare = STORE_ID_PREFIXES
        .iter()
        .find_map(|p| tok.strip_prefix(p))
        .unwrap_or(tok);
    if bare.is_empty() {
        return false;
    }
    let Ok(conn) = s.conn() else {
        return false;
    };
    ["posts", "jobs", "runs", "records"].iter().any(|table| {
        conn.query_row(
            &format!("SELECT 1 FROM {table} WHERE id = ?"),
            [bare],
            |_| Ok(()),
        )
        .is_ok()
    })
}

fn rel_and_symbol(tok: &str) -> (String, String) {
    let rel = tok.split(':').next().unwrap_or("");
    let (rel, sym) = rel.split_once('#').unwrap_or((rel, ""));
    (rel.into(), sym.into())
}

fn rel_of_token(tok: &str) -> String {
    rel_and_symbol(tok).0
}

pub fn signal_resolves(
    s: &pulse_store::Store,
    project_path: &str,
    summary: &str,
    body: &str,
) -> bool {
    let text = format!("{summary}\n{body}");
    for tok in signal_tokens(&text) {
        if tok.to_ascii_lowercase().starts_with("check:") && tok.len() > "check:".len() {
            return true;
        }
        if store_evidence(s, &tok) {
            return true;
        }
        let rel = rel_of_token(&tok);
        if project_path.is_empty() || rel.is_empty() || Path::new(&rel).is_absolute() {
            continue;
        }
        let clean = clean_rel(&rel);
        if clean.is_empty() || clean == "." || clean.split('/').any(|p| p == "..") {
            continue;
        }
        if Path::new(project_path).join(&clean).exists() {
            return true;
        }
    }
    false
}

fn signal_paths(project_path: &str, summary: &str, body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for tok in signal_tokens(&format!("{summary}\n{body}")) {
        let rel = rel_of_token(&tok);
        if project_path.is_empty() || rel.is_empty() || Path::new(&rel).is_absolute() {
            continue;
        }
        let clean = clean_rel(&rel);
        if clean.is_empty() || clean == "." || clean.split('/').any(|p| p == "..") {
            continue;
        }
        if Path::new(project_path).join(&clean).exists() {
            out.push(clean);
        }
    }
    out
}

pub fn signal_unlanded(
    s: &pulse_store::Store,
    project_path: &str,
    summary: &str,
    body: &str,
) -> bool {
    if project_path.is_empty() {
        return false;
    }
    for tok in signal_tokens(&format!("{summary}\n{body}")) {
        if tok.to_ascii_lowercase().starts_with("check:") && tok.len() > "check:".len() {
            return false;
        }
        if store_evidence(s, &tok) {
            return false;
        }
    }
    let paths = signal_paths(project_path, summary, body);
    if paths.is_empty() {
        return false;
    }
    let branch: String = match s.conn().ok().and_then(|conn| {
        conn.query_row(
            "SELECT default_branch FROM projects WHERE path = ?",
            [project_path],
            |r| r.get::<_, String>(0),
        )
        .ok()
    }) {
        Some(b) if !b.is_empty() => b,
        _ => return false,
    };
    let Some(head) = default_branch_head(project_path, &branch) else {
        return false;
    };
    for p in paths {
        if !landed_path_exists(project_path, &head, &p) {
            continue;
        }
        if let Some(sym) = signal_symbol(&format!("{summary}\n{body}"), &p)
            && !landed_symbol_exists(project_path, &head, &p, &sym)
        {
            continue;
        }
        return false;
    }
    true
}

fn signal_symbol(text: &str, p: &str) -> Option<String> {
    for tok in signal_tokens(text) {
        let (rel, sym) = rel_and_symbol(&tok);
        if !sym.is_empty() && clean_rel(&rel) == p {
            return Some(sym);
        }
    }
    None
}

#[derive(Clone)]
struct Head {
    commit: String,
    ok: bool,
    at: Instant,
}
type LandedCache = (BTreeMap<String, Head>, BTreeMap<String, bool>);
static LANDED_CACHE: LazyLock<Mutex<LandedCache>> =
    LazyLock::new(|| Mutex::new((BTreeMap::new(), BTreeMap::new())));
const LANDED_HEAD_TTL: Duration = Duration::from_secs(5);
const LANDED_EXISTS_CAP: usize = 4096;

pub fn reset_landed_cache() {
    let mut c = LANDED_CACHE.lock().expect("landed cache poisoned");
    c.0.clear();
    c.1.clear();
}

#[cfg(test)]
pub(crate) fn expire_landed_head_cache_for_test() {
    let mut c = LANDED_CACHE.lock().expect("landed cache poisoned");
    for h in c.0.values_mut() {
        h.at -= LANDED_HEAD_TTL + Duration::from_secs(1);
    }
}

fn default_branch_head(project_path: &str, branch: &str) -> Option<String> {
    let key = format!("{project_path}\0{branch}");
    let now = Instant::now();
    {
        let c = LANDED_CACHE.lock().expect("landed cache poisoned");
        if let Some(h) = c.0.get(&key)
            && now.duration_since(h.at) < LANDED_HEAD_TTL
        {
            return h.ok.then(|| h.commit.clone());
        }
    }
    let out = landed_git(
        project_path,
        [
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{branch}^{{commit}}"),
        ],
    )
    .output();
    let (ok, commit) = match out {
        Ok(out) if out.status.success() && !out.stdout.is_empty() => (
            true,
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        ),
        _ => (false, String::new()),
    };
    LANDED_CACHE
        .lock()
        .expect("landed cache poisoned")
        .0
        .insert(
            key,
            Head {
                commit: commit.clone(),
                ok,
                at: now,
            },
        );
    if !ok {
        crate::log_line(format!(
            "tally: {project_path}: default branch {branch:?} does not resolve to a commit; unlanded-Signal hold is inert (not falling back to HEAD)"
        ));
    }
    ok.then_some(commit)
}

fn landed_git<const N: usize>(project_path: &str, args: [&str; N]) -> Command {
    let mut cmd = Command::new(pulse_gitenv::git_program());
    cmd.arg("-C").arg(project_path).args(args);
    cmd.stderr(Stdio::null());
    cmd.env_clear();
    for kv in pulse_gitenv::env() {
        if let Some((k, v)) = kv.split_once('=') {
            cmd.env(k, v);
        }
    }
    cmd
}

fn landed_path_exists(project_path: &str, commit: &str, p: &str) -> bool {
    let key = format!("{project_path}\0{commit}\0{p}");
    {
        let c = LANDED_CACHE.lock().expect("landed cache poisoned");
        if let Some(v) = c.1.get(&key) {
            return *v;
        }
    }
    let spec = format!("{commit}:{}", p.replace('\\', "/"));
    let v = landed_git(project_path, ["cat-file", "-e", &spec])
        .status()
        .is_ok_and(|s| s.success());
    let mut c = LANDED_CACHE.lock().expect("landed cache poisoned");
    if c.1.len() >= LANDED_EXISTS_CAP {
        c.1.clear();
    }
    c.1.insert(key, v);
    v
}

fn landed_symbol_exists(project_path: &str, commit: &str, p: &str, sym: &str) -> bool {
    let key = format!("{project_path}\0{commit}\0{p}#{sym}");
    {
        let c = LANDED_CACHE.lock().expect("landed cache poisoned");
        if let Some(v) = c.1.get(&key) {
            return *v;
        }
    }
    let spec = format!("{commit}:{}", p.replace('\\', "/"));
    let out = landed_git(project_path, ["show", &spec]).output();
    let re = Regex::new(&format!(
        r"(^|[^A-Za-z0-9_]){}([^A-Za-z0-9_]|$)",
        regex::escape(sym)
    ))
    .unwrap();
    let v = out.is_err() || re.is_match(&String::from_utf8_lossy(&out.unwrap().stdout));
    let mut c = LANDED_CACHE.lock().expect("landed cache poisoned");
    if c.1.len() >= LANDED_EXISTS_CAP {
        c.1.clear();
    }
    c.1.insert(key, v);
    v
}

pub fn is_worker_post(s: &pulse_store::Store, agent: &str) -> bool {
    s.conn()
        .ok()
        .and_then(|conn| {
            conn.query_row("SELECT kind FROM agents WHERE name = ?", [agent], |r| {
                r.get::<_, String>(0)
            })
            .ok()
        })
        .is_some_and(|kind| kind == "worker")
}

pub fn owner_targeted(summary: &str, body: &str) -> bool {
    for tok in signal_tokens(&format!("{summary}\n{body}")) {
        let rel = rel_of_token(&tok).trim_start_matches('/').to_string();
        for p in OWNER_PATHS {
            if rel == *p || rel.starts_with(p) {
                return true;
            }
        }
    }
    false
}

/// Whether the card's `File:` line names only paths that are gone from the checkout
/// together with their directories. A card that names a file to create keeps its directory
/// and passes; a card written against a tree that was since moved or removed (a port, a
/// rename of a whole package) does not, and no worker should be sent to look for it.
/// The card's `File:` (or `Path:`) text as written, backticks and quotes
/// stripped; empty when the card names none.
pub fn card_file_text(body: &str) -> String {
    let fields = crate::card_class::card_fields(body);
    crate::card_class::first_card_text(&[
        fields.get("file").cloned().unwrap_or_default(),
        fields.get("path").cloned().unwrap_or_default(),
    ])
    .trim()
    .trim_matches(['`', '\'', '"'])
    .to_string()
}

pub fn card_file_gone(project_path: &str, body: &str) -> bool {
    if project_path.is_empty() {
        return false;
    }
    let fields = crate::card_class::card_fields(body);
    let value = crate::card_class::first_card_text(&[
        fields.get("file").cloned().unwrap_or_default(),
        fields.get("path").cloned().unwrap_or_default(),
    ]);
    let mut concrete = 0;
    for part in value.split(',') {
        let raw = part
            .trim()
            .trim_matches(['`', '\'', '"', '(', ')', '[', ']'])
            .trim_end_matches(['.', ':', ';']);
        // A file class ("the three git callers", "src/**/*.rs") is not a path to look up.
        if raw.is_empty()
            || raw.contains(char::is_whitespace)
            || raw.contains(['*', '?', '{', '}'])
            || Path::new(raw).is_absolute()
        {
            continue;
        }
        let clean = clean_rel(raw);
        if clean.is_empty() || clean == "." || clean.split('/').any(|p| p == "..") {
            continue;
        }
        concrete += 1;
        let full = Path::new(project_path).join(&clean);
        if full.exists() {
            return false;
        }
        // A new file may go anywhere under a tree that exists, however deep; only a
        // path whose whole top-level tree is missing predates a move or removal.
        let Some((top, _)) = clean.split_once('/') else {
            return false;
        };
        if Path::new(project_path).join(top).exists() {
            return false;
        }
    }
    concrete > 0
}

pub fn in_flight_gate(sg: &Suggestion) -> String {
    if sg.job.is_empty() || sg.job_status.is_empty() {
        return String::new();
    }
    match sg.job_status.as_str() {
        "queued" | "leased" | "blocked" => format!("{} as job {}", sg.job_status, sg.job),
        _ => String::new(),
    }
}

pub fn plan_gate(
    s: &pulse_store::Store,
    project_path: &str,
    sg: &Suggestion,
    post: Option<&Post>,
) -> String {
    plan_gate_with(s, PlanContext::default(), project_path, sg, post)
}

pub fn plan_gate_with(
    s: &pulse_store::Store,
    ctx: PlanContext,
    project_path: &str,
    sg: &Suggestion,
    post: Option<&Post>,
) -> String {
    let gate = in_flight_gate(sg);
    if !gate.is_empty() {
        return gate;
    }
    if let Some(dup) = ctx.duplicates.get(&sg.id)
        && !dup.is_empty()
    {
        return duplicate_gate(dup);
    }
    if sg.score <= 0 {
        return GATE_NET_VOTES.into();
    }
    let mut summary = sg.summary.as_str();
    let mut body = "";
    let mut agent = sg.agent.as_str();
    if let Some(post) = post {
        if post.for_owner {
            return GATE_OWNER.into();
        }
        if !post.summary.is_empty() {
            summary = &post.summary;
        }
        body = &post.body;
        agent = &post.agent;
    }
    if card_file_gone(project_path, body) {
        return GATE_FILE_GONE.into();
    }
    if post.is_some() && is_worker_post(s, agent) {
        if !signal_resolves(s, project_path, summary, body) {
            return GATE_NO_EVIDENCE.into();
        }
        if owner_targeted(summary, body) {
            return GATE_OWNER_PATHS.into();
        }
        if signal_unlanded(s, project_path, summary, body) {
            return GATE_UNLANDED.into();
        }
        if run_check_is_no_op(project_path, summary, body) {
            return GATE_NO_OP_CHECK.into();
        }
    }
    if ctx.operating_layer_cap_set
        && ctx.operating_layer_today >= ctx.operating_layer_cap
        && card_class(summary, body) == CARD_CLASS_OPERATING_LAYER
    {
        return operating_layer_cap_gate(ctx.operating_layer_today, ctx.operating_layer_cap);
    }
    String::new()
}

static RUN_CHECK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"go test\s+((?:[^\s'"`]+\s+)*?)-run[= ]+(?:'([^']*)'|"([^"]*)"|`([^`]*)`|([^'"`\s]+))((?:[ \t]+[^\s'"`]+)*)"#).unwrap()
});

#[cfg(test)]
type ListTestsHook = fn(&str, &[String], &str) -> std::io::Result<Vec<String>>;

#[cfg(test)]
static LIST_TESTS_HOOK: LazyLock<Mutex<Option<ListTestsHook>>> = LazyLock::new(|| Mutex::new(None));

#[cfg(test)]
pub(crate) fn set_list_tests_hook(hook: Option<ListTestsHook>) {
    *LIST_TESTS_HOOK.lock().expect("list tests hook poisoned") = hook;
}

fn run_check_regex(caps: &regex::Captures<'_>) -> String {
    (2..=5)
        .find_map(|i| {
            caps.get(i)
                .map(|m| m.as_str().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_default()
}

fn list_tests(dir: &str, pkgs: &[String], regex: &str) -> std::io::Result<Vec<String>> {
    #[cfg(test)]
    if let Some(hook) = *LIST_TESTS_HOOK.lock().expect("list tests hook poisoned") {
        return hook(dir, pkgs, regex);
    }
    let out = Command::new("go")
        .args(["test"])
        .args(pkgs)
        .args(["-list", regex])
        .current_dir(dir)
        .output()?;
    if !out.status.success() {
        return Err(std::io::Error::other("go test -list failed"));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !line.starts_with("ok ")
                && !line.starts_with('?')
                && !line.contains(' ')
        })
        .map(str::to_string)
        .collect())
}

pub fn run_check_is_no_op(project_path: &str, summary: &str, body: &str) -> bool {
    if project_path.is_empty() {
        return false;
    }
    for m in RUN_CHECK_RE.captures_iter(&format!("{summary}\n{body}")) {
        let re = run_check_regex(&m);
        let mut pkgs: Vec<String> = Vec::new();
        for tok in m.get(1).map_or("", |m| m.as_str()).split_whitespace() {
            if tok.starts_with('-') || (tok.contains("..") && !tok.ends_with("...")) {
                pkgs.clear();
                break;
            }
            pkgs.push(tok.into());
        }
        for tok in m.get(6).map_or("", |m| m.as_str()).split_whitespace() {
            if tok.starts_with('-') {
                continue;
            }
            if !tok.starts_with("./") && !tok.starts_with("...") {
                break;
            }
            if tok.contains("..") && !tok.ends_with("...") {
                break;
            }
            pkgs.push(tok.trim_end_matches(['.', ',', ';']).into());
        }
        if pkgs.is_empty() || re.is_empty() {
            continue;
        }
        if Regex::new(&re).is_err() {
            return true;
        }
        match list_tests(project_path, &pkgs, &re) {
            Ok(names) if names.is_empty() => return true,
            _ => {}
        }
    }
    false
}

fn clean_rel(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let normalized = p.replace('\\', "/");
    for part in normalized.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|x| *x != "..") {
                    parts.pop();
                } else {
                    parts.push("..");
                }
            }
            _ => parts.push(part),
        }
    }
    parts.join("/")
}
