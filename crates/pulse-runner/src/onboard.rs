use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub(crate) const DECISION_VALID_DAYS: i32 = 7;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct DecisionHit {
    pub post_id: String,
    pub answer: String,
    pub decided_at: String,
    pub valid: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct OnboardOpts {
    pub name: String,
    pub remote: String,
    pub verify_cmd: String,
    pub actor: String,
    pub brain: bool,
}

/// Go: func authorizingDecision(q querier, marker, now string) *decisionHit
pub(crate) fn authorizing_decision(
    q: &dyn crate::util::Querier,
    marker: String,
    now: String,
) -> Option<Box<DecisionHit>> {
    let pattern = Regex::new(&format!(r"(?m)^[ \t]*{}[ \t]*$", regex::escape(&marker))).ok()?;
    let rows = crate::util::must_rows(
        q,
        "SELECT d.post_id, d.answer, d.decided_at, p.body FROM decisions d JOIN posts p ON p.id = d.post_id WHERE d.decided_by = ? ORDER BY d.decided_at DESC, d.post_id DESC",
        &[&pulse_store::HUMAN_AGENT],
    );
    for r in rows {
        if !pattern.is_match(&r.str("body")) {
            continue;
        }
        let age = crate::util::must_iso(&now)
            .signed_duration_since(crate::util::must_iso(&r.str("decided_at")));
        let valid = r.str("answer") == "yes"
            && age >= chrono::Duration::zero()
            && age <= chrono::Duration::days(DECISION_VALID_DAYS as i64);
        return Some(Box::new(DecisionHit {
            post_id: r.str("post_id"),
            answer: r.str("answer"),
            decided_at: r.str("decided_at"),
            valid,
        }));
    }
    None
}

/// Go: func agentGate(q querier, marker, actor, now string) (string, error)
pub(crate) fn agent_gate(
    q: &dyn crate::util::Querier,
    marker: String,
    actor: String,
    now: String,
) -> anyhow::Result<String> {
    if actor.is_empty() || actor == pulse_store::HUMAN_AGENT {
        return Ok(String::new());
    }
    let Some(found) = authorizing_decision(q, marker.clone(), now) else {
        return Err(crate::util::fail(2, format!("An agent may run this only after the human's `pulse decide ... yes` on a board proposal whose body carries the line `{marker}`; no decided proposal names it")).into());
    };
    if !found.valid {
        return Err(crate::util::fail(2, format!("The newest decision naming `{marker}` ({}: {} on {}) does not authorize it; a yes at most {} days old is needed", found.post_id, found.answer, crate::recipe::short(&found.decided_at, 10), DECISION_VALID_DAYS)).into());
    }
    Ok(found.post_id)
}

/// Go: func daemonStatus(s *store.Store, now string) string
pub(crate) fn daemon_status(s: &pulse_store::Store, now: String) -> String {
    let Ok(conn) = s.conn() else {
        return "never started".into();
    };
    let r = crate::util::must_one(&conn, "SELECT value FROM meta WHERE key = 'daemon'", &[]);
    if r.0.is_empty() {
        return "never started".into();
    }
    let Ok(beat) = serde_json::from_str::<serde_json::Value>(&r.str("value")) else {
        return "unreadable heartbeat".into();
    };
    if beat.get("note").and_then(|v| v.as_str()) == Some("exited") {
        return format!(
            "exited {} (pid {})",
            beat.get("at")
                .map(|v| v.to_string())
                .unwrap_or_default()
                .trim_matches('"'),
            beat.get("pid").map(|v| v.to_string()).unwrap_or_default()
        );
    }
    let at = beat.get("at").and_then(|v| v.as_str()).unwrap_or("");
    let age = crate::util::must_iso(&now)
        .signed_duration_since(crate::util::must_iso(at))
        .num_seconds();
    let interval = beat
        .get("interval")
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
        .filter(|v| *v > 0)
        .unwrap_or(crate::daemon::DEFAULT_DAEMON_INTERVAL_SECONDS as i64);
    let loops = beat
        .get("loops")
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
        .unwrap_or(0);
    let state = if age > 2 * interval + 60 {
        "silent"
    } else {
        "alive"
    };
    let pid = beat
        .get("pid")
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
        .map(|p| p.to_string())
        .unwrap_or_default();
    format!("{state} pid {pid}, last loop {age}s ago, {loops} loop(s), interval {interval}s")
}

/// Go: func onboardProject(s *store.Store, path, now string, o onboardOpts) ([]string, error)
pub(crate) fn onboard_project(
    s: &pulse_store::Store,
    path: String,
    now: String,
    o: OnboardOpts,
) -> anyhow::Result<Vec<String>> {
    let root = Path::new(&crate::board::expand_user(path))
        .canonicalize()?
        .display()
        .to_string();
    if !Path::new(&root).is_dir() {
        return Err(crate::util::fail(2, format!("Not a directory: {root}")).into());
    }
    let conn = s.conn()?;
    let existing =
        crate::util::must_one(&conn, "SELECT slug FROM projects WHERE path = ?", &[&root]);
    let slug = if !existing.0.is_empty() {
        existing.str("slug")
    } else {
        let base_name = if o.name.is_empty() {
            Path::new(&root)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("project")
                .to_string()
        } else {
            o.name.clone()
        };
        let mut base = crate::util::slugify(&base_name, 40);
        if base == pulse_store::COMMONS_SLUG {
            base.push_str("-project");
        }
        crate::board::unique_slug(&conn, base, "projects".into(), "slug".into())
    };
    let decision = agent_gate(
        &conn,
        format!("onboard: {slug}"),
        o.actor.clone(),
        now.clone(),
    )?;
    let evolution = Path::new(&root).join("docs").join("evolution");
    if let Ok(st) = std::fs::metadata(&evolution) {
        if !st.is_dir() {
            return Err(crate::util::fail(
                2,
                format!("{} exists but is not a directory", evolution.display()),
            )
            .into());
        }
        let missing: Vec<&str> = ["constitution.md", "evaluators.md", "archive.md"]
            .into_iter()
            .filter(|name| !evolution.join(name).exists())
            .collect();
        if !missing.is_empty() {
            return Err(crate::util::fail(2, format!("docs/evolution/ is partial; missing {}. A half-filled loop folder is the owner's to finish", missing.join(", "))).into());
        }
    }
    let (project, created) = crate::board::add_project(
        s,
        root.clone(),
        o.name.clone(),
        o.remote,
        o.brain,
        o.verify_cmd,
        o.actor.clone(),
        String::new(),
    )?;
    let (onboard_job, onboard_new) =
        crate::jobs::queue_onboarding_job(s, project.str("slug"), "init".into())?;
    let mut reflect = crate::util::must_one(
        &conn,
        "SELECT * FROM jobs WHERE project = ? AND kind = 'reflect' ORDER BY created_at, id LIMIT 1",
        &[&project.str("slug")],
    );
    let mut reflect_new = false;
    if reflect.0.is_empty() {
        let by = if o.actor.is_empty() {
            "onboard".into()
        } else {
            o.actor.clone()
        };
        let (j, created) = crate::jobs::add_job(
            s,
            project.str("slug"),
            "reflect".into(),
            "First reflect: turn the onboarding findings into one-variable suggestion cards".into(),
            "First reflect after onboarding: read the onboarding findings and the loop's records (`pulse record show`), run the project's own checks, and turn each useful signal into one-variable suggestion cards. Do not flip loop gates; record human-only baseline or loop-on work as FINDING: lines.".into(),
            0,
            19,
            format!("onboard-reflect:{}", project.str("slug")),
            by,
            true,
        )?;
        reflect = j;
        reflect_new = created;
    }
    let actor = if o.actor.is_empty() {
        pulse_store::HUMAN_AGENT.to_string()
    } else {
        o.actor
    };
    crate::util::event_at(
        &conn,
        &now,
        &actor,
        "project.onboard",
        &project.str("slug"),
        serde_json::json!({"created": created, "scaffolded": [], "decision": decision}),
    );
    let verb = if created { "registered" } else { "refreshed" };
    let brain_note = if project.int("brain") != 0 {
        " (brain)"
    } else {
        ""
    };
    let mut lines = vec![format!(
        "{verb} project {} at {}{}",
        project.str("slug"),
        project.str("path"),
        brain_note
    )];
    lines.push(format!(
        "{} onboarding job {}",
        if onboard_new {
            "queued"
        } else {
            "already queued"
        },
        onboard_job.str("id")
    ));
    lines.push(format!(
        "{} {}",
        if reflect_new {
            "queued first reflect job"
        } else {
            "first reflect already exists"
        },
        reflect.str("id")
    ));
    if evolution.exists() {
        lines.push("docs/evolution/ already present, untouched".into());
    } else {
        lines.push("no docs/evolution/ scaffold written: the loop's constitution and evaluators live as records".into());
    }
    lines.push(loop_record_line(
        s,
        project.str("slug"),
        "constitution".into(),
    ));
    lines.push(loop_record_line(
        s,
        project.str("slug"),
        "evaluators".into(),
    ));
    if crate::recipe::recipe_text(s, &project.str("slug")).1 {
        lines.push("environment recipe present (CTX-environment), untouched".into());
    } else if Path::new(&root).join(crate::recipe::RECIPE_PATH).exists() {
        lines.push(format!(
            "{} found in the checkout; `pulse env import --project {}` moves it into the store",
            crate::recipe::RECIPE_PATH,
            project.str("slug")
        ));
    } else {
        lines.push("no environment recipe yet; the onboarding agent writes it with `pulse env set`, a human reviews it".into());
    }
    let starters = starter_count(s, project.str("slug"));
    if created && starters > 0 {
        lines.push(format!(
            "seeded {starters} starter fixtures (pulse evolve fixture list --project {})",
            project.str("slug")
        ));
    } else if starters == 0 {
        lines.push(format!(
            "starter fixtures absent: pulse evolve fixture seed --project {}",
            project.str("slug")
        ));
    }
    lines.extend([
        "What stays human:".into(),
        format!("1. the baseline records itself: once evolve.loop is 1 the daemon measures {} with the enabled fixtures and archives the baseline (ADR-031; costs real agent runs); sooner by hand: pulse job add --project {} --kind baseline --title 'Record baseline'", project.str("default_branch"), project.str("slug")),
        heldout_step(s),
        "3. set loop: on in the constitution record".into(),
    ]);
    if !daemon_status(s, now).starts_with("alive ") {
        lines.push("4. keep the daemon and the hub alive with `pulse service install`".into());
    }
    lines.push(format!("store: {}", s.path));
    Ok(lines)
}

/// Go: func starterCount(s *store.Store, slug string) int
pub(crate) fn starter_count(s: &pulse_store::Store, slug: String) -> i32 {
    match pulse_evolve::list_fixtures(s, &slug, false) {
        Ok(fixtures) => fixtures
            .iter()
            .filter(|f| pulse_evolve::is_starter(&f.name))
            .count() as i32,
        Err(_) => -1,
    }
}

/// Go: func loopRecordLine(s *store.Store, slug, name string) string
pub(crate) fn loop_record_line(s: &pulse_store::Store, slug: String, name: String) -> String {
    let rows = s.list_records(pulse_store::RecordFilter {
        project: slug.clone(),
        kind: "evolution".into(),
        slug: name.clone(),
        ..Default::default()
    });
    let Ok(rows) = rows else {
        return format!("{name}: absent: pulse evolve seed --project {slug}");
    };
    let Some(rec) = rows.first() else {
        return format!("{name}: absent: pulse evolve seed --project {slug}");
    };
    if rec.current_rev == 1
        && let Ok((_, Some(rev))) = s.get_record(&rec.id, 0)
    {
        if let Some(from) = rev.frontmatter.get("seeded_from").and_then(|v| v.as_str())
            && !from.is_empty()
        {
            return format!(
                "{name}: copied from {} ({})",
                from.rsplit('/').next().unwrap_or(from),
                rec.id
            );
        }
        if let Some(origin) = rec.origin.strip_prefix("markdown:") {
            let origin = origin.split('@').next().unwrap_or(origin);
            return format!("{name}: imported from {origin} ({})", rec.id);
        }
    }
    format!("{name}: present ({} r{})", rec.id, rec.current_rev)
}

/// Go: func heldoutStep(s *store.Store) string
pub(crate) fn heldout_step(s: &pulse_store::Store) -> String {
    if pulse_evolve::heldout_mode(s) == "owner" {
        "2. keep at least two private fixtures in evolve.heldout_dir: pulse evolve setup DIR".into()
    } else {
        "2. nothing for the held-out set: the daemon writes two sealed fixtures itself (ADR-033); evolve.heldout = owner to hand-write them instead".into()
    }
}
