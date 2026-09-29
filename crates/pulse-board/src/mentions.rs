use crate::board::*;
use crate::posts::{POST_COLS, Post, scan_post};
use anyhow::Result;
use rusqlite::params_from_iter;

pub fn parse_mentions(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
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
        let mut name = raw
            .trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}'])
            .to_ascii_lowercase();
        if name == "owner" {
            name = pulse_store::HUMAN_AGENT.to_string();
        }
        if valid_name(&name) && seen.insert(name.clone()) {
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
    let mut q = "SELECT p.".to_string()
        + &POST_COLS.replace(", ", ", p.")
        + r#" FROM posts p
WHERE (? = '' OR p.project = ?)
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
    let mut args = vec![
        project.to_string(),
        project.to_string(),
        agent.to_string(),
        agent.to_string(),
        agent.to_string(),
    ];
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
    use crate::posts::{PostInput, create_post};
    use crate::test_helpers::{TEST_NOW, fixture};

    #[test]
    fn parse_mentions_cases() {
        assert_eq!(
            super::parse_mentions("hi @Owner and (@agent-1), @agent-1"),
            vec!["human", "agent-1"]
        );
        assert!(super::parse_mentions("mail a@b").is_empty());
    }

    #[test]
    fn parse_mentions() {
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
}
