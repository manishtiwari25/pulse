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
pub use parse_mentions as ParseMentions;

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
pub use mentions_for as MentionsFor;

pub fn mention_count(s: &pulse_store::Store, project: &str, agent: &str) -> Result<usize> {
    Ok(mentions_for(s, project, agent, 0)?.len())
}
pub use mention_count as MentionCount;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_mentions_cases() {
        assert_eq!(
            parse_mentions("hi @Owner and (@agent-1), @agent-1"),
            vec!["human", "agent-1"]
        );
        assert!(parse_mentions("mail a@b").is_empty());
    }
}
