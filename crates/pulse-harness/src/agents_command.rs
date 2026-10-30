//! `pulse agents list|lint|schema`: the custom agent definitions - the
//! `.pulse/agents` files, their Claude and Copilot twins, the owner's
//! `~/.pulse/agents` and the store's `agent` records. `list` shows what
//! loads and which layer owns each name, `lint` reads every file one by
//! one and names what is wrong, `schema` prints the front matter's JSON
//! Schema. The command word `agents` belongs to the inspect crate (a bare
//! `pulse agents` prints the scorecards of the running agents); it hands
//! these three words here. `pulse agent` (singular) manages the workers.
use crate::agents::{
    AGENT_BODY_LIMIT, AgentDef, AgentLayer, DESCRIPTION_LIMIT, agent_files, agent_schema,
    load_agent_specs, merge_specs, model_allowed, read_agent_file, read_agent_layers,
};
use crate::harness::{PromptData, normalize_prompt_data, render_prompt};
use crate::skills::load_skills;
use anyhow::Result;
use pulse_cli::{Context, FlagSet, errorf, table, write_json};
use pulse_store::{STORE_NAME, Store};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// The usage lines of the three words, for the command's help.
pub const USAGE: &str = "pulse agents list [--project SLUG] [--cwd DIR] [--json]\n       pulse agents lint [PATH...] [--project SLUG] [--cwd DIR] [--json]\n       pulse agents schema\n       (`pulse agent` manages the running workers; `pulse agents` their definitions)";

/// The words of `pulse agents` that mean the definitions.
pub const SUBCOMMANDS: [&str; 3] = ["list", "lint", "schema"];

pub fn is_subcommand(word: &str) -> bool {
    SUBCOMMANDS.contains(&word)
}

/// The flags of the three words (the command adds its own).
pub fn flags(fs: &mut FlagSet) {
    fs.string_var(
        "cwd",
        "",
        "the checkout whose .pulse/agents, .claude/agents and .github/agents to read (default: the --project checkout the store knows, else the current directory)",
    );
}

/// Runs `pulse agents list|lint|schema`; `args[0]` is the word.
pub fn run(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let sub = args.first().map(String::as_str).unwrap_or("");
    if sub == "schema" {
        return write_json(&mut c.out, &agent_schema());
    }
    let store = open_store(c)?;
    let cwd = checkout(c, fs, store.as_ref());
    match sub {
        "list" => list(c, store.as_ref(), &cwd),
        "lint" => lint(c, store.as_ref(), &cwd, &args[1..]),
        other => Err(errorf(
            2,
            &format!("pulse agents: unknown subcommand {other:?}"),
            "",
            "pulse agents list | lint [PATH...] | schema",
        )
        .into()),
    }
}

/// The checkout whose files are read: `--cwd`, else the `--project`
/// checkout the store knows, else the current directory.
fn checkout(c: &Context, fs: &FlagSet, store: Option<&Store>) -> String {
    let given = fs.get_string("cwd");
    if !given.is_empty() {
        return given;
    }
    if !c.project.is_empty()
        && let Some(s) = store
        && let Ok(Some(p)) = s.project(&c.project)
        && !p.path.is_empty()
    {
        return p.path;
    }
    String::new()
}

/// The store at the PULSE home, read-only, when there is one; a
/// checkout without a store still gets its files read.
fn open_store(c: &Context) -> Result<Option<Store>> {
    if let Some(s) = &c.store {
        return Ok(Some(s.clone()));
    }
    if !Path::new(&c.home).join(STORE_NAME).exists() {
        return Ok(None);
    }
    Ok(Some(Store::open(pulse_store::Options {
        home: c.home.clone(),
        read_only: true,
        skip_records: true,
        now: c.now.clone(),
        ..pulse_store::Options::default()
    })?))
}

#[derive(Serialize)]
struct ListedAgent<'a> {
    name: &'a str,
    layer: &'static str,
    source: &'a str,
    description: &'a str,
    model: &'a str,
    kinds: &'a [String],
    select_tags: &'a [String],
    tools: Option<&'a [String]>,
    read_only: bool,
    max_steps: Option<u32>,
    warnings: usize,
    errors: usize,
}

#[derive(Serialize)]
struct RejectedAgent<'a> {
    name: &'a str,
    layer: &'static str,
    source: &'a str,
    reason: &'a str,
}

#[derive(Serialize)]
struct ListOut<'a> {
    project: &'a str,
    agents: Vec<ListedAgent<'a>>,
    rejected: Vec<RejectedAgent<'a>>,
}

fn list(c: &mut Context, store: Option<&Store>, cwd: &str) -> Result<()> {
    let project = c.project.clone();
    let specs = load_agent_specs(cwd, &c.home, store, &project);
    let (loaded, rejected): (Vec<&AgentDef>, Vec<&AgentDef>) =
        specs.iter().partition(|a| a.loads());
    if c.json {
        let out = ListOut {
            project: &project,
            agents: loaded
                .iter()
                .map(|a| ListedAgent {
                    name: &a.name,
                    layer: a.layer.label(),
                    source: &a.source,
                    description: &a.description,
                    model: a.model.as_deref().unwrap_or(""),
                    kinds: &a.kinds,
                    select_tags: &a.select_tags,
                    tools: a.tools.as_deref(),
                    read_only: a.read_only,
                    max_steps: a.max_steps,
                    warnings: a.warnings.len(),
                    errors: a.errors.len(),
                })
                .collect(),
            rejected: rejected
                .iter()
                .map(|a| RejectedAgent {
                    name: &a.name,
                    layer: a.layer.label(),
                    source: &a.source,
                    reason: a.rejected.as_deref().unwrap_or(""),
                })
                .collect(),
        };
        return write_json(&mut c.out, &out);
    }
    if loaded.is_empty() && rejected.is_empty() {
        c.println(format!(
            "no custom agents (the agents are {}, and the store's agent records)",
            AgentLayer::FILE_LAYERS
                .iter()
                .map(|l| format!("{}/*.md", l.label()))
                .collect::<Vec<_>>()
                .join(", ")
        ))?;
        return Ok(());
    }
    let rows: Vec<Vec<String>> = loaded
        .iter()
        .map(|a| {
            vec![
                a.name.clone(),
                a.layer.label().to_string(),
                a.source.clone(),
                a.model.clone().unwrap_or_else(|| "-".into()),
                if a.kinds.is_empty() {
                    "-".to_string()
                } else {
                    a.kinds.join(",")
                },
                a.warnings.len().to_string(),
            ]
        })
        .collect();
    if !rows.is_empty() {
        table(
            &mut c.out,
            c.color,
            Some(&["NAME", "LAYER", "SOURCE", "MODEL", "KINDS", "WARNINGS"]),
            &rows,
        )?;
    }
    for a in rejected {
        c.println(format!(
            "not loaded: {}: {}",
            a.source,
            a.rejected.as_deref().unwrap_or("")
        ))?;
    }
    Ok(())
}

/// One lint finding against one file or record.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Finding {
    pub level: &'static str,
    pub source: String,
    pub message: String,
}

#[derive(Serialize)]
struct LintedFile<'a> {
    source: &'a str,
    name: &'a str,
    layer: &'static str,
    loads: bool,
    findings: Vec<&'a Finding>,
}

#[derive(Serialize)]
struct LintOut<'a> {
    files: Vec<LintedFile<'a>>,
    errors: usize,
    warnings: usize,
}

/// What the lint needs to resolve names against: the skills in reach and
/// the store's MCP servers and model allow-list, when there is a store.
pub struct LintScope {
    skills: Vec<String>,
    mcp_servers: Option<Vec<String>>,
    models_allowed: String,
}

impl LintScope {
    fn read(cwd: &str, home: &str, store: Option<&Store>, project: &str) -> Self {
        let mut skills: Vec<String> = load_skills(cwd, home).into_iter().map(|k| k.name).collect();
        let mut mcp_servers = None;
        let mut models_allowed = String::new();
        if let Some(s) = store {
            if let Ok(more) = s.skills_for(project) {
                skills.extend(more.into_iter().map(|k| k.name));
            }
            if let Ok(servers) = s.mcp_servers(project) {
                mcp_servers = Some(servers.into_iter().map(|m| m.name).collect());
            }
            models_allowed = s.setting("models.allowed", "");
        }
        Self {
            skills,
            mcp_servers,
            models_allowed,
        }
    }
}

/// The findings of one spec as read: its rejection and lint errors, its
/// warnings, what each foreign tool name became, and the checks the
/// reader does not make (the body compiles as a prompt template, the
/// model is allowed, the skills and MCP servers it names exist).
pub fn lint_spec(a: &AgentDef, scope: &LintScope) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut add = |level: &'static str, message: String| {
        out.push(Finding {
            level,
            source: a.source.clone(),
            message,
        });
    };
    if let Some(why) = &a.rejected {
        add("error", format!("does not load: {why}"));
    }
    for e in &a.errors {
        add("error", e.clone());
    }
    if let Err(e) = render_prompt(&a.body, normalize_prompt_data(PromptData::default())) {
        add("error", format!("body does not render as a prompt: {e}"));
    }
    for w in &a.warnings {
        add("warning", w.clone());
    }
    if let Some(model) = &a.model
        && !scope.models_allowed.trim().is_empty()
        && !model_allowed(&scope.models_allowed, model)
    {
        add(
            "warning",
            format!(
                "model {model} is not in models.allowed ({}); a run keeps the class model",
                scope.models_allowed.trim()
            ),
        );
    }
    for k in &a.skills {
        if !scope.skills.iter().any(|have| have == k) {
            add(
                "warning",
                format!(
                    "skill `{k}` is not among the skills of the checkout, ~/.pulse/skills or the store"
                ),
            );
        }
    }
    if let Some(servers) = &scope.mcp_servers {
        for m in &a.mcp_servers {
            if !servers.iter().any(|have| have == m) {
                add(
                    "warning",
                    format!("MCP server `{m}` is not a server the store knows (pulse mcp list)"),
                );
            }
        }
    }
    for r in &a.renamed {
        add("info", format!("tool {r}"));
    }
    out
}

/// The layer a file given on the command line belongs to: the owner's
/// directory and the two foreign layouts by their path, anything else
/// as a repository file (the strictest reading).
fn layer_of_path(path: &Path, home: &str) -> AgentLayer {
    let text = path.to_string_lossy().replace('\\', "/");
    if !home.is_empty() {
        let own = Path::new(home).join("agents");
        if path.starts_with(&own) {
            return AgentLayer::Home;
        }
    }
    if text.contains("/.claude/agents/") || text.starts_with(".claude/agents/") {
        AgentLayer::Claude
    } else if text.contains("/.github/agents/") || text.starts_with(".github/agents/") {
        AgentLayer::Github
    } else {
        AgentLayer::Repo
    }
}

/// The specs behind the paths given: a directory stands for its `*.md`
/// files. A path that is neither is an error finding of its own.
fn specs_of_paths(paths: &[String], home: &str) -> (Vec<AgentDef>, Vec<Finding>) {
    let mut specs = Vec::new();
    let mut findings = Vec::new();
    for given in paths {
        let path = PathBuf::from(given);
        let files: Vec<PathBuf> = if path.is_dir() {
            let files = agent_files(&path);
            if files.is_empty() {
                findings.push(Finding {
                    level: "warning",
                    source: given.clone(),
                    message: "no *.md files in this directory".into(),
                });
            }
            files
        } else if path.is_file() {
            vec![path.clone()]
        } else {
            findings.push(Finding {
                level: "error",
                source: given.clone(),
                message: "no such file or directory".into(),
            });
            continue;
        };
        for file in files {
            let layer = layer_of_path(&file, home);
            match read_agent_file(&file, layer) {
                Some(mut def) => {
                    def.source = file.to_string_lossy().to_string();
                    specs.push(def);
                }
                None => findings.push(Finding {
                    level: "error",
                    source: file.to_string_lossy().to_string(),
                    message: "cannot be read".into(),
                }),
            }
        }
    }
    (specs, findings)
}

/// The warnings the merge adds to a name's owner (a lower twin tried to
/// widen it): what the specs did not say on their own.
fn merge_findings(specs: &[AgentDef]) -> Vec<Finding> {
    let mut out = Vec::new();
    for winner in merge_specs(specs.to_vec()) {
        let Some(own) = specs.iter().find(|a| a.source == winner.source) else {
            continue;
        };
        for w in &winner.warnings {
            if !own.warnings.contains(w) {
                out.push(Finding {
                    level: "warning",
                    source: winner.source.clone(),
                    message: w.clone(),
                });
            }
        }
    }
    out
}

/// Lints the specs: every finding of every file, then the summary; the
/// error that ends the command carries exit code 1 when a file has one.
pub fn lint_specs(specs: &[AgentDef], extra: Vec<Finding>, scope: &LintScope) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();
    for a in specs {
        findings.extend(lint_spec(a, scope));
    }
    findings.extend(merge_findings(specs));
    findings.extend(extra);
    findings
}

fn lint(c: &mut Context, store: Option<&Store>, cwd: &str, paths: &[String]) -> Result<()> {
    let project = c.project.clone();
    let scope = LintScope::read(cwd, &c.home, store, &project);
    let (specs, extra) = if paths.is_empty() {
        (read_agent_layers(cwd, &c.home, store, &project), Vec::new())
    } else {
        specs_of_paths(paths, &c.home)
    };
    let findings = lint_specs(&specs, extra, &scope);
    let errors = findings.iter().filter(|f| f.level == "error").count();
    let warnings = findings.iter().filter(|f| f.level == "warning").count();
    if c.json {
        let mut files: Vec<LintedFile> = specs
            .iter()
            .map(|a| LintedFile {
                source: &a.source,
                name: &a.name,
                layer: a.layer.label(),
                loads: a.loads(),
                findings: findings.iter().filter(|f| f.source == a.source).collect(),
            })
            .collect();
        for f in &findings {
            if !specs.iter().any(|a| a.source == f.source)
                && !files.iter().any(|x| x.source == f.source)
            {
                files.push(LintedFile {
                    source: &f.source,
                    name: "",
                    layer: AgentLayer::Repo.label(),
                    loads: false,
                    findings: findings.iter().filter(|x| x.source == f.source).collect(),
                });
            }
        }
        write_json(
            &mut c.out,
            &LintOut {
                files,
                errors,
                warnings,
            },
        )?;
    } else {
        for f in &findings {
            c.println(format!("{} {}: {}", f.level, f.source, f.message))?;
        }
        let what = if paths.is_empty() {
            format!("{} agent spec(s)", specs.len())
        } else {
            format!("{} file(s)", specs.len())
        };
        c.println(format!(
            "{errors} error(s), {warnings} warning(s) in {what}; the limits are {AGENT_BODY_LIMIT} characters of body and {DESCRIPTION_LIMIT} of description"
        ))?;
    }
    if errors > 0 {
        return Err(errorf(
            1,
            &format!("pulse agents lint: {errors} error(s)"),
            "",
            "fix the files named above; pulse agents schema prints the keys",
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_cli::Buffer;
    use std::fs;
    use std::io::Write;

    fn invoke(home: &str, project: &str, json: bool, args: &[&str]) -> (String, String, i32) {
        let out = Buffer::new();
        let err = Buffer::new();
        let mut c = Context {
            store: None,
            home: home.into(),
            out: Box::new(out.clone()),
            err: Box::new(err.clone()),
            json,
            quiet: false,
            color: false,
            agent: "tester".into(),
            project: project.into(),
            session: String::new(),
            now: "2026-09-11T05:28:22Z".into(),
            args: Vec::new(),
        };
        let mut fs = FlagSet::new("agents");
        fs.set_output(pulse_cli::flags::Output::Discard);
        flags(&mut fs);
        let argv: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        fs.parse(&argv).unwrap();
        let rest = fs.args().to_vec();
        let code = match run(&mut c, &fs, &rest) {
            Ok(()) => 0,
            Err(e) => {
                if let Some(ce) = e.downcast_ref::<pulse_cli::Error>() {
                    let _ = write!(c.err, "{ce}");
                    ce.exit_code()
                } else {
                    let _ = write!(c.err, "{e}");
                    1
                }
            }
        };
        (out.string(), err.string(), code)
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn lint_names_the_limits_the_template_and_the_tier_rule_and_exits_one() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let root = dir.path().join("repo");
        write(
            &root,
            ".pulse/agents/long.md",
            &format!(
                "---\ndescription: {}\n---\n{}\n",
                "d".repeat(DESCRIPTION_LIMIT + 1),
                "b".repeat(AGENT_BODY_LIMIT + 1)
            ),
        );
        write(
            &root,
            ".pulse/agents/templated.md",
            "---\ndescription: renders\n---\nWork in {{.Cwd}} but never {{.Unknown}}.\n",
        );
        write(
            &root,
            ".pulse/agents/hasty.md",
            "---\ndescription: too often\npulse:\n  schedule:\n    every: 30m\n---\nPoll.\n",
        );
        write(
            &root,
            ".pulse/agents/fine.md",
            "---\ndescription: fine\ntools: Bash, Read\n---\nAll good in {{.Cwd}}.\n",
        );
        let root_text = root.to_string_lossy().to_string();
        let home_text = home.to_string_lossy().to_string();

        let (out, err, code) = invoke(&home_text, "", false, &["--cwd", &root_text, "lint"]);
        assert_eq!(code, 1, "{out}\n{err}");
        assert!(
            out.contains(&format!(
                "error .pulse/agents/long.md: description is {} characters, over the {DESCRIPTION_LIMIT} limit",
                DESCRIPTION_LIMIT + 1
            )),
            "{out}"
        );
        assert!(
            out.contains(&format!(
                "error .pulse/agents/long.md: body is {} characters, over the {AGENT_BODY_LIMIT} limit",
                AGENT_BODY_LIMIT + 1
            )),
            "{out}"
        );
        assert!(
            out.contains(
                "error .pulse/agents/templated.md: body does not render as a prompt: template: prompt:1: map has no entry for key \"{{.Unknown}}\""
            ),
            "{out}"
        );
        assert!(
            out.contains(
                "error .pulse/agents/hasty.md: `pulse.schedule.every: 30m` is under the 60-minute floor (Tier 0/1)"
            ),
            "{out}"
        );
        assert!(
            out.contains("info .pulse/agents/fine.md: tool Bash -> bash"),
            "{out}"
        );
        assert!(
            out.contains("info .pulse/agents/fine.md: tool Read -> read"),
            "{out}"
        );
        assert!(!out.contains("fine.md: body does not render"), "{out}");
        assert!(out.contains("4 error(s),"), "{out}");
        assert!(out.contains("in 4 agent spec(s)"), "{out}");
        assert!(err.contains("pulse agents lint: 4 error(s)"), "{err}");

        // the same files by path, as JSON, with a path that is not there
        let missing = root.join("nowhere.md").to_string_lossy().to_string();
        let agents_dir = root.join(".pulse/agents").to_string_lossy().to_string();
        let (out, _err, code) = invoke(&home_text, "", true, &["lint", &agents_dir, &missing]);
        assert_eq!(code, 1, "{out}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["errors"], 5, "{out}");
        let files = v["files"].as_array().unwrap();
        assert_eq!(files.len(), 5, "{out}");
        let missing_file = files
            .iter()
            .find(|f| f["source"].as_str().unwrap() == missing)
            .unwrap();
        assert_eq!(
            missing_file["findings"][0]["message"],
            "no such file or directory"
        );
        let fine = files
            .iter()
            .find(|f| f["source"].as_str().unwrap().ends_with("fine.md"))
            .unwrap();
        assert_eq!(fine["loads"], true);
        assert_eq!(fine["layer"], ".pulse/agents");

        // only the clean file: exit 0
        let fine_path = root
            .join(".pulse/agents/fine.md")
            .to_string_lossy()
            .to_string();
        let (out, err, code) = invoke(&home_text, "", false, &["lint", &fine_path]);
        assert_eq!(code, 0, "{out}\n{err}");
        assert!(
            out.contains("0 error(s), 0 warning(s) in 1 file(s)"),
            "{out}"
        );
    }

    #[test]
    fn lint_reports_the_rejected_file_the_twin_that_widens_and_the_unknown_skill() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let root = dir.path().join("repo");
        write(
            &home,
            "agents/helper.md",
            "---\ndescription: the owner's helper\ntools: [read, grep]\nskills: [nothing-here]\n---\nHelp.\n",
        );
        write(
            &root,
            ".pulse/agents/helper.md",
            "---\ndescription: the repo's helper\ntools: [read, grep, edit, write]\n---\nHelp more.\n",
        );
        write(
            &root,
            ".claude/agents/free.md",
            "---\ndescription: unbounded\npermissionMode: bypassPermissions\n---\nAnything.\n",
        );
        let root_text = root.to_string_lossy().to_string();
        let home_text = home.to_string_lossy().to_string();

        let (out, _err, code) = invoke(&home_text, "", false, &["--cwd", &root_text, "lint"]);
        assert_eq!(code, 1, "{out}");
        assert!(
            out.contains(
                "error .claude/agents/free.md: does not load: `permissionMode: bypassPermissions`"
            ),
            "{out}"
        );
        assert!(
            out.contains("warning ~/.pulse/agents/helper.md: repository file `.pulse/agents/helper.md` tried to widen tools: edit, write"),
            "{out}"
        );
        assert!(
            out.contains(
                "warning ~/.pulse/agents/helper.md: skill `nothing-here` is not among the skills"
            ),
            "{out}"
        );
        assert!(out.contains("1 error(s),"), "{out}");

        // list shows the owner file winning and the rejected one apart
        let (out, _err, code) = invoke(&home_text, "", false, &["--cwd", &root_text, "list"]);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("helper"), "{out}");
        assert!(out.contains("~/.pulse/agents"), "{out}");
        assert!(
            out.contains("not loaded: .claude/agents/free.md: `permissionMode: bypassPermissions`"),
            "{out}"
        );
        let (out, _err, code) = invoke(&home_text, "", true, &["--cwd", &root_text, "list"]);
        assert_eq!(code, 0, "{out}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let agents = v["agents"].as_array().unwrap();
        assert_eq!(agents.len(), 1, "{out}");
        assert_eq!(agents[0]["name"], "helper");
        assert_eq!(agents[0]["layer"], "~/.pulse/agents");
        assert_eq!(agents[0]["tools"], serde_json::json!(["read", "grep"]));
        assert!(agents[0]["warnings"].as_u64().unwrap() >= 2, "{out}");
        assert_eq!(v["rejected"][0]["source"], ".claude/agents/free.md");
    }

    #[test]
    fn schema_is_a_draft_2020_12_document_and_an_unknown_subcommand_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_string_lossy().to_string();
        let (out, _err, code) = invoke(&home, "", false, &["schema"]);
        assert_eq!(code, 0, "{out}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["$schema"], "https://json-schema.org/draft/2020-12/schema",
            "{out}"
        );
        assert!(v["$id"].as_str().is_some_and(|id| !id.is_empty()), "{out}");
        assert!(
            v["properties"]["pulse"]["properties"]["kinds"].is_object(),
            "{out}"
        );

        let (_out, err, code) = invoke(&home, "", false, &["frobnicate"]);
        assert_eq!(code, 2);
        assert!(err.contains("unknown subcommand \"frobnicate\""), "{err}");

        let empty = dir.path().join("empty");
        fs::create_dir_all(&empty).unwrap();
        let empty_text = empty.to_string_lossy().to_string();
        let (out, _err, code) = invoke(&home, "", false, &["--cwd", &empty_text, "list"]);
        assert_eq!(code, 0, "{out}");
        assert!(out.starts_with("no custom agents"), "{out}");
    }
}
