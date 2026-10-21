//! The toolkit a project's shape asks for: the MCP servers and skills pulse
//! suggests from what a checkout shows (its remote, manifests, folders)
//! and what it already has. `pulse toolkit` prints them, the hub shows
//! them as cards, and the daemon asks the owner about one at a time the
//! way it asks about a lead seat; a yes registers the tool or the drafted
//! skill, a declined thread rests the question for `toolkit.suggest_days`.
//! While the owner is away (`owner.presence = away`) nothing is asked: the
//! daemon adds what needs no hand of the owner's - a drafted skill, a tool
//! without a secret - and says so on the board; a tool that needs a secret
//! stays a card on the hub.

pub mod shape;
pub mod skills;
pub mod tools;

use crate::util::{event_at, must_one, must_rows, plus_hours, trunc};
use anyhow::Result;
use pulse_cli::{Context, FlagSet};
use serde::Serialize;
use serde_json::json;
use shape::Shape;
use std::io::Write;

pub const ASK_KIND: &str = "toolkit";
pub const BOARD_AREA: &str = "toolkit";
/// The line a daemon question carries so a yes knows what to add:
/// `toolkit: <slug> tool|skill <name>`.
pub const DECISION_MARKER: &str = "toolkit:";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ToolSuggestion {
    pub project: String,
    pub name: String,
    pub title: String,
    pub about: String,
    pub docs: String,
    pub why: String,
    pub transport: String,
    pub command: String,
    pub args: Vec<String>,
    pub url: String,
    pub auth: String,
    pub auth_header: String,
    pub secret: String,
    /// `shared` for every project, `project` for this one.
    pub scope: String,
    /// The `pulse mcp add` line that registers it.
    pub add: String,
    /// What follows the add, when something does (a login, a secret).
    pub after: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SkillSuggestion {
    pub project: String,
    /// The name the draft is registered under: the recipe's, or
    /// `<slug>-<recipe>` when another project's skill holds it.
    pub name: String,
    /// The catalog recipe; the board thread and the summary use it, so a
    /// name that falls back to the prefixed form is still the same ask.
    pub recipe: String,
    pub description: String,
    pub why: String,
    pub body: String,
    /// The `pulse skill add` line that registers the drafted body.
    pub add: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogTool {
    pub name: &'static str,
    pub title: &'static str,
    pub about: &'static str,
    pub docs: &'static str,
    pub transport: &'static str,
    pub auth: &'static str,
    pub shared: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogSkill {
    pub name: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Catalog {
    pub tools: Vec<CatalogTool>,
    pub skills: Vec<CatalogSkill>,
}

/// Everything pulse knows how to suggest, signal or not, for the hub's
/// "more" lists.
pub fn catalog() -> Catalog {
    Catalog {
        tools: tools::TOOLS
            .iter()
            .map(|t| CatalogTool {
                name: t.name,
                title: t.title,
                about: t.about,
                docs: t.docs,
                transport: t.transport,
                auth: t.auth,
                shared: t.shared,
            })
            .collect(),
        skills: skills::SKILLS
            .iter()
            .map(|k| CatalogSkill {
                name: k.name,
                description: k.description,
            })
            .collect(),
    }
}

fn tool_suggestion(t: &tools::ToolPick, shape: &Shape, why: String) -> ToolSuggestion {
    let (add, after) = tools::add_command(t, &shape.project);
    ToolSuggestion {
        project: shape.project.clone(),
        name: t.name.into(),
        title: t.title.into(),
        about: t.about.into(),
        docs: t.docs.into(),
        why,
        transport: t.transport.into(),
        command: t.command.into(),
        args: t.args.iter().map(|a| (*a).to_string()).collect(),
        url: t.url.into(),
        auth: t.auth.into(),
        auth_header: t.auth_header.into(),
        secret: t.secret.into(),
        scope: if t.shared { "shared" } else { "project" }.into(),
        add,
        after,
    }
}

fn skill_suggestion(k: &skills::SkillPick, shape: &Shape, why: String) -> SkillSuggestion {
    let name = skills::draft_name(k, shape);
    SkillSuggestion {
        project: shape.project.clone(),
        name: name.clone(),
        recipe: k.name.into(),
        description: k.description.into(),
        why,
        body: (k.body)(shape),
        add: format!(
            "pulse toolkit add skill {name} --project {}   (or: pulse skill add --name {name} --description '...' --file SKILL.md --project {})",
            shape.project, shape.project
        ),
    }
}

/// The tools and skills a shape asks for and does not have, in catalog
/// order.
pub fn suggest_for(shape: &Shape) -> (Vec<ToolSuggestion>, Vec<SkillSuggestion>) {
    let tools = tools::TOOLS
        .iter()
        .filter(|t| !shape.tools.contains(t.name))
        .filter_map(|t| (t.signal)(shape).map(|why| tool_suggestion(t, shape, why)))
        .collect();
    let skills = skills::SKILLS
        .iter()
        .filter(|k| !skills::present(k, shape))
        .filter_map(|k| (k.signal)(shape).map(|why| skill_suggestion(k, shape, why)))
        .collect();
    (tools, skills)
}

/// The suggestions for one project, read from its checkout now.
pub fn suggestions(
    s: &pulse_store::Store,
    slug: &str,
) -> (Vec<ToolSuggestion>, Vec<SkillSuggestion>) {
    suggest_for(&shape::read(s, slug))
}

/// Registers a catalog tool for a project the way the owner's `pulse mcp
/// add` line would: shared picks for every project, the rest for this one.
pub fn add_tool(
    s: &pulse_store::Store,
    actor: &str,
    slug: &str,
    name: &str,
) -> Result<pulse_store::McpServer> {
    let Some(t) = tools::pick(name) else {
        anyhow::bail!("No tool {name:?} in the catalog (pulse toolkit --json lists it)");
    };
    if t.auth != "none" && t.auth != "oauth" {
        anyhow::bail!(
            "{} needs a secret ({}); register it yourself once the secret is in place: {}",
            t.name,
            t.secret,
            tools::add_command(t, slug).0
        );
    }
    s.add_mcp_server(
        actor,
        pulse_store::McpServerInput {
            name: t.name.into(),
            project: if t.shared { String::new() } else { slug.into() },
            transport: t.transport.into(),
            command: t.command.into(),
            args: t.args.iter().map(|a| (*a).to_string()).collect(),
            url: t.url.into(),
            auth: t.auth.into(),
            auth_header: t.auth_header.into(),
            secret_ref: String::new(),
            env: Default::default(),
            note: format!("{} Suggested by pulse toolkit for {slug}.", t.about),
        },
    )
}

/// Registers a drafted skill for a project, written from its shape now.
pub fn add_skill(
    s: &pulse_store::Store,
    actor: &str,
    slug: &str,
    name: &str,
) -> Result<pulse_store::Skill> {
    let shape = shape::read(s, slug);
    let Some(k) = skills::SKILLS
        .iter()
        .find(|k| k.name == name || skills::draft_name(k, &shape) == name)
    else {
        anyhow::bail!("No skill {name:?} in the catalog (pulse toolkit --json lists it)");
    };
    s.add_skill(
        actor,
        pulse_store::SkillInput {
            name: skills::draft_name(k, &shape),
            project: slug.into(),
            description: k.description.into(),
            body: (k.body)(&shape),
        },
    )
}

/// Asks the owner, once per `toolkit.suggest_days`, about the first tool
/// or skill the project's shape asks for. One question per project at a
/// time, like a lead seat: an open one holds the rest, a declined thread
/// rests for the window. While the owner is away nothing is asked; see
/// [`add_toolkit_unasked`]. Returns the daemon's log lines.
pub(crate) fn suggest_toolkit(
    s: &pulse_store::Store,
    slug: &str,
    now: &str,
    actor: &str,
) -> Vec<String> {
    let days = crate::util::setting_int(s, "toolkit.suggest_days");
    if days <= 0 || slug == pulse_store::COMMONS_SLUG {
        return Vec::new();
    }
    let Ok(conn) = s.conn() else {
        return Vec::new();
    };
    // A look reads the checkout's manifests, so a project is looked at once
    // an hour, not once a pass.
    let key = format!("toolkit-suggest:{slug}");
    let last = must_one(&conn, "SELECT value AS x FROM meta WHERE key = ?", &[&key]).str("x");
    if !last.is_empty() && last > plus_hours(now, -1.0) {
        return Vec::new();
    }
    let _ = conn.execute(
        "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, now],
    );
    if s.owner_away() {
        return add_toolkit_unasked(s, slug, now, actor, days);
    }
    if let Some(open) = crate::hygiene::open_daemon_questions_of_kind(s, slug, ASK_KIND).first() {
        return vec![format!(
            "toolkit suggestion for {slug}: waiting on {} ({})",
            open.str("id"),
            trunc(&open.str("summary"), 80)
        )];
    }
    let since = plus_hours(now, -24.0 * f64::from(days));
    let (tools, skills) = suggestions(s, slug);
    let mut candidates: Vec<(String, String, String)> = Vec::new();
    for t in &tools {
        candidates.push((
            tool_thread(t),
            trunc(&format!("Connect {} to {slug}? {}", t.title, t.why), 230),
            tool_body(t, days),
        ));
    }
    for k in &skills {
        candidates.push((
            skill_thread(k),
            trunc(
                &format!(
                    "Add {} {} skill to {slug}? {}",
                    article(&k.recipe),
                    k.recipe,
                    k.why
                ),
                230,
            ),
            skill_body(k, days),
        ));
    }
    for (thread, summary, body) in candidates {
        let asked = must_one(
            &conn,
            "SELECT date AS x FROM posts WHERE project = ? AND area = ? AND thread = ? AND kind = 'question' ORDER BY date DESC LIMIT 1",
            &[&slug, &BOARD_AREA, &thread],
        )
        .str("x");
        if !asked.is_empty() && asked >= since {
            continue;
        }
        match crate::hygiene::post_daemon_owner_question(
            s,
            crate::hygiene::DaemonQuestionAsk {
                project: slug.to_string(),
                ask_kind: ASK_KIND.into(),
                area: BOARD_AREA.into(),
                thread: thread.clone(),
                summary: summary.clone(),
                body,
                actor: actor.to_string(),
                now: now.to_string(),
            },
        ) {
            Ok(Some(post)) => {
                event_at(
                    &conn,
                    now,
                    actor,
                    "toolkit.suggested",
                    slug,
                    json!({"thread": thread, "post": post.str("id"), "summary": summary}),
                );
                return vec![format!(
                    "asked the owner about {thread} for {slug} ({})",
                    post.str("id")
                )];
            }
            Ok(None) => return Vec::new(),
            Err(e) => {
                return vec![format!(
                    "toolkit suggestion for {slug}: {}",
                    crate::run::err_message(&e)
                )];
            }
        }
    }
    Vec::new()
}

fn tool_thread(t: &ToolSuggestion) -> String {
    format!("tool-{}", t.name)
}

fn skill_thread(k: &SkillSuggestion) -> String {
    format!("skill-{}", k.recipe)
}

fn article(word: &str) -> &'static str {
    if word.starts_with(['a', 'e', 'i', 'o', 'u']) {
        "an"
    } else {
        "a"
    }
}

/// One thing the shape asks for, in the away pass.
enum Candidate<'a> {
    Tool(&'a ToolSuggestion),
    Skill(&'a SkillSuggestion),
}

/// Adds the toolkit the shape asks for without a question, while the owner
/// is away: a drafted skill and a tool that needs no secret are added and
/// said so on the board - a `result` on the thread, or an `answer` on the
/// question the owner was asked while present - with the line that undoes
/// it. A tool that needs a secret is the owner's and stays a card on the
/// hub, with no post. A thread the daemon wrote to inside the window is
/// left alone, so a failing add is reported once per window. Returns the
/// daemon's log lines.
fn add_toolkit_unasked(
    s: &pulse_store::Store,
    slug: &str,
    now: &str,
    actor: &str,
    days: i32,
) -> Vec<String> {
    let Ok(conn) = s.conn() else {
        return Vec::new();
    };
    let since = plus_hours(now, -24.0 * f64::from(days));
    let open = crate::hygiene::open_daemon_questions_of_kind(s, slug, ASK_KIND);
    let (tools, skills) = suggestions(s, slug);
    let candidates = tools
        .iter()
        .map(Candidate::Tool)
        .chain(skills.iter().map(Candidate::Skill))
        .collect::<Vec<_>>();
    let mut lines = Vec::new();
    for c in candidates {
        let (thread, kind, name, why, undo) = match &c {
            Candidate::Tool(t) => {
                if t.auth != "none" && t.auth != "oauth" {
                    continue;
                }
                (
                    tool_thread(t),
                    "tool",
                    t.name.clone(),
                    t.why.clone(),
                    format!("pulse mcp disable {}", t.name),
                )
            }
            Candidate::Skill(k) => (
                skill_thread(k),
                "skill",
                k.recipe.clone(),
                k.why.clone(),
                format!(
                    "pulse skill disable {name}; edit it: pulse skill update {name}",
                    name = k.name
                ),
            ),
        };
        let question = open
            .iter()
            .find(|q| q.str("thread") == thread)
            .map(|q| q.str("id"));
        if question.is_none() {
            // A decline (the owner's `decide no` or a declined thread) and
            // the daemon's own outcome rest the thread for the window; a
            // question somebody merely answered does not, since nothing
            // was added on it.
            let rested = must_one(
                &conn,
                "SELECT date AS x FROM posts WHERE project = ? AND area = ? AND thread = ? \
                 AND (resolution = 'declined' OR (agent = ? AND kind IN ('result', 'answer'))) \
                 ORDER BY date DESC LIMIT 1",
                &[&slug, &BOARD_AREA, &thread, &actor],
            )
            .str("x");
            if !rested.is_empty() && rested >= since {
                continue;
            }
        }
        let added = match &c {
            Candidate::Tool(t) => add_tool(s, actor, slug, &t.name).map(|m| (m.id, m.auth)),
            Candidate::Skill(k) => {
                add_skill(s, actor, slug, &k.recipe).map(|k| (k.id, String::new()))
            }
        };
        let what = match &c {
            Candidate::Tool(t) => format!("Connected {} to {slug}", t.title),
            Candidate::Skill(k) => {
                format!("Added {} {} skill to {slug}", article(&k.recipe), k.recipe)
            }
        };
        let (summary, body, resolution) = match &added {
            Ok((id, auth)) => {
                let sign_in = if auth == "oauth" {
                    format!(
                        "\n\nIt works once someone signs in: **Sign in** on its card under Connections, or `pulse mcp login {name}` in a terminal; until then agents see it as not signed in and skip it."
                    )
                } else {
                    String::new()
                };
                (
                    trunc(&format!("{what} while the owner is away: {why}"), 230),
                    format!(
                        "`{slug}` shows this: {why}.\n\n\
The owner is away (`owner.presence = away`), so pulse did this on its own instead of asking: {what} ({id}).{sign_in}\n\n\
Undo it: `{undo}`. With the owner present pulse asks first; `pulse settings set toolkit.suggest_days 0` stops toolkit suggestions for good."
                    ),
                    "proceeded",
                )
            }
            Err(e) => {
                let e = crate::run::err_message(e);
                let add = match &c {
                    Candidate::Tool(t) => t.add.clone(),
                    Candidate::Skill(k) => k.add.clone(),
                };
                (
                    trunc(
                        &format!("Could not add the {name} {kind} to {slug} on its own: {e}"),
                        230,
                    ),
                    format!(
                        "`{slug}` shows this: {why}.\n\n\
The owner is away, so pulse tried to add this itself and could not: {e}\n\n\
Add it by hand: `{add}`. Nothing else is asked about it for {days} days."
                    ),
                    "dropped",
                )
            }
        };
        let posted = crate::board::create_post(
            s,
            now.to_string(),
            crate::board::PostOpts {
                kind: if question.is_some() {
                    "answer"
                } else {
                    "result"
                }
                .into(),
                project: slug.to_string(),
                area: BOARD_AREA.into(),
                thread: thread.clone(),
                summary,
                body,
                agent: crate::board::actor_agent(s, actor.to_string()),
                on: question.clone().unwrap_or_default(),
                resolution: resolution.into(),
                date: now.to_string(),
                ..Default::default()
            },
        );
        let post = match posted {
            Ok(p) => p.str("id"),
            Err(e) => {
                lines.push(format!(
                    "toolkit for {slug}: {thread} not posted: {}",
                    crate::run::err_message(&e)
                ));
                String::new()
            }
        };
        match added {
            Ok((id, auth)) => {
                event_at(
                    &conn,
                    now,
                    actor,
                    "toolkit.added",
                    &id,
                    json!({"project": slug, "kind": kind, "name": name, "post": post, "question": question, "owner": "away"}),
                );
                let next = if auth == "oauth" {
                    format!("; it waits for a sign-in: pulse mcp login {name}")
                } else {
                    String::new()
                };
                lines.push(format!(
                    "added {kind} {name} ({id}) for {slug} while the owner is away{next}"
                ));
            }
            Err(e) => lines.push(format!(
                "could not add {kind} {name} for {slug} while the owner is away: {}",
                crate::run::err_message(&e)
            )),
        }
    }
    lines
}

fn decline_lines(slug: &str, thread: &str, days: i32) -> String {
    format!(
        "Decline: resolve this thread (`pulse thread resolve {BOARD_AREA}/{thread} --project {slug} --as declined`) and I will not ask again for {days} days; `pulse settings set toolkit.suggest_days 0` stops these suggestions for good."
    )
}

fn tool_body(t: &ToolSuggestion, days: i32) -> String {
    let after = if t.after.is_empty() {
        String::new()
    } else {
        format!("Then: `{}`\n", t.after)
    };
    let scope = if t.scope == "shared" {
        "every project"
    } else {
        "this project only"
    };
    let yes = match t.auth.as_str() {
        "oauth" => format!(
            "A **yes** on this question registers it for {scope}; the sign-in stays yours - **Sign in** on its card under Connections, or `{}` in a terminal.",
            t.after
        ),
        "none" => format!("A **yes** on this question registers it for {scope}."),
        _ => "A **yes** on this question does not register it: the server needs a secret, and only you put one in place; run the lines above when it is.".to_string(),
    };
    format!(
        "`{slug}` shows this: {why}.\n\n\
**{title}**: {about} ([docs]({docs}))\n\n\
Connect it: `{add}`\n{after}\n\
{decline}\n\n\
{yes} The line below is how pulse knows which:\n\n\
{marker} {slug} tool {name}\n",
        slug = t.project,
        why = t.why,
        title = t.title,
        about = t.about,
        docs = t.docs,
        add = t.add,
        decline = decline_lines(&t.project, &format!("tool-{}", t.name), days),
        marker = DECISION_MARKER,
        name = t.name,
    )
}

fn skill_body(k: &SkillSuggestion, days: i32) -> String {
    let named = if k.name == k.recipe {
        String::new()
    } else {
        format!(
            " Another project's skill holds the name `{recipe}`, so this one is registered as `{name}`.",
            recipe = k.recipe,
            name = k.name
        )
    };
    format!(
        "`{slug}` shows this: {why}.\n\n\
A **{recipe}** skill would tell every agent: {description}\n\n\
I drafted it from the checkout (the body is below the line); `pulse toolkit draft {recipe} --project {slug}` prints it alone.{named}\n\n\
Add it: `pulse toolkit add skill {recipe} --project {slug}`\n\n\
{decline}\n\n\
A **yes** on this question adds it as drafted; edit it after with `pulse skill update {name}`. The line below is how pulse knows which:\n\n\
{marker} {slug} skill {recipe}\n\n\
---\n\n{body}",
        slug = k.project,
        why = k.why,
        name = k.name,
        recipe = k.recipe,
        description = k.description,
        decline = decline_lines(&k.project, &skill_thread(k), days),
        marker = DECISION_MARKER,
        body = k.body,
    )
}

fn decision_marker_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(&format!(
            r"(?m)^[ \t]*{}[ \t]+(\S+)[ \t]+(tool|skill)[ \t]+(\S+)[ \t]*$",
            regex::escape(DECISION_MARKER)
        ))
        .expect("toolkit decision marker")
    })
}

/// Adds what the owner said yes to: every decided-yes question in the
/// project's `toolkit` area whose body carries `toolkit: <slug> tool|skill
/// <name>` and that no pass has acted on yet. One look per decision (the
/// `meta` key `toolkit-decided:<post>` remembers it). Returns the daemon's
/// log lines.
pub(crate) fn add_decided_toolkit(
    s: &pulse_store::Store,
    slug: &str,
    now: &str,
    actor: &str,
) -> Vec<String> {
    let Ok(conn) = s.conn() else {
        return Vec::new();
    };
    let decided = must_rows(
        &conn,
        "SELECT d.post_id, d.decided_by, p.body FROM decisions d JOIN posts p ON p.id = d.post_id \
         WHERE p.project = ?1 AND p.area = ?2 AND p.kind = 'question' AND d.answer = 'yes' \
         AND NOT EXISTS (SELECT 1 FROM meta WHERE key = 'toolkit-decided:' || d.post_id) \
         ORDER BY d.decided_at, d.post_id",
        &[&slug, &BOARD_AREA],
    );
    let mut lines = Vec::new();
    for d in decided {
        let post_id = d.str("post_id");
        let body = d.str("body");
        let Some(caps) = decision_marker_re().captures(&body) else {
            continue;
        };
        if &caps[1] != slug {
            continue;
        }
        let kind = caps[2].to_string();
        let name = caps[3].to_string();
        let remember = |note: &str| {
            let _ = conn.execute(
                "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                rusqlite::params![format!("toolkit-decided:{post_id}"), note],
            );
        };
        let shape = shape::read(s, slug);
        let already = match kind.as_str() {
            "tool" => shape.tools.contains(&name),
            _ => skills::pick(&name).is_some_and(|k| skills::present(k, &shape)),
        };
        if already {
            remember("present");
            continue;
        }
        let by = d.str("decided_by");
        let result = match kind.as_str() {
            "tool" => add_tool(s, &by, slug, &name).map(|m| (m.id, m.auth)),
            _ => add_skill(s, &by, slug, &name).map(|k| (k.id, String::new())),
        };
        match result {
            Ok((id, auth)) => {
                remember(&id);
                event_at(
                    &conn,
                    now,
                    actor,
                    "toolkit.decided",
                    &id,
                    json!({"project": slug, "kind": kind, "name": name, "post": post_id}),
                );
                let next = if auth == "oauth" {
                    format!(
                        "; sign in from the hub's Connections page or a terminal: pulse mcp login {name}"
                    )
                } else {
                    String::new()
                };
                lines.push(format!(
                    "added {kind} {name} ({id}) for {slug} on the owner's yes to {post_id}{next}"
                ));
            }
            Err(e) => {
                let why = crate::run::err_message(&e);
                remember(&format!("failed: {why}"));
                // The yes closed the question for the owner; the answer on
                // it is where they learn the add did not happen, and how to
                // do it by hand.
                let add = match kind.as_str() {
                    "tool" => tools::pick(&name).map(|t| tools::add_command(t, slug).0),
                    _ => Some(format!("pulse toolkit add skill {name} --project {slug}")),
                }
                .unwrap_or_default();
                let answer = crate::board::create_post(
                    s,
                    now.to_string(),
                    crate::board::PostOpts {
                        kind: "answer".into(),
                        project: slug.to_string(),
                        area: BOARD_AREA.into(),
                        thread: format!("{kind}-{name}"),
                        summary: trunc(
                            &format!("Could not add the {name} {kind} to {slug} on the yes: {why}"),
                            230,
                        ),
                        body: format!(
                            "The owner said yes to {post_id} and pulse could not act on it: {why}\n\n\
Add it by hand: `{add}`."
                        ),
                        agent: crate::board::actor_agent(s, actor.to_string()),
                        on: post_id.clone(),
                        resolution: "dropped".into(),
                        date: now.to_string(),
                        ..Default::default()
                    },
                );
                if let Err(e) = answer {
                    lines.push(format!(
                        "toolkit for {slug}: the failed yes to {post_id} not posted: {}",
                        crate::run::err_message(&e)
                    ));
                }
                lines.push(format!(
                    "could not add {kind} {name} for {slug} on the owner's yes to {post_id}: {why}"
                ));
            }
        }
    }
    lines
}

// ---- `pulse toolkit` -----------------------------------------------------

pub(crate) fn toolkit_flags(fs: &mut FlagSet) {
    fs.bool_var(
        "all",
        false,
        "show: include the catalog entries no signal called for",
    );
}

#[derive(Serialize)]
struct ProjectOut<'a> {
    project: &'a str,
    tools: &'a [ToolSuggestion],
    skills: &'a [SkillSuggestion],
}

#[derive(Serialize)]
struct OneOut<'a> {
    project: &'a str,
    tools: &'a [ToolSuggestion],
    skills: &'a [SkillSuggestion],
    catalog: Catalog,
}

#[derive(Serialize)]
struct AllOut<'a> {
    projects: Vec<ProjectOut<'a>>,
    catalog: Catalog,
}

fn store(c: &Context) -> Result<&pulse_store::Store> {
    c.store
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("toolkit: store is needed"))
}

fn actor(c: &Context) -> String {
    if c.agent.is_empty() {
        pulse_store::HUMAN_AGENT.into()
    } else {
        c.agent.clone()
    }
}

fn need_project(c: &Context) -> Result<String> {
    if c.project.is_empty() {
        return Err(pulse_cli::errorf(
            2,
            "Missing --project",
            "the toolkit is read per project",
            "pulse toolkit ... --project SLUG",
        )
        .into());
    }
    Ok(c.project.clone())
}

fn known_project(s: &pulse_store::Store, project: &str) -> Result<()> {
    if s.project(project)?.is_none() {
        return Err(pulse_cli::errorf(
            2,
            &format!("No project {project:?}"),
            "it is not registered in this store",
            "pulse projects",
        )
        .into());
    }
    Ok(())
}

fn print_project(
    c: &mut Context,
    project: &str,
    tools: &[ToolSuggestion],
    skills: &[SkillSuggestion],
) -> Result<()> {
    if tools.is_empty() && skills.is_empty() {
        writeln!(
            c.out,
            "{project}: nothing its shape asks for that it does not have (pulse toolkit --all lists the whole catalog)"
        )?;
        return Ok(());
    }
    writeln!(c.out, "{project}")?;
    if !tools.is_empty() {
        writeln!(c.out, "  Tools:")?;
        for t in tools {
            writeln!(c.out, "    {} ({}): {}", t.title, t.name, t.why)?;
            writeln!(c.out, "      {}", t.add)?;
            if !t.after.is_empty() {
                writeln!(c.out, "      then: {}", t.after)?;
            }
        }
    }
    if !skills.is_empty() {
        writeln!(c.out, "  Skills:")?;
        for k in skills {
            writeln!(c.out, "    {}: {}", k.name, k.why)?;
            writeln!(
                c.out,
                "      pulse toolkit add skill {} --project {project}",
                k.name
            )?;
        }
    }
    Ok(())
}

pub(crate) fn cmd_toolkit(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let sub = args.first().map(String::as_str).unwrap_or("show");
    let rest = args.get(1..).unwrap_or(&[]);
    match sub {
        "show" | "" => {
            let s = store(c)?;
            if c.project.is_empty() {
                let projects = s.projects(true)?;
                let per: Vec<(String, Vec<ToolSuggestion>, Vec<SkillSuggestion>)> = projects
                    .iter()
                    .filter(|p| p.slug != pulse_store::COMMONS_SLUG)
                    .map(|p| {
                        let (t, k) = suggestions(s, &p.slug);
                        (p.slug.clone(), t, k)
                    })
                    .collect();
                if c.json {
                    return pulse_cli::write_json(
                        &mut c.out,
                        &AllOut {
                            projects: per
                                .iter()
                                .map(|(p, t, k)| ProjectOut {
                                    project: p,
                                    tools: t,
                                    skills: k,
                                })
                                .collect(),
                            catalog: catalog(),
                        },
                    );
                }
                if per.is_empty() {
                    writeln!(c.out, "No active project; pulse init registers one")?;
                }
                for (p, t, k) in &per {
                    print_project(c, p, t, k)?;
                }
                if fs.get_bool("all") {
                    print_catalog(c)?;
                }
                return Ok(());
            }
            let project = c.project.clone();
            known_project(s, &project)?;
            let (tools, skills) = suggestions(s, &project);
            if c.json {
                return pulse_cli::write_json(
                    &mut c.out,
                    &OneOut {
                        project: &project,
                        tools: &tools,
                        skills: &skills,
                        catalog: catalog(),
                    },
                );
            }
            print_project(c, &project, &tools, &skills)?;
            if fs.get_bool("all") {
                print_catalog(c)?;
            }
            Ok(())
        }
        "add" => {
            let project = need_project(c)?;
            let s = store(c)?;
            known_project(s, &project)?;
            let (kind, name) = kind_and_name(rest)?;
            let by = actor(c);
            match kind {
                "tool" => {
                    let m = add_tool(s, &by, &project, name)?;
                    writeln!(
                        c.out,
                        "{} {} ({}) added",
                        m.id,
                        m.name,
                        if m.project.is_empty() {
                            "every project"
                        } else {
                            m.project.as_str()
                        }
                    )?;
                    if m.auth == "oauth" {
                        writeln!(c.out, "Next: pulse mcp login {}", m.name)?;
                    } else {
                        writeln!(c.out, "Next: pulse mcp connect {}", m.name)?;
                    }
                }
                _ => {
                    let k = add_skill(s, &by, &project, name)?;
                    writeln!(
                        c.out,
                        "{} {} added for {}; edit it with pulse skill update {}",
                        k.id, k.name, k.project, k.name
                    )?;
                }
            }
            Ok(())
        }
        "draft" => {
            let project = need_project(c)?;
            let s = store(c)?;
            known_project(s, &project)?;
            let name = rest.first().map(String::as_str).unwrap_or("");
            if name.is_empty() {
                return Err(pulse_cli::errorf(
                    2,
                    "Missing skill name",
                    "pulse toolkit draft NAME prints the body pulse would add",
                    "pulse toolkit --project SLUG",
                )
                .into());
            }
            let shape = shape::read(s, &project);
            let Some(k) = skills::SKILLS
                .iter()
                .find(|k| k.name == name || skills::draft_name(k, &shape) == name)
            else {
                return Err(pulse_cli::errorf(
                    2,
                    &format!("No skill {name:?} in the catalog"),
                    "pulse toolkit --all lists it",
                    "pulse toolkit --all",
                )
                .into());
            };
            write!(c.out, "{}", (k.body)(&shape))?;
            if !(k.body)(&shape).ends_with('\n') {
                writeln!(c.out)?;
            }
            Ok(())
        }
        other => Err(pulse_cli::errorf(
            2,
            &format!("pulse toolkit: unknown subcommand {other:?}"),
            "see the usage",
            "pulse help toolkit",
        )
        .into()),
    }
}

fn kind_and_name(rest: &[String]) -> Result<(&'static str, &str)> {
    let kind = rest.first().map(String::as_str).unwrap_or("");
    let name = rest.get(1).map(String::as_str).unwrap_or("");
    let kind = match kind {
        "tool" | "mcp" => "tool",
        "skill" => "skill",
        _ => {
            return Err(pulse_cli::errorf(
                2,
                "pulse toolkit add needs tool|skill NAME",
                "the catalog has both",
                "pulse toolkit add tool github --project SLUG",
            )
            .into());
        }
    };
    if name.is_empty() {
        return Err(pulse_cli::errorf(
            2,
            &format!("pulse toolkit add {kind} needs a name"),
            "pulse toolkit --project SLUG lists the ones its shape asks for",
            "pulse toolkit --all",
        )
        .into());
    }
    Ok((kind, name))
}

fn print_catalog(c: &mut Context) -> Result<()> {
    writeln!(c.out, "Catalog:")?;
    for t in tools::TOOLS {
        writeln!(
            c.out,
            "  tool {} ({}, {}{}): {}",
            t.name,
            t.transport,
            t.auth,
            if t.shared { ", every project" } else { "" },
            t.about
        )?;
    }
    for k in skills::SKILLS {
        writeln!(c.out, "  skill {}: {}", k.name, k.description)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{new_bare_repo, new_store, run};
    use crate::test_helpers_daemon::{must_rows, set_setting};
    use crate::test_helpers_provider::{invoke, invoke_ok};

    fn write(root: &str, rel: &str, text: &str) {
        let p = std::path::Path::new(root).join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn names<T>(items: &[T], f: impl Fn(&T) -> &str) -> Vec<String> {
        items.iter().map(|i| f(i).to_string()).collect()
    }

    #[test]
    fn suggestions_read_the_checkout_and_stop_once_the_thing_exists() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (_, root) = new_bare_repo(&s);
        // A bare README: only context7 could fire, and it needs dependencies.
        let (tools, skills) = suggestions(&s, "proj");
        assert!(tools.is_empty(), "{:?}", names(&tools, |t| &t.name));
        assert!(skills.is_empty(), "{:?}", names(&skills, |k| &k.name));

        write(
            &root,
            "package.json",
            r#"{"name":"proj","dependencies":{"next":"16","react":"19","pg":"8","@sentry/nextjs":"8"},"devDependencies":{"@playwright/test":"1"}}"#,
        );
        write(&root, "pnpm-lock.yaml", "lockfileVersion: 9\n");
        write(&root, "prisma/schema.prisma", "datasource db {}\n");
        write(&root, ".github/workflows/ci.yml", "on: push\n");
        write(&root, "Dockerfile", "FROM node:22\n");
        write(&root, "CHANGELOG.md", "# Changelog\n");
        run(
            std::path::Path::new(&root),
            &[
                "git",
                "remote",
                "add",
                "origin",
                "git@github.com:acme/proj.git",
            ],
        );
        let shape = shape::read(&s, "proj");
        assert_eq!(shape.remote_host, "github.com");
        assert_eq!(shape.ui.as_deref(), Some("react"), "{:?}", shape.ui);
        assert!(shape.dep("pg") && shape.dep("@sentry/nextjs"));

        let (tools, skills) = suggestions(&s, "proj");
        let tool_names = names(&tools, |t| &t.name);
        assert_eq!(
            tool_names,
            [
                "github",
                "playwright",
                "context7",
                "postgres",
                "prisma",
                "sentry"
            ],
            "{tool_names:?}"
        );
        let skill_names = names(&skills, |k| &k.name);
        assert_eq!(
            skill_names,
            [
                "dev-setup",
                "ui-check",
                "release-check",
                "db-migrations",
                "containers",
                "ci-failures"
            ],
            "{skill_names:?}"
        );
        let github = &tools[0];
        assert_eq!(github.scope, "shared");
        assert_eq!(
            github.add,
            "pulse mcp add --name github --transport http --url https://api.githubcopilot.com/mcp/ --auth oauth"
        );
        assert_eq!(github.after, "pulse mcp login github");
        let postgres = tools.iter().find(|t| t.name == "postgres").unwrap();
        assert!(postgres.add.ends_with("--project proj"), "{}", postgres.add);
        assert!(postgres.why.contains("pg"), "{}", postgres.why);
        let setup = &skills[0];
        assert!(
            setup.body.contains("pnpm install --frozen-lockfile"),
            "{}",
            setup.body
        );
        assert!(setup.body.contains("pulse env show --project proj"));
        let migrations = skills.iter().find(|k| k.name == "db-migrations").unwrap();
        assert!(
            migrations.body.contains("npx prisma migrate dev"),
            "{}",
            migrations.body
        );

        // Registered once (any status, any scope), a tool is no longer asked for;
        // a skill under an alias counts too, and so does a folder skill.
        add_tool(&s, "human:owner", "proj", "github").unwrap();
        let shared = s.mcp_servers("proj").unwrap();
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].project, "", "github is for every project");
        let sentry = s
            .add_mcp_server(
                "human:owner",
                pulse_store::McpServerInput {
                    name: "sentry".into(),
                    project: "proj".into(),
                    transport: "http".into(),
                    url: "https://mcp.sentry.dev/mcp".into(),
                    auth: "oauth".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        s.set_mcp_server_status("human:owner", &sentry.id, "disabled")
            .unwrap();
        s.add_skill(
            "human:owner",
            pulse_store::SkillInput {
                name: "docker".into(),
                project: String::new(),
                description: "How we build images".into(),
                body: "# Docker\n".into(),
            },
        )
        .unwrap();
        write(&root, ".claude/skills/ci-failures/SKILL.md", "# CI\n");
        let (tools, skills) = suggestions(&s, "proj");
        assert_eq!(
            names(&tools, |t| &t.name),
            ["playwright", "context7", "postgres", "prisma"]
        );
        assert_eq!(
            names(&skills, |k| &k.name),
            ["dev-setup", "ui-check", "release-check", "db-migrations"]
        );
        // A skill name another project holds moves the draft under this one.
        s.add_skill(
            "human:owner",
            pulse_store::SkillInput {
                name: "ui-check".into(),
                project: pulse_store::COMMONS_SLUG.into(),
                description: "theirs".into(),
                body: "# theirs\n".into(),
            },
        )
        .unwrap();
        let (_, skills) = suggestions(&s, "proj");
        assert!(
            names(&skills, |k| &k.name).contains(&"proj-ui-check".to_string()),
            "{:?}",
            names(&skills, |k| &k.name)
        );
        let added = add_skill(&s, "human:owner", "proj", "proj-ui-check").unwrap();
        assert_eq!(added.name, "proj-ui-check");
        assert_eq!(added.project, "proj");
        assert!(added.body.contains("UI check for proj"));
        let (_, skills) = suggestions(&s, "proj");
        assert!(
            !names(&skills, |k| &k.name)
                .iter()
                .any(|n| n.contains("ui-check"))
        );
        // A secret-bearing tool is never registered on the owner's behalf.
        let err = add_tool(&s, "human:owner", "proj", "postgres").unwrap_err();
        assert!(err.to_string().contains("needs a secret"), "{err}");
    }

    #[test]
    fn daemon_asks_one_question_at_a_time_and_a_yes_adds_it() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (_, root) = new_bare_repo(&s);
        // The store's clock stands still in tests, so the hourly memo is
        // cleared by hand between passes instead of waiting an hour.
        let now = s.now();
        let forget_memo = || {
            let conn = s.conn().unwrap();
            conn.execute("DELETE FROM meta WHERE key = 'toolkit-suggest:proj'", [])
                .unwrap();
        };
        assert!(
            suggest_toolkit(&s, "proj", &now, "daemon").is_empty(),
            "nothing to ask for"
        );
        write(
            &root,
            "package.json",
            r#"{"dependencies":{"next":"16","react":"19","zod":"3"}}"#,
        );
        run(
            std::path::Path::new(&root),
            &[
                "git",
                "remote",
                "add",
                "origin",
                "https://github.com/acme/proj.git",
            ],
        );
        forget_memo();
        let lines = suggest_toolkit(&s, "proj", &now, "daemon");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("asked the owner about tool-github for proj"),
            "{lines:?}"
        );
        let posts = must_rows(
            &s,
            "SELECT id, kind, area, thread, summary, body FROM posts WHERE project = 'proj'",
            &[],
        );
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].str("kind"), "question");
        assert_eq!(
            (
                posts[0].str("area").as_str(),
                posts[0].str("thread").as_str()
            ),
            ("toolkit", "tool-github")
        );
        assert!(
            posts[0]
                .str("summary")
                .starts_with("Connect GitHub to proj?"),
            "{}",
            posts[0].str("summary")
        );
        let body = posts[0].str("body");
        assert!(body.contains("pulse mcp add --name github"), "{body}");
        assert!(body.contains("\ntoolkit: proj tool github\n"), "{body}");
        // The hourly memo holds a repeat; after it, the open question holds
        // the rest (playwright, context7, dev-setup, ui-check wait).
        assert!(suggest_toolkit(&s, "proj", &now, "daemon").is_empty());
        forget_memo();
        let lines = suggest_toolkit(&s, "proj", &now, "daemon");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("waiting on"), "{lines:?}");
        // A yes is for the daemon, not a worker.
        let decision =
            pulse_board::jobs::decide(&s, &posts[0].str("id"), "yes", "", pulse_store::HUMAN_AGENT)
                .unwrap();
        assert!(decision.job.is_none(), "{:?}", decision.job);
        let lines = add_decided_toolkit(&s, "proj", &now, "daemon");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("added tool github (MCP-001) for proj on the owner's yes to "),
            "{lines:?}"
        );
        assert!(
            lines[0].ends_with(
                "sign in from the hub's Connections page or a terminal: pulse mcp login github"
            ),
            "{lines:?}"
        );
        let github = s.mcp_server("", "github").unwrap();
        assert_eq!(github.created_by, pulse_store::HUMAN_AGENT);
        assert_eq!(github.auth, "oauth");
        assert_eq!(github.project, "");
        assert!(
            add_decided_toolkit(&s, "proj", &now, "daemon").is_empty(),
            "a yes adds once"
        );
        let events = must_rows(
            &s,
            "SELECT subject, detail FROM events WHERE action = 'toolkit.decided'",
            &[],
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].str("subject"), "MCP-001");
        // The decided question is closed, so the next pass asks the next
        // thing: playwright. Declined, it rests for the window and the pass
        // after moves on to context7.
        forget_memo();
        let lines = suggest_toolkit(&s, "proj", &now, "daemon");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("asked the owner about tool-playwright"),
            "{lines:?}"
        );
        let decline = |thread: &str| {
            pulse_board::threads::resolve_thread(
                &s,
                pulse_board::threads::ResolveInput {
                    project: "proj".into(),
                    key: format!("toolkit/{thread}"),
                    resolution: "declined".into(),
                    summary: "not here".into(),
                    body: String::new(),
                    agent: pulse_store::HUMAN_AGENT.into(),
                    session: String::new(),
                },
            )
            .unwrap();
        };
        decline("tool-playwright");
        forget_memo();
        let lines = suggest_toolkit(&s, "proj", &now, "daemon");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("asked the owner about tool-context7"),
            "{lines:?}"
        );
        // A skill question's yes adds the drafted body.
        decline("tool-context7");
        forget_memo();
        let lines = suggest_toolkit(&s, "proj", &now, "daemon");
        assert!(
            lines[0].starts_with("asked the owner about skill-dev-setup"),
            "{lines:?}"
        );
        let q = must_rows(
            &s,
            "SELECT id, body FROM posts WHERE project = 'proj' AND thread = 'skill-dev-setup' AND kind = 'question'",
            &[],
        )
        .remove(0);
        assert!(
            q.str("body").contains("\ntoolkit: proj skill dev-setup\n"),
            "{}",
            q.str("body")
        );
        assert!(q.str("body").contains("# Dev setup for proj"));
        pulse_board::jobs::decide(&s, &q.str("id"), "yes", "", pulse_store::HUMAN_AGENT).unwrap();
        let lines = add_decided_toolkit(&s, "proj", &now, "daemon");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("added skill dev-setup (SKILL-001) for proj"),
            "{lines:?}"
        );
        let k = s.skill("dev-setup").unwrap();
        assert_eq!(k.project, "proj");
        assert!(k.body.contains("npm ci"), "{}", k.body);
        // Declined threads rest for the window; after it they are asked again.
        forget_memo();
        let lines = suggest_toolkit(&s, "proj", &now, "daemon");
        assert!(
            lines[0].starts_with("asked the owner about skill-ui-check"),
            "{lines:?}"
        );
        decline("skill-ui-check");
        forget_memo();
        assert!(
            suggest_toolkit(&s, "proj", &now, "daemon").is_empty(),
            "everything asked about inside the window"
        );
        forget_memo();
        let lines = suggest_toolkit(&s, "proj", &plus_hours(&now, 24.0 * 31.0), "daemon");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("asked the owner about tool-playwright"),
            "{lines:?}"
        );
        set_setting(&s, "toolkit.suggest_days", "0");
        forget_memo();
        assert!(suggest_toolkit(&s, "proj", &plus_hours(&now, 24.0 * 40.0), "daemon").is_empty());
    }

    #[test]
    fn while_the_owner_is_away_the_toolkit_is_added_without_a_question() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (_, root) = new_bare_repo(&s);
        let now = s.now();
        let forget_memo = || {
            let conn = s.conn().unwrap();
            conn.execute("DELETE FROM meta WHERE key = 'toolkit-suggest:proj'", [])
                .unwrap();
        };
        // Another project holds the `dev-setup` name, so proj's draft falls
        // back to `proj-dev-setup`; it also holds `ui-check` and the
        // fallback `proj-ui-check`, so that add has nowhere to go.
        {
            let conn = s.conn().unwrap();
            conn.execute(
                "INSERT INTO projects (slug, name, path, remote, brain, verify_cmd, default_branch, status, added_at, added_by, last_seen) VALUES ('other', 'Other', '', '', 0, '', 'main', 'active', ?1, ?2, ?1)",
                rusqlite::params![now, pulse_store::HUMAN_AGENT],
            )
            .unwrap();
        }
        for name in ["dev-setup", "ui-check", "proj-ui-check"] {
            s.add_skill(
                pulse_store::HUMAN_AGENT,
                pulse_store::SkillInput {
                    name: name.into(),
                    project: "other".into(),
                    description: format!("Other's {name}"),
                    body: "# Other\n".into(),
                },
            )
            .unwrap();
        }
        write(
            &root,
            "package.json",
            r#"{"dependencies":{"next":"16","react":"19","zod":"3","pg":"8"}}"#,
        );
        run(
            std::path::Path::new(&root),
            &[
                "git",
                "remote",
                "add",
                "origin",
                "https://github.com/acme/proj.git",
            ],
        );
        // Present, the daemon asks; the question is open when the owner
        // leaves.
        let lines = suggest_toolkit(&s, "proj", &now, "daemon");
        assert!(
            lines[0].starts_with("asked the owner about tool-github"),
            "{lines:?}"
        );
        let asked = must_rows(
            &s,
            "SELECT id FROM posts WHERE project = 'proj' AND kind = 'question'",
            &[],
        );
        assert_eq!(asked.len(), 1);
        let question = asked[0].str("id");
        // A question a lead merely answered (nothing was added on it) is
        // not an answer the daemon honours; a thread the owner declined
        // rests for the window.
        let playwright = crate::hygiene::post_daemon_owner_question(
            &s,
            crate::hygiene::DaemonQuestionAsk {
                project: "proj".into(),
                ask_kind: ASK_KIND.into(),
                area: BOARD_AREA.into(),
                thread: "tool-playwright".into(),
                summary: "Connect Playwright to proj?".into(),
                body: "asked earlier".into(),
                actor: "daemon".into(),
                now: now.clone(),
            },
        )
        .unwrap()
        .unwrap();
        crate::board::create_post(
            &s,
            now.clone(),
            crate::board::PostOpts {
                kind: "answer".into(),
                project: "proj".into(),
                area: BOARD_AREA.into(),
                thread: "tool-playwright".into(),
                summary: "Decision by lead (owner away): yes".into(),
                body: "yes".into(),
                agent: crate::board::actor_agent(&s, "proj-lead".into()),
                on: playwright.str("id"),
                date: now.clone(),
                ..Default::default()
            },
        )
        .unwrap();
        let context7 = crate::hygiene::post_daemon_owner_question(
            &s,
            crate::hygiene::DaemonQuestionAsk {
                project: "proj".into(),
                ask_kind: ASK_KIND.into(),
                area: BOARD_AREA.into(),
                thread: "tool-context7".into(),
                summary: "Connect Context7 to proj?".into(),
                body: "asked earlier".into(),
                actor: "daemon".into(),
                now: now.clone(),
            },
        )
        .unwrap()
        .unwrap();
        pulse_board::jobs::decide(&s, &context7.str("id"), "no", "", pulse_store::HUMAN_AGENT)
            .unwrap();
        set_setting(&s, "owner.presence", "away");
        forget_memo();
        let lines = suggest_toolkit(&s, "proj", &now, "daemon");
        assert_eq!(
            lines,
            vec![
                "added tool github (MCP-001) for proj while the owner is away; it waits for a sign-in: pulse mcp login github".to_string(),
                "added tool playwright (MCP-002) for proj while the owner is away".into(),
                "added skill dev-setup (SKILL-004) for proj while the owner is away".into(),
                "could not add skill ui-check for proj while the owner is away: Invalid name proj-ui-check: SKILL-003 already uses it; pick another or update that one (pulse skill update proj-ui-check)".into(),
            ]
        );
        assert!(
            s.mcp_server("proj", "context7").is_err(),
            "a declined thread rests"
        );
        assert_eq!(s.mcp_server("", "github").unwrap().created_by, "daemon");
        assert_eq!(s.mcp_server("proj", "playwright").unwrap().auth, "none");
        let k = s.skill("proj-dev-setup").unwrap();
        assert_eq!(
            (k.project.as_str(), k.created_by.as_str()),
            ("proj", "daemon")
        );
        assert!(k.body.contains("npm ci"), "{}", k.body);
        assert!(
            s.mcp_server("proj", "postgres").is_err(),
            "a tool that needs a secret is the owner's"
        );
        let posts = must_rows(
            &s,
            "SELECT kind, thread, on_post, resolution, summary, body FROM posts WHERE project = 'proj' AND agent = 'daemon' AND kind <> 'question' ORDER BY id",
            &[],
        );
        let shape: Vec<(String, String, String, String)> = posts
            .iter()
            .map(|p| {
                (
                    p.str("kind"),
                    p.str("thread"),
                    p.str("on_post"),
                    p.str("resolution"),
                )
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                (
                    "answer".into(),
                    "tool-github".into(),
                    question.clone(),
                    "proceeded".into()
                ),
                (
                    "result".into(),
                    "tool-playwright".into(),
                    String::new(),
                    "proceeded".into()
                ),
                (
                    "result".into(),
                    "skill-dev-setup".into(),
                    String::new(),
                    "proceeded".into()
                ),
                (
                    "result".into(),
                    "skill-ui-check".into(),
                    String::new(),
                    "dropped".into()
                ),
            ]
        );
        assert!(
            posts[0]
                .str("summary")
                .starts_with("Connected GitHub to proj while the owner is away:"),
            "{}",
            posts[0].str("summary")
        );
        assert!(
            posts[0].str("body").contains("pulse mcp login github"),
            "{}",
            posts[0].str("body")
        );
        assert!(
            posts[2]
                .str("summary")
                .starts_with("Added a dev-setup skill to proj while the owner is away:"),
            "{}",
            posts[2].str("summary")
        );
        let body = posts[2].str("body");
        assert!(body.contains("owner.presence = away"), "{body}");
        assert!(body.contains("Undo it: `pulse skill disable proj-dev-setup; edit it: pulse skill update proj-dev-setup`"), "{body}");
        assert!(
            posts[3]
                .str("summary")
                .starts_with("Could not add the ui-check skill to proj on its own:"),
            "{}",
            posts[3].str("summary")
        );
        assert!(
            posts[3]
                .str("body")
                .contains("`pulse toolkit add skill proj-ui-check --project proj"),
            "{}",
            posts[3].str("body")
        );
        // The question the owner was asked while present is closed by the
        // answer.
        assert!(crate::hygiene::open_daemon_questions_of_kind(&s, "proj", ASK_KIND).is_empty());
        let events = must_rows(
            &s,
            "SELECT subject, detail FROM events WHERE action = 'toolkit.added' ORDER BY subject",
            &[],
        );
        assert_eq!(
            names(&events, |e| e.0.get("subject").unwrap().as_str().unwrap()),
            ["MCP-001", "MCP-002", "SKILL-004"]
        );
        let detail: serde_json::Value = serde_json::from_str(&events[2].str("detail")).unwrap();
        assert_eq!(detail["name"], "dev-setup");
        assert_eq!(detail["owner"], "away");
        // Nothing is asked, and a failing add is reported once per window.
        forget_memo();
        assert!(suggest_toolkit(&s, "proj", &now, "daemon").is_empty());
        assert_eq!(
            crate::test_helpers::count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE project = 'proj' AND kind = 'question'",
                &[]
            ),
            3
        );
        forget_memo();
        let lines = suggest_toolkit(&s, "proj", &plus_hours(&now, 24.0 * 31.0), "daemon");
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines[0].starts_with("added tool context7 (MCP-003) for proj while the owner is away"),
            "{lines:?}"
        );
        assert!(
            lines[1].starts_with("could not add skill ui-check"),
            "{lines:?}"
        );
        assert_eq!(
            crate::test_helpers::count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE project = 'proj' AND thread = 'skill-ui-check'",
                &[]
            ),
            2
        );
        // Back, the owner is asked again - about what is left: the tool that
        // needed their secret, then the skill that would not add.
        set_setting(&s, "owner.presence", "present");
        forget_memo();
        let lines = suggest_toolkit(&s, "proj", &plus_hours(&now, 24.0 * 62.0), "daemon");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("asked the owner about tool-postgres"),
            "{lines:?}"
        );
    }

    #[test]
    fn toolkit_command_shows_adds_and_drafts() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (_, root) = new_bare_repo(&s);
        write(
            &root,
            "Cargo.toml",
            "[package]\nname = \"proj\"\n\n[dependencies]\nsqlx = \"0.8\"\n",
        );
        write(
            &root,
            "migrations/0001_init.sql",
            "create table t (id int);\n",
        );
        let out = invoke_ok(&s, "toolkit", "proj", &[]);
        assert!(
            out.contains("proj\n  Tools:\n    Postgres (postgres): It talks to Postgres (sqlx)"),
            "{out}"
        );
        assert!(out.contains("  Skills:\n    dev-setup:"), "{out}");
        assert!(
            out.contains("    db-migrations: It keeps migrations in migrations (sqlx)"),
            "{out}"
        );
        let out = invoke_ok(&s, "toolkit", "", &[]);
        assert!(
            out.starts_with("proj\n  Tools:\n"),
            "every active project without --project: {out}"
        );
        let (tools, skills) = suggestions(&s, "proj");
        let v = serde_json::to_value(OneOut {
            project: "proj",
            tools: &tools,
            skills: &skills,
            catalog: catalog(),
        })
        .unwrap();
        assert_eq!(v["tools"][0]["name"], "postgres");
        assert_eq!(v["tools"][0]["scope"], "project");
        assert!(v["catalog"]["tools"].as_array().unwrap().len() >= 20);
        assert!(v["catalog"]["skills"].as_array().unwrap().len() >= 8);
        let out = invoke_ok(&s, "toolkit", "proj", &["draft", "db-migrations"]);
        assert!(out.starts_with("# Database migrations for proj"), "{out}");
        assert!(out.contains("sqlx migrate add -r"), "{out}");
        let out = invoke_ok(&s, "toolkit", "proj", &["add", "skill", "db-migrations"]);
        assert!(
            out.starts_with("SKILL-001 db-migrations added for proj"),
            "{out}"
        );
        let (_, err, code) = invoke(&s, "toolkit", "proj", &["add", "tool", "postgres"]);
        assert_eq!(code, 1, "{err}");
        assert!(err.contains("needs a secret"), "{err}");
        let (_, err, code) = invoke(&s, "toolkit", "", &["add", "tool", "github"]);
        assert_eq!(code, 2);
        assert!(err.contains("Missing --project"), "{err}");
        let (_, err, code) = invoke(&s, "toolkit", "proj", &["add", "tool", "nope"]);
        assert_eq!(code, 1);
        assert!(err.contains("No tool \"nope\""), "{err}");
        let out = invoke_ok(&s, "toolkit", "proj", &[]);
        assert!(!out.contains("db-migrations:"), "{out}");
        let out = invoke_ok(&s, "toolkit", "proj", &["--all"]);
        assert!(
            out.contains("Catalog:\n  tool github (http, oauth, every project)"),
            "{out}"
        );
    }
}
