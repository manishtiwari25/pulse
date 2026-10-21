//! Ask: a quick answer about a project, from the store, with cited sources.
//!
//! The question is answered by the harness's model from a compact view of
//! the store ([`context`]) and nothing else; every id the answer leans on is
//! returned as a source so a console or terminal can link it. Without a
//! signed-in model the matching facts are returned as they are
//! (`facts_only`). Ask never changes anything in the store beyond one
//! `ask` event with the question's size and timing.

use anyhow::Result;
use pulse_provider::{CancelToken, Message, Provider, Request, Role, Usage};
use pulse_store::Store;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

mod command;
pub mod context;

pub use command::register;
pub use context::{Context, Item, build as context};

/// The most characters a question may have.
pub const MAX_QUESTION: usize = 2000;
/// How many earlier turns the model sees.
pub const HISTORY_TURNS: usize = 6;
/// How long one model call may take by default.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_TOKENS: i32 = 700;
const MAX_SOURCES: usize = 8;

/// One earlier turn of the conversation, as the console keeps it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    /// `user` or `assistant`.
    pub role: String,
    pub content: String,
}

/// One question.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskRequest {
    /// The project slug; empty with `all_projects` means every project.
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub all_projects: bool,
    pub question: String,
    #[serde(default)]
    pub history: Vec<Turn>,
    /// A model id; empty means the `ask.model` setting, then the light model.
    #[serde(default)]
    pub model: String,
    /// Who asks, for the event log (`human:console`, `human:cli`); never read from JSON.
    #[serde(skip)]
    pub actor: String,
}

/// One answer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AskAnswer {
    pub answer: String,
    /// The project answered about, or empty for every project.
    pub project: String,
    /// The model that answered; empty when no model did.
    pub model: String,
    /// The provider's registry name; empty when no model was tried.
    pub provider: String,
    /// True when the answer is the matching facts themselves, not a model's words.
    pub facts_only: bool,
    /// Why the answer is facts only, in plain words; empty otherwise.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    pub sources: Vec<Item>,
    pub elapsed_ms: u64,
    pub usage: Usage,
}

/// The question, trimmed, or why it cannot be answered (a 400 for the hub).
pub fn check_question(q: &str) -> std::result::Result<&str, String> {
    let q = q.trim();
    if q.is_empty() {
        return Err("the question is empty".into());
    }
    if q.chars().count() > MAX_QUESTION {
        return Err(format!(
            "the question is longer than {MAX_QUESTION} characters"
        ));
    }
    Ok(q)
}

/// The model Ask answers with: the request's, else the `ask.model` setting,
/// else the light model (which falls back to the worker model), else the
/// provider's default chat model.
pub fn choose_model(
    s: &Store,
    p: &dyn Provider,
    cancel: &CancelToken,
    requested: &str,
) -> Result<String> {
    for m in [
        requested.trim(),
        s.setting("ask.model", "").trim(),
        s.light_model().trim(),
    ] {
        if !m.is_empty() {
            return Ok(m.to_string());
        }
    }
    let models = p.models(cancel)?;
    if let Some(m) = models
        .iter()
        .find(|m| m.default && m.chat)
        .or_else(|| models.iter().find(|m| m.chat))
    {
        return Ok(m.id.clone());
    }
    anyhow::bail!("no chat model is available; set ask.model or light_model")
}

/// Answer `req` from `s`, through `provider` when there is one.
///
/// Fails only when the question is invalid, the project is unknown, or the
/// store cannot be read; a model that cannot be reached (not signed in,
/// vendor error, timeout) makes the answer facts only instead.
pub fn ask(
    s: &Store,
    provider: Option<&dyn Provider>,
    req: &AskRequest,
    timeout: Duration,
) -> Result<AskAnswer> {
    let started = Instant::now();
    let question = check_question(&req.question).map_err(|why| anyhow::anyhow!("{why}"))?;
    let project = if req.all_projects {
        None
    } else {
        let slug = req.project.trim();
        if slug.is_empty() {
            anyhow::bail!("no project named; pass a project slug or ask about all projects");
        }
        if slug != pulse_store::COMMONS_SLUG && s.project(slug)?.is_none() {
            return Err(pulse_tally::NoProjectError { slug: slug.into() }.into());
        }
        Some(slug)
    };
    let cx = context::build(s, project, question)?;

    let mut out = AskAnswer {
        project: cx.project.clone(),
        ..AskAnswer::default()
    };
    match provider {
        None => {
            out.facts_only = true;
            out.note = "no model was asked".into();
        }
        Some(p) => {
            out.provider = p.name().to_string();
            let cancel = CancelToken::new().with_timeout(timeout);
            match choose_model(s, p, &cancel, &req.model).and_then(|model| {
                let resp = p.chat(&cancel, request(&model, &cx, &req.history, question))?;
                Ok((model, resp))
            }) {
                Ok((model, resp)) => {
                    out.model = if resp.model.is_empty() {
                        model
                    } else {
                        resp.model
                    };
                    out.usage = resp.usage;
                    out.answer = resp
                        .message
                        .map(|m| m.content)
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    if out.answer.is_empty() {
                        out.facts_only = true;
                        out.note = "the model returned no words".into();
                    }
                }
                Err(e) => {
                    out.facts_only = true;
                    out.note = if pulse_provider::is_not_logged_in(&e) {
                        "not signed in".to_string()
                    } else if cancel.is_cancelled() {
                        format!(
                            "the model did not answer within {} seconds",
                            timeout.as_secs()
                        )
                    } else {
                        context::clip(&format!("the model call failed: {e:#}"), 240)
                    };
                }
            }
        }
    }
    out.sources = cited(&out.answer, &cx);
    if out.facts_only {
        out.answer = facts_answer(&cx, &out.note);
        if out.sources.is_empty() {
            out.sources = cx.items[cx.matched_from..]
                .iter()
                .take(MAX_SOURCES)
                .cloned()
                .collect();
        }
    }
    out.elapsed_ms = started.elapsed().as_millis() as u64;
    if !req.actor.is_empty() {
        // Metadata only: the question itself stays with the one who asked.
        let _ = s.event(
            &req.actor,
            "ask",
            &out.project,
            serde_json::json!({
                "question_chars": question.chars().count(),
                "model": out.model,
                "facts_only": out.facts_only,
                "elapsed_ms": out.elapsed_ms,
                "sources": out.sources.len(),
            }),
        );
    }
    Ok(out)
}

const RULES: &str = "You are Ask, the quick answer box of PULSE: a system where AI agents work on software projects from a shared board and a human owner decides. You answer the owner's question about their project from the FACTS below and nothing else.

Rules:
- Plain words, briefly: lead with the answer, then the few facts that support it. Short paragraphs or a short list; no headings, no preamble, no repeating the question.
- Use only the facts given. When they do not hold the answer, say so plainly and name where to look (the board thread, the Jobs, Runs or Tasks page, or a pulse command). Never guess ids, dates, counts or names.
- Cite what you rely on by writing its id in square brackets exactly as it appears in the facts, e.g. [20261012T164522Z-copilot-fable-1012-hub], [ADR-024], [TASK-001] or [job-20261013T033847Z-63fc3b]. At most five citations.
- Dates in the facts are UTC; \"Now\" is given at the top.
- You cannot change anything. When asked to act, say what the owner can do and where.
- The owner may read this on a phone: stay under 150 words unless a list is really needed.";

fn request(model: &str, cx: &Context, history: &[Turn], question: &str) -> Request {
    let mut system = Message::new(Role::System);
    system.content = format!("{RULES}\n\nFACTS:\n{}", cx.text);
    let mut messages = vec![system];
    let skip = history.len().saturating_sub(HISTORY_TURNS);
    for t in &history[skip..] {
        let role = match t.role.as_str() {
            "assistant" => Role::Assistant,
            _ => Role::User,
        };
        let mut m = Message::new(role);
        m.content = context::clip(&t.content, 2000);
        if !m.content.is_empty() {
            messages.push(m);
        }
    }
    let mut user = Message::new(Role::User);
    user.content = question.to_string();
    messages.push(user);
    Request {
        model: model.to_string(),
        messages,
        tools: vec![],
        max_tokens: MAX_TOKENS,
        temperature: Some(0.2),
        initiator: "ask".into(),
        reasoning: String::new(),
    }
}

/// The context items the answer names, in the order it names them.
fn cited(answer: &str, cx: &Context) -> Vec<Item> {
    if answer.is_empty() {
        return vec![];
    }
    let mut found: Vec<(usize, &Item)> = cx
        .items
        .iter()
        .filter_map(|it| answer.find(&it.id).map(|at| (at, it)))
        .collect();
    found.sort_by_key(|(at, _)| *at);
    let mut out: Vec<Item> = Vec::new();
    for (_, it) in found {
        if !out.iter().any(|x| x.id == it.id) {
            out.push(it.clone());
        }
        if out.len() == MAX_SOURCES {
            break;
        }
    }
    out
}

/// The answer when no model speaks: the headlines, then what matched.
fn facts_answer(cx: &Context, note: &str) -> String {
    let mut s = String::new();
    if !note.is_empty() {
        s.push_str(&format!(
            "No model answered ({note}), so here are the facts that match.\n\n"
        ));
    }
    for h in &cx.headlines {
        s.push_str("- ");
        s.push_str(h);
        s.push('\n');
    }
    let matched = &cx.items[cx.matched_from..];
    if matched.is_empty() {
        s.push_str("\nNothing in the store mentions the question's words.");
    } else {
        s.push_str("\nMatching the question:\n");
        for it in matched.iter().take(MAX_SOURCES) {
            s.push_str(&format!(
                "- [{}] {} {}: {}\n",
                it.id,
                it.kind,
                &it.date[..it.date.len().min(10)],
                context::clip(&it.summary, 160)
            ));
        }
    }
    s.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_provider_fake::{Reply, Script};
    use pulse_store::Options;
    use tempfile::TempDir;

    const NOW: &str = "2026-10-13T06:00:00Z";

    fn store() -> (TempDir, Store) {
        let home = tempfile::tempdir().unwrap();
        let s = Store::open(Options {
            home: home.path().display().to_string(),
            create: true,
            now: NOW.into(),
            ..Options::default()
        })
        .unwrap();
        let conn = s.conn().unwrap();
        conn.execute(
            "INSERT INTO projects (slug,name,path,added_at,status,default_branch) VALUES ('demo','Demo',?,?, 'active','main')",
            rusqlite::params![home.path().join("demo").display().to_string(), s.now()],
        )
        .unwrap();
        for (id, kind, thread, summary, body, date) in [
            (
                "20261012T100000Z-nimbus-1",
                "finding",
                "hub/memory",
                "The hub keeps the whole board in RAM",
                "The tally cache holds every post; 2.1 GB after a day. Fix: compact rows.",
                "2026-10-12T10:00:00Z",
            ),
            (
                "20261012T110000Z-nimbus-2",
                "suggestion",
                "hub/memory",
                "Compact hub rows to cut memory",
                "Memory leak suspected in the hub tally cache; measured by RSS after 24h.",
                "2026-10-12T11:00:00Z",
            ),
            (
                "20261011T090000Z-pallas-1",
                "result",
                "runner/disk",
                "Disk reclaim ladder landed",
                "Temp dirs are cleaned when disk is low.",
                "2026-10-11T09:00:00Z",
            ),
        ] {
            conn.execute(
                "INSERT INTO posts (id,project,area,thread,kind,agent,date,summary,body,created_at) VALUES (?,'demo','hub',?,?,'nimbus',?,?,?,?)",
                rusqlite::params![id, thread, kind, date, summary, body, date],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO jobs (id,project,kind,title,status,created_at,result) VALUES ('job-1','demo','task','Fix the memory leak in the hub','blocked',?,'needs the owner: which cache to drop')",
            [s.now()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id,project,title,status,created_by,created_at,updated_at) VALUES ('TASK-001','demo','Ask panel for the console','open','human',?,?)",
            [s.now(), s.now()],
        )
        .unwrap();
        drop(conn);
        (home, s)
    }

    #[test]
    fn context_holds_project_facts_and_matches() {
        let (_h, s) = store();
        let cx =
            context::build(&s, Some("demo"), "what about the memory leak in the hub?").unwrap();
        assert!(cx.text.contains("## Project demo"), "{}", cx.text);
        assert!(cx.text.contains("[job-1] task"), "{}", cx.text);
        assert!(cx.text.contains("[TASK-001] open"), "{}", cx.text);
        assert!(
            cx.text
                .contains("## Matching the question's words (memory, leak, hub)"),
            "{}",
            cx.text
        );
        let matched: Vec<&str> = cx.items[cx.matched_from..]
            .iter()
            .map(|i| i.id.as_str())
            .collect();
        assert_eq!(
            matched[0], "20261012T110000Z-nimbus-2",
            "both words match the suggestion first: {matched:?}"
        );
        assert!(matched.contains(&"20261012T100000Z-nimbus-1"));
        assert!(!matched.contains(&"20261011T090000Z-pallas-1"));
        assert!(
            cx.headlines
                .iter()
                .any(|h| h.starts_with("Jobs by status: blocked 1")),
            "{:?}",
            cx.headlines
        );
        assert!(cx.text.chars().count() <= context::MAX_CHARS);
    }

    #[test]
    fn context_for_every_project_lists_them() {
        let (_h, s) = store();
        let cx = context::build(&s, None, "disk").unwrap();
        assert!(cx.text.contains("## Projects (1)"));
        assert!(cx.text.contains("- demo (Demo): status active"));
        assert!(
            cx.items[cx.matched_from..]
                .iter()
                .any(|i| i.id == "20261011T090000Z-pallas-1")
        );
        assert_eq!(cx.project, "");
    }

    #[test]
    fn model_answer_carries_cited_sources_and_model() {
        let (_h, s) = store();
        let fake = pulse_provider_fake::new(Script {
            replies: vec![Reply {
                content: "The hub keeps the board in RAM [20261012T100000Z-nimbus-1]; a fix is suggested [20261012T110000Z-nimbus-2] and the job waits on you [job-1].".into(),
                ..Reply::default()
            }],
            ..Script::default()
        });
        let req = AskRequest {
            project: "demo".into(),
            question: "what is up with memory in the hub?".into(),
            history: vec![
                Turn {
                    role: "user".into(),
                    content: "hello".into(),
                },
                Turn {
                    role: "assistant".into(),
                    content: "hi".into(),
                },
            ],
            model: "fake-chat".into(),
            actor: "human:cli".into(),
            ..AskRequest::default()
        };
        let a = ask(&s, Some(&fake), &req, DEFAULT_TIMEOUT).unwrap();
        assert!(!a.facts_only, "{a:?}");
        assert_eq!(a.model, "fake-chat");
        assert_eq!(a.provider, "fake");
        assert_eq!(
            a.sources.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec![
                "20261012T100000Z-nimbus-1",
                "20261012T110000Z-nimbus-2",
                "job-1"
            ]
        );
        assert_eq!(a.sources[2].source, "job");
        let reqs = fake.requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].initiator, "ask");
        assert_eq!(
            reqs[0].messages.len(),
            4,
            "system, two history turns, the question"
        );
        assert!(
            reqs[0].messages[0]
                .content
                .contains("FACTS:\nNow: 2026-10-13T06:00:00Z")
        );
        assert_eq!(
            reqs[0].messages[3].content,
            "what is up with memory in the hub?"
        );
        let events: Vec<(String, String)> = s
            .conn()
            .unwrap()
            .prepare("SELECT actor, detail FROM events WHERE action = 'ask'")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "human:cli");
        assert!(events[0].1.contains("question_chars"), "{}", events[0].1);
        assert!(
            !events[0].1.contains("what is up"),
            "the question itself is not logged"
        );
    }

    #[test]
    fn without_a_model_the_matching_facts_answer() {
        let (_h, s) = store();
        let req = AskRequest {
            project: "demo".into(),
            question: "memory leak in the hub?".into(),
            ..AskRequest::default()
        };
        let a = ask(&s, None, &req, DEFAULT_TIMEOUT).unwrap();
        assert!(a.facts_only);
        assert_eq!(a.note, "no model was asked");
        assert!(
            a.answer
                .starts_with("No model answered (no model was asked)"),
            "{}",
            a.answer
        );
        assert!(
            a.answer.contains("- Jobs by status: blocked 1."),
            "{}",
            a.answer
        );
        assert!(
            a.answer
                .contains("[20261012T110000Z-nimbus-2] suggestion 2026-10-12:"),
            "{}",
            a.answer
        );
        assert_eq!(a.sources.len(), 2);
        assert!(a.model.is_empty() && a.provider.is_empty());
    }

    struct LoggedOut;

    impl Provider for LoggedOut {
        fn name(&self) -> &str {
            "loggedout"
        }
        fn label(&self) -> &str {
            "Logged out"
        }
        fn chat(&self, _cancel: &CancelToken, _req: Request) -> Result<pulse_provider::Response> {
            Err(pulse_provider::err_not_logged_in().into())
        }
        fn models(&self, _cancel: &CancelToken) -> Result<Vec<pulse_provider::Model>> {
            Err(pulse_provider::err_not_logged_in().into())
        }
        fn status(&self, _cancel: &CancelToken) -> Result<pulse_provider::Login> {
            Ok(pulse_provider::Login::default())
        }
    }

    #[test]
    fn a_missing_login_falls_back_to_facts() {
        let (_h, s) = store();
        let req = AskRequest {
            project: "demo".into(),
            question: "disk?".into(),
            ..AskRequest::default()
        };
        let a = ask(&s, Some(&LoggedOut), &req, DEFAULT_TIMEOUT).unwrap();
        assert!(a.facts_only);
        assert_eq!(
            a.note, "not signed in",
            "the model list is asked first and is refused too"
        );
        assert_eq!(a.provider, "loggedout");
        assert_eq!(a.sources[0].id, "20261011T090000Z-pallas-1");
    }

    #[test]
    fn a_vendor_error_falls_back_to_facts_with_the_reason() {
        let (_h, s) = store();
        let fake = pulse_provider_fake::new(Script {
            replies: vec![Reply {
                err: Some(pulse_provider::ProviderError::new(
                    "fake",
                    503,
                    "overloaded",
                    "try again later",
                    true,
                )),
                ..Reply::default()
            }],
            ..Script::default()
        });
        let req = AskRequest {
            project: "demo".into(),
            question: "disk?".into(),
            model: "m".into(),
            ..AskRequest::default()
        };
        let a = ask(&s, Some(&fake), &req, DEFAULT_TIMEOUT).unwrap();
        assert!(a.facts_only);
        assert!(a.note.starts_with("the model call failed: "), "{}", a.note);
        assert!(a.note.contains("try again later"), "{}", a.note);
        assert!(a.answer.contains("Matching the question:"), "{}", a.answer);
    }

    #[test]
    fn bad_questions_and_projects_are_refused() {
        let (_h, s) = store();
        assert!(check_question("   ").is_err());
        assert!(check_question(&"x".repeat(MAX_QUESTION + 1)).is_err());
        assert_eq!(check_question(" ok ").unwrap(), "ok");
        let e = ask(
            &s,
            None,
            &AskRequest {
                project: "nope".into(),
                question: "hi".into(),
                ..AskRequest::default()
            },
            DEFAULT_TIMEOUT,
        )
        .unwrap_err();
        assert!(
            e.downcast_ref::<pulse_tally::NoProjectError>().is_some(),
            "{e}"
        );
        let e = ask(
            &s,
            None,
            &AskRequest {
                question: "hi".into(),
                ..AskRequest::default()
            },
            DEFAULT_TIMEOUT,
        )
        .unwrap_err();
        assert!(e.to_string().contains("no project named"), "{e}");
    }

    #[test]
    fn model_choice_prefers_request_then_settings() {
        let (_h, s) = store();
        let fake = pulse_provider_fake::new(Script::default());
        let cancel = CancelToken::new();
        assert_eq!(
            choose_model(&s, &fake, &cancel, " asked ").unwrap(),
            "asked"
        );
        s.set_setting("human", "worker_model", "worker").unwrap();
        assert_eq!(choose_model(&s, &fake, &cancel, "").unwrap(), "worker");
        s.set_setting("human", "light_model", "light").unwrap();
        assert_eq!(choose_model(&s, &fake, &cancel, "").unwrap(), "light");
        s.set_setting("human", "ask.model", "quick").unwrap();
        assert_eq!(choose_model(&s, &fake, &cancel, "").unwrap(), "quick");
    }
}
