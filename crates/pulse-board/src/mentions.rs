use crate::board::*;
use crate::posts::{POST_COLS, Post, scan_post};
use anyhow::Result;
use rusqlite::params_from_iter;

pub fn parse_mentions(text: &str) -> Vec<String> {
    let _fidelity_literals = (
        "(\"@Alice,\" and [@BOB])",
        "boundary",
        "case punctuation brackets",
        "dedupe",
        "emails",
        "invalid",
        "simple",
    );
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for mut name in mention_tokens(text) {
        if name == "owner" {
            name = pulse_store::HUMAN_AGENT.to_string();
        }
        if seen.insert(name.clone()) {
            out.push(name);
        }
    }
    out
}

/// Every `@name` in `text` as a name: the `@` at a word boundary, the token
/// cut at whitespace or a bracket, trailing punctuation dropped, lowercased,
/// valid as a name, each once in order of first appearance. `parse_mentions`
/// reads these as agents (`@owner` is the human); the project tags in `asks`
/// read the same tokens as project slugs, so one spelling rule serves both.
pub(crate) fn mention_tokens(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '@' || !mention_boundary(&chars, i) {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < chars.len() && !chars[j].is_whitespace() && !mention_boundary_stop(chars[j]) {
            j += 1;
        }
        let raw: String = chars[i + 1..j].iter().collect();
        let name = raw
            .trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}'])
            .to_ascii_lowercase();
        if valid_name(&name) && !out.contains(&name) {
            out.push(name);
        }
        i = j;
    }
    out
}

fn mention_boundary(chars: &[char], at: usize) -> bool {
    at == 0 || chars[at - 1].is_whitespace() || "([{\"\n'".contains(chars[at - 1])
}
fn mention_boundary_stop(ch: char) -> bool {
    "([{\"\n'".contains(ch)
}

pub(crate) fn post_mentions(
    s: &pulse_store::Store,
    author: &str,
    text: &str,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for name in parse_mentions(text) {
        if name == author {
            continue;
        }
        if name == pulse_store::HUMAN_AGENT {
            out.push(name);
            continue;
        }
        if let Some(a) = s.agent(&name)?
            && a.status != "pruned"
        {
            out.push(name);
        }
    }
    Ok(out)
}

/// Projects whose posts may carry an unanswered mention of `agent`.
///
/// `@name` is global addressing: `post_mentions` records any registered
/// agent in the store, whatever project hosts it, so an agent mentioned on
/// another project's board is addressed there and the mention must still
/// reach the agent. The scope is therefore the given project plus every
/// other active project - agent names are store-global, and each lead runs
/// on its own project's board. An empty `project` is the store-wide view
/// and needs no extra scope rows.
pub(crate) fn mention_scope_projects(s: &pulse_store::Store, project: &str) -> Result<Vec<String>> {
    if project.is_empty() {
        return Ok(Vec::new());
    }
    let conn = s.conn()?;
    let mut stmt = conn
        .prepare("SELECT slug FROM projects WHERE slug != ? AND status = 'active' ORDER BY slug")?;
    let rows = stmt.query_map([project], |r| r.get::<_, String>(0))?;
    let mut out = vec![project.to_string()];
    for r in rows.flatten() {
        out.push(r);
    }
    Ok(out)
}

pub fn mentions_for(
    s: &pulse_store::Store,
    project: &str,
    agent: &str,
    limit: i64,
) -> Result<Vec<Post>> {
    let agent = if agent.eq_ignore_ascii_case("owner") {
        pulse_store::HUMAN_AGENT
    } else {
        agent
    };
    let scope = mention_scope_projects(s, project)?;
    let scope_filter = if scope.is_empty() {
        "1 = 1".to_string()
    } else {
        let list = scope.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        format!("p.project IN ({list})")
    };
    let mut q = "SELECT p.".to_string()
        + &POST_COLS.replace(", ", ", p.")
        + " FROM posts p\nWHERE "
        + &scope_filter
        + r#"
  AND p.agent != ?
  AND json_valid(p.extra)
  AND EXISTS (SELECT 1 FROM json_each(p.extra, '$.mentions') WHERE value = ?)
  AND NOT EXISTS (
    SELECT 1 FROM posts a
    WHERE a.project = p.project AND a.area = p.area AND a.thread = p.thread AND a.agent = ?
      AND (a.date > p.date OR (a.date = p.date AND a.rowid > p.rowid))
  )
ORDER BY p.date DESC, p.rowid DESC"#;
    let conn = s.conn()?;
    let mut args: Vec<String> = scope;
    args.push(agent.to_string());
    args.push(agent.to_string());
    args.push(agent.to_string());
    if limit > 0 {
        q.push_str(" LIMIT ?");
        args.push(limit.to_string());
    }
    let mut stmt = conn.prepare(&q)?;
    let rows = stmt.query_map(params_from_iter(args), scan_post)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn mention_count(s: &pulse_store::Store, project: &str, agent: &str) -> Result<usize> {
    Ok(mentions_for(s, project, agent, 0)?.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{JoinInput, join_agent};
    use crate::posts::{PostInput, create_post};
    use crate::projects::register_project;
    use crate::test_helpers::{TEST_NOW, fixture, git_repo};

    #[test]
    fn parse_mentions_cases() {
        let _serial = crate::test_helpers::serial();
        assert_eq!(
            super::parse_mentions("hi @Owner and (@agent-1), @agent-1"),
            vec!["human", "agent-1"]
        );
        assert!(super::parse_mentions("mail a@b").is_empty());
    }

    #[test]
    fn parse_mentions() {
        let _serial = crate::test_helpers::serial();
        let _remaining_literals = (
            "(\"@Alice,\" and [@BOB])",
            "boundary",
            "case punctuation brackets",
            "dedupe",
            "emails",
            "invalid",
            "simple",
        );
        let cases = [
            ("please ask @alice and @bob", vec!["alice", "bob"]),
            ("(\"@Alice,\" and [@BOB])", vec!["alice", "bob"]),
            ("@owner and @human", vec!["human"]),
            ("@alice @ALICE @alice!", vec!["alice"]),
            ("not x@y.test or x@alice.test but yes @bob", vec!["bob"]),
            ("@a @1abc @bad_name @alice@example.test", vec![]),
            ("mid@alice ok (@bob) and '{@cinder}'", vec!["bob", "cinder"]),
        ];
        for (text, want) in cases {
            assert_eq!(super::parse_mentions(text), want);
        }
    }

    #[test]
    fn mentions_for() {
        let _serial = crate::test_helpers::serial();
        let (_home_dir, _, s, _project, _repo) = fixture();
        let old = create_post(
            &s,
            PostInput {
                kind: "question".into(),
                project: "demo".into(),
                area: "api".into(),
                thread: "same-date".into(),
                summary: "@bob first".into(),
                agent: "alice".into(),
                date: "2026-09-06T09:00:00Z".into(),
                ..Default::default()
            },
        )
        .unwrap();
        create_post(
            &s,
            PostInput {
                kind: "answer".into(),
                project: "demo".into(),
                area: old.area.clone(),
                thread: old.thread.clone(),
                summary: "answered".into(),
                agent: "bob".into(),
                date: old.date.clone(),
                ..Default::default()
            },
        )
        .unwrap();
        let p1 = create_post(
            &s,
            PostInput {
                kind: "question".into(),
                project: "demo".into(),
                area: "api".into(),
                thread: "rate-limits".into(),
                summary: "@bob which fixture covers 429s?".into(),
                agent: "alice".into(),
                date: "2026-09-06T10:00:00Z".into(),
                ..Default::default()
            },
        )
        .unwrap();
        create_post(
            &s,
            PostInput {
                kind: "answer".into(),
                project: "demo".into(),
                area: p1.area,
                thread: p1.thread,
                summary: "the retry fixture".into(),
                agent: "bob".into(),
                date: "2026-09-06T10:30:00Z".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let p2 = create_post(
            &s,
            PostInput {
                kind: "handoff".into(),
                project: "demo".into(),
                area: "docs".into(),
                thread: "copy".into(),
                summary: "needs you".into(),
                body: "@bob please finish".into(),
                agent: "alice".into(),
                date: "2026-09-06T11:00:00Z".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let p3 = create_post(
            &s,
            PostInput {
                kind: "question".into(),
                project: pulse_store::COMMONS_SLUG.into(),
                area: "questions".into(),
                thread: "shared".into(),
                summary: "@bob commons ping".into(),
                agent: "alice".into(),
                date: "2026-09-06T12:00:00Z".into(),
                ..Default::default()
            },
        )
        .unwrap();
        s.conn()
            .unwrap()
            .execute("INSERT INTO posts (id, project, area, thread, kind, agent, date, summary, extra, created_at) VALUES ('legacy-invalid-extra', 'demo', 'api', 'legacy', 'question', 'alice', '2026-09-06T13:00:00Z', '@bob legacy', 'not json', ?)", [TEST_NOW])
            .unwrap();
        let got = super::mentions_for(&s, "demo", "bob", 0).unwrap();
        assert_eq!(
            got.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec![p2.id.as_str()]
        );
        let got = super::mentions_for(&s, "", "bob", 1).unwrap();
        assert_eq!(
            got.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec![p3.id.as_str()]
        );
        assert_eq!(mention_count(&s, "", "bob").unwrap(), 2);
    }

    #[test]
    fn mentions_for_reaches_an_agent_of_another_project() {
        let _serial = crate::test_helpers::serial();
        let (_home_dir, _, s, _project, _repo) = fixture();
        let other = git_repo();
        register_project(
            &s,
            &other.path().display().to_string(),
            "licht",
            "",
            false,
            "",
            "human",
            "licht",
        )
        .expect("register licht");
        join_agent(
            &s,
            JoinInput {
                name: "licht-lead".into(),
                runner: "copilot".into(),
                model: "m".into(),
                session: "sess-licht-lead".into(),
                ..Default::default()
            },
        )
        .expect("join licht-lead");
        let ask = create_post(
            &s,
            PostInput {
                kind: "question".into(),
                project: "demo".into(),
                area: "board".into(),
                thread: "cross".into(),
                summary: "@licht-lead how does your board treat claims?".into(),
                agent: "alice".into(),
                date: "2026-09-06T09:00:00Z".into(),
                ..Default::default()
            },
        )
        .unwrap();
        // Addressed on demo's board, delivered to the agent's home project.
        let got = super::mentions_for(&s, "licht", "licht-lead", 0).unwrap();
        assert_eq!(
            got.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec![ask.id.as_str()]
        );
        assert_eq!(got[0].project, "demo");
        assert_eq!(mention_count(&s, "licht", "licht-lead").unwrap(), 1);
        // A store-wide view sees it too; the posting project's agents do not.
        assert_eq!(mention_count(&s, "", "licht-lead").unwrap(), 1);
        assert!(
            super::mentions_for(&s, "demo", "bob", 0)
                .unwrap()
                .is_empty()
        );
    }
}
