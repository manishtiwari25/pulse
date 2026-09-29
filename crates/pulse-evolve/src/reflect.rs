//! Port of internal/evolve/reflect.go.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkingState {
    pub branch: String,
    pub head: String,
    pub changes: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub problem: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Hygiene {
    pub branches: Vec<String>,
    pub worktrees: Vec<String>,
    pub clones: Vec<String>,
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Reflection {
    pub project: String,
    pub working: WorkingState,
    pub board: String,
    #[serde(default, skip_deserializing)]
    pub ballot: Vec<pulse_tally::BallotItem>,
    pub proposals: Vec<pulse_tally::Proposal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gates: Option<crate::status::LoopStatus>,
    pub hygiene: Hygiene,
    pub verify_cmd: String,
    pub level: String,
    pub next_human_step: String,
    pub next_commands: Vec<String>,
    pub problems: Vec<String>,
}

/// Go: func lines(text string) []string {
pub(crate) fn lines(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(ToString::to_string)
        .collect()
}

/// Go: func workingState(ctx context.Context, repo string) WorkingState {
pub(crate) fn working_state(cancel: &pulse_bus::CancellationToken, repo: &str) -> WorkingState {
    let mut state = WorkingState {
        changes: Vec::new(),
        ..Default::default()
    };
    match crate::process::git_output(
        cancel,
        repo,
        &["rev-parse".into(), "--short".into(), "HEAD".into()],
    ) {
        Ok(head) => state.head = head,
        Err(_) => {
            state.problem = "checkout is unavailable or has no committed HEAD".to_string();
            return state;
        }
    }
    state.branch = crate::process::git_output(
        cancel,
        repo,
        &[
            "symbolic-ref".into(),
            "--short".into(),
            "--quiet".into(),
            "HEAD".into(),
        ],
    )
    .unwrap_or_else(|_| "detached".to_string());
    match crate::process::git_output(
        cancel,
        repo,
        &[
            "status".into(),
            "--short".into(),
            "--untracked-files=normal".into(),
        ],
    ) {
        Ok(status) => state.changes = lines(&status),
        Err(_) => state.problem = "git status could not inspect the working tree".to_string(),
    }
    state
}

/// Go: func hygiene(ctx context.Context, s *store.Store, repo string) Hygiene {
pub(crate) fn hygiene(
    cancel: &pulse_bus::CancellationToken,
    s: Option<&pulse_store::Store>,
    repo: &str,
) -> Hygiene {
    let mut h = Hygiene {
        branches: Vec::new(),
        worktrees: Vec::new(),
        clones: Vec::new(),
        problems: Vec::new(),
    };
    match crate::process::git_output(
        cancel,
        repo,
        &[
            "for-each-ref".into(),
            "--format=%(refname:short)".into(),
            "refs/heads/evolve/".into(),
            "refs/heads/pulse/".into(),
        ],
    ) {
        Ok(branches) => h.branches = lines(&branches),
        Err(_) => h
            .problems
            .push("cannot inspect stray experiment branches".to_string()),
    }
    match crate::process::git_output(
        cancel,
        repo,
        &["worktree".into(), "list".into(), "--porcelain".into()],
    ) {
        Ok(worktrees) => {
            for line in worktrees.lines() {
                if let Some(value) = line.strip_prefix("worktree ") {
                    h.worktrees.push(value.to_string());
                }
            }
        }
        Err(_) => h.problems.push("cannot inspect worktrees".to_string()),
    }
    if let Some(s) = s {
        let root = match crate::clones::fleet_root(s, false) {
            Ok(root) => root,
            Err(_) => {
                h.problems
                    .push("fleet location does not pass boundary checks".to_string());
                return h;
            }
        };
        match fs::read_dir(&root) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    if entry.file_type().is_ok_and(|t| t.is_dir()) {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if crate::fleet::MANAGED_ID.is_match(&name) {
                            h.clones
                                .push(Path::new(&root).join(name).display().to_string());
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => h
                .problems
                .push("cannot inspect managed clone directories".to_string()),
        }
    }
    h
}

/// Go: func Reflect(ctx context.Context, s *store.Store, project, area, agent, session string) (*Reflection, error) {
pub fn reflect(
    cancel: &pulse_bus::CancellationToken,
    s: Option<&pulse_store::Store>,
    project: &str,
    area: &str,
    agent: &str,
    session: &str,
) -> Result<Reflection> {
    let mut r = Reflection {
        project: project.to_string(),
        level: "A".to_string(),
        next_human_step: "pulse init PATH".to_string(),
        board: "NOTICES\n  no PULSE store or registered project; observation is still available"
            .to_string(),
        ballot: Vec::new(),
        proposals: Vec::new(),
        problems: Vec::new(),
        next_commands: Vec::new(),
        ..Default::default()
    };
    let repo = pulse_store::cwd();
    let Some(s) = s else {
        r.working = working_state(cancel, &repo);
        r.hygiene = hygiene(cancel, None, &repo);
        r.next_commands = vec!["pulse init PATH".into(), "pulse reflect --project P".into()];
        return Ok(r);
    };
    let p = match pulse_board::current_project(s, project) {
        Ok(p) => p,
        Err(_) => {
            r.problems
                .push("select a registered project with --project".to_string());
            r.next_human_step = "pulse projects".to_string();
            r.working = working_state(cancel, &repo);
            r.hygiene = hygiene(cancel, Some(s), &repo);
            r.next_commands = vec!["pulse projects".into(), "pulse reflect --project P".into()];
            return Ok(r);
        }
    };
    if p.slug == pulse_store::COMMONS_SLUG {
        return Err(crate::fixtures::input_error(
            "Reflection needs a project checkout",
            "use the commons board separately",
            "pulse projects",
        ));
    }
    r.project = p.slug.clone();
    r.verify_cmd = p.verify_cmd.clone();
    r.working = working_state(cancel, &p.path);
    r.hygiene = hygiene(cancel, Some(s), &p.path);
    let mut agent_name = agent.to_string();
    let mut session_name = session.to_string();
    let a = pulse_board::resolve_agent(s, agent, false)?;
    if let Some(a) = a.as_ref() {
        agent_name = a.name.clone();
        session_name = pulse_board::agent_session(Some(a), session);
    } else {
        agent_name.clear();
    }
    r.board = pulse_tally::digest_for(
        s,
        &p.slug,
        area,
        pulse_tally::Thresholds::default(),
        &agent_name,
        &session_name,
    )?;
    r.ballot = pulse_tally::ballot(
        s,
        &p.slug,
        &agent_name,
        &session_name,
        pulse_tally::Thresholds::default(),
    )?;
    r.proposals = pulse_tally::proposals(s, &p.slug)?;
    let gates = crate::status::status(s, &p.slug)?;
    r.level = gates.level.clone();
    r.next_human_step = gates.next_human_step.clone();
    r.gates = Some(gates);
    let area = if area.is_empty() { "evolution" } else { area };
    r.next_commands.push(format!(
        "pulse post finding --project {} --area {} --thread reflection --summary 'FACT' --body 'EVIDENCE'",
        p.slug, area
    ));
    for item in &r.ballot {
        let (id, amendment) = if let Some(s) = item.suggestion.as_ref() {
            (s.id.clone(), false)
        } else if let Some(a) = item.amendment.as_ref() {
            (a.id.clone(), true)
        } else {
            (String::new(), false)
        };
        if !id.is_empty() {
            r.next_commands.push(format!("pulse show {id}"));
            let mut vote = format!("pulse vote {id} up|down --summary 'EVIDENCE'");
            if amendment {
                vote.push_str(" --text-hash HASH --body 'Read: WHAT CHANGES'");
            }
            r.next_commands.push(vote);
            break;
        }
    }
    if r.ballot.is_empty() {
        r.next_commands
            .push(format!("pulse ballot --project {}", p.slug));
    }
    if let Some(gates) = &r.gates {
        if r.level == "D"
            && matches!(
                gates.planning.state.as_str(),
                "ready" | "queued" | "leased" | "finalizing"
            )
        {
            r.next_commands
                .push(format!("pulse evolve cycle --project {}", p.slug));
        } else {
            r.next_commands
                .push(format!("pulse evolve status --project {}", p.slug));
        }
    }
    r.next_commands
        .push(format!("pulse evolve report --project {}", p.slug));
    Ok(r)
}

/// Go: func printReflection(w io.Writer, color bool, r *Reflection) {
pub(crate) fn print_reflection(w: &mut dyn Write, color: bool, r: &Reflection) {
    let _ = writeln!(w, "REFLECT - {}", r.project);
    let _ = writeln!(
        w,
        "\nWORKING STATE\n  branch {} at {}; {} changed path(s)",
        r.working.branch,
        r.working.head,
        r.working.changes.len()
    );
    for change in &r.working.changes {
        let _ = writeln!(w, "  {change}");
    }
    if !r.working.problem.is_empty() {
        let _ = writeln!(w, "  {}", r.working.problem);
    }
    let _ = writeln!(w, "\nBOARD\n{}", r.board);
    let _ = writeln!(w, "{}", pulse_tally::ballot_text(&r.project, "", &r.ballot));
    let _ = writeln!(w, "FOR THE HUMAN");
    let _ = writeln!(w, "{}", pulse_tally::proposals_text(&r.proposals));
    if let Some(gates) = &r.gates {
        let _ = writeln!(w, "LOOP GATES");
        let _ = pulse_cli::table(w, color, None, &crate::status::status_rows(gates));
    }
    let _ = writeln!(
        w,
        "\nHYGIENE (report other sessions' strays; never delete them)"
    );
    let _ = pulse_cli::table(
        w,
        color,
        Some(&["KIND", "COUNT", "PATHS"]),
        &[
            vec![
                "evolve/* and pulse/* branches".into(),
                r.hygiene.branches.len().to_string(),
                r.hygiene.branches.join(", "),
            ],
            vec![
                "worktrees".into(),
                r.hygiene.worktrees.len().to_string(),
                r.hygiene.worktrees.join(", "),
            ],
            vec![
                "managed clone roots".into(),
                r.hygiene.clones.len().to_string(),
                r.hygiene.clones.join(", "),
            ],
        ],
    );
    let verify = if r.verify_cmd.is_empty() {
        "not configured"
    } else {
        &r.verify_cmd
    };
    let _ = writeln!(
        w,
        "\nHEALTH CHECKS\n  project verify_cmd (not run): {verify}"
    );
    for problem in r.problems.iter().chain(r.hygiene.problems.iter()) {
        let _ = writeln!(w, "  {problem}");
    }
    let _ = writeln!(w, "\nLEVEL ALLOWED: {}", r.level);
    let _ = writeln!(w, "NEXT HUMAN STEP: {}", r.next_human_step);
    let _ = writeln!(
        w,
        "\nNEXT COMMANDS (fill uppercase placeholders; choose your vote from evidence)"
    );
    for command in &r.next_commands {
        let _ = writeln!(w, "{command}");
    }
}
