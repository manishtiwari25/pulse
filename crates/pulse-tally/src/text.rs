use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

pub(crate) fn day_number(mut value: &str) -> i32 {
    // Cut at a char boundary: a byte slice would panic on a non-ASCII date string.
    if let Some((idx, _)) = value.char_indices().nth(10) {
        value = &value[..idx];
    }
    let Ok(t) = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d") else {
        return 0;
    };
    chrono::Datelike::num_days_from_ce(&t)
}

pub(crate) fn trunc(text: &str, n: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() > n {
        format!(
            "{}~",
            chars[..n.saturating_sub(1)].iter().collect::<String>()
        )
    } else {
        text.to_string()
    }
}

pub(crate) fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

pub(crate) fn sha256_text(text: &str) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn fenced_block(body: &str, heading: &str) -> Option<Vec<String>> {
    let mut state = 0;
    let mut fence = String::new();
    let mut lines: Vec<String> = Vec::new();
    for raw in body.split('\n') {
        match state {
            0 if raw.starts_with(&format!("## {heading}"))
                && raw[("## ".len() + heading.len())..]
                    .trim_end_matches([' ', '\t'])
                    .is_empty() =>
            {
                state = 1;
            }
            0 => {}
            1 => {
                if raw.starts_with("## ") {
                    state = -1;
                    break;
                }
                if raw.starts_with('`') {
                    let n = raw.bytes().take_while(|b| *b == b'`').count();
                    fence = raw[..n].to_string();
                    state = 2;
                }
            }
            2 => {
                if raw.starts_with(&fence)
                    && raw[fence.len()..].trim_end_matches([' ', '\t']).is_empty()
                {
                    state = 3;
                    break;
                }
                lines.push(raw.trim_end_matches([' ', '\t']).to_string());
            }
            _ => {}
        }
        if state == -1 || state == 3 {
            break;
        }
    }
    if state < 2 {
        return None;
    }
    while lines.first().is_some_and(|s| s.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|s| s.is_empty()) {
        lines.pop();
    }
    if lines.is_empty() { None } else { Some(lines) }
}

pub(crate) fn block_hash(lines: &[String]) -> String {
    sha256_text(&(lines.join("\n") + "\n"))
}

pub fn text_hash(text: &str) -> String {
    if let Some(block) = fenced_block(text, "Proposed text") {
        return block_hash(&block);
    }
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|l| l.trim_end_matches([' ', '\t']).to_string())
        .collect();
    while lines.first().is_some_and(|s| s.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|s| s.is_empty()) {
        lines.pop();
    }
    block_hash(&lines)
}

fn table_cell(text: &str, name: &str) -> Option<String> {
    let re = Regex::new(&format!(r"\|[ \t]*{}[ \t]*\|([^|]*)", regex::escape(name))).ok()?;
    let m = re.captures(text)?;
    Some(m[1].replace('`', "").trim().to_string())
}

static FRACTION_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([0-9]+)/([0-9]+)").unwrap());
static INTEGER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9]+").unwrap());

fn table_num(text: &str, name: &str) -> Option<f64> {
    let cell = table_cell(text, name)?;
    if let Some(m) = FRACTION_RE.captures(&cell) {
        let n: f64 = m[1].parse().ok()?;
        let d: f64 = m[2].parse().ok()?;
        return Some(n / d);
    }
    if let Some(m) = INTEGER_RE.find(&cell) {
        return m.as_str().parse().ok();
    }
    None
}

pub(crate) fn table_row(text: &str, name: &str) -> Option<String> {
    for line in text.split('\n') {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() >= 3 && parts[1].trim() == name {
            return Some(parts[2].trim().to_string());
        }
    }
    None
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmendmentCheck {
    pub hash: String,
    pub anchor: String,
    pub section: String,
    pub verdict: String,
}

pub fn check(body: &str, constitution: &str, protected: &str) -> AmendmentCheck {
    let Some(prop) = fenced_block(body, "Proposed text") else {
        return AmendmentCheck {
            verdict: "malformed: no fenced block under \"## Proposed text\"".into(),
            ..AmendmentCheck::default()
        };
    };
    let hsh = block_hash(&prop);
    let Some(cur) = fenced_block(body, "Current text") else {
        return AmendmentCheck {
            hash: hsh,
            verdict: "malformed: no fenced block under \"## Current text\"".into(),
            ..AmendmentCheck::default()
        };
    };
    if constitution.is_empty() {
        return AmendmentCheck {
            hash: hsh,
            verdict: "unchecked: no constitution record".into(),
            ..AmendmentCheck::default()
        };
    }
    let cur_text = cur.join("\n") + "\n";
    let prop_text = prop.join("\n") + "\n";
    let mut verdict = "ok".to_string();
    let cq = table_num(&cur_text, "Amendment quorum");
    let pq = table_num(&prop_text, "Amendment quorum");
    if cq.is_some() && pq.is_none_or(|p| p < cq.unwrap()) {
        verdict = "lowers or removes the amendment quorum".into();
    }
    let cm = table_num(&cur_text, "Amendment supermajority");
    let pm = table_num(&prop_text, "Amendment supermajority");
    if verdict == "ok" && cm.is_some() && pm.is_none_or(|p| p < cm.unwrap()) {
        verdict = "lowers or removes the amendment supermajority".into();
    }
    for guard in ["Amendment voter", "Informed vote", "Not amendable by vote"] {
        if verdict != "ok" {
            break;
        }
        if table_cell(&cur_text, guard).is_some() && table_cell(&prop_text, guard).is_none() {
            verdict = format!("removes the \"{guard}\" row");
        }
    }
    if verdict == "ok"
        && let (Some(cp), Some(pp)) = (
            table_cell(&cur_text, "Not amendable by vote"),
            table_cell(&prop_text, "Not amendable by vote"),
        )
    {
        for item in cp.split(',') {
            let it = item.trim();
            if !it.is_empty() && !format!(",{pp},").contains(it) {
                verdict = format!("drops \"{it}\" from the protected sections");
            }
        }
    }
    let mut lines: Vec<String> = constitution
        .split('\n')
        .map(|l| l.trim_end_matches([' ', '\t']).to_string())
        .collect();
    if lines.last().is_some_and(|s| s.is_empty()) {
        lines.pop();
    }
    let hay = format!("\n{}\n", lines.join("\n"));
    let needle = format!("\n{cur_text}");
    let mut count = 0;
    let mut pos = 0usize;
    let mut first: Option<usize> = None;
    while let Some(k) = hay[pos..].find(&needle) {
        let at = pos + k;
        count += 1;
        first.get_or_insert(at);
        pos = at + 1;
    }
    let mut sections: Vec<String> = Vec::new();
    if count == 1 {
        let first = first.unwrap_or(0);
        let ln = hay[..first + 1].matches('\n').count();
        let span = cur_text.matches('\n').count();
        let n = lines.len();
        let mut fm_end = 0;
        if n > 0 && is_rule(&lines[0]) {
            for (i, line) in lines.iter().enumerate().skip(1) {
                if is_rule(line) {
                    fm_end = i + 1;
                    break;
                }
            }
        }
        let mut sec_of = Vec::with_capacity(n);
        let mut sec = "preamble".to_string();
        for (i, line) in lines.iter().enumerate() {
            if let Some(rest) = line.strip_prefix("## ") {
                sec = rest.trim_end_matches([' ', '\t']).to_string();
            }
            if fm_end > 0 && i < fm_end {
                sec_of.push("frontmatter".to_string());
            } else {
                sec_of.push(sec.clone());
            }
        }
        let items: Vec<String> = protected
            .replace('`', "")
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
        let end = std::cmp::min(ln + span, n + 1);
        for j in ln..end {
            if j < 1 || j > n {
                continue;
            }
            let s = sec_of[j - 1].clone();
            if !sections.contains(&s) {
                sections.push(s.clone());
            }
            for it in &items {
                if verdict == "ok" && !it.is_empty() && it.eq_ignore_ascii_case(&s) {
                    verdict = format!("protected section \"{s}\"");
                }
            }
        }
    }
    AmendmentCheck {
        hash: hsh,
        anchor: count.to_string(),
        section: sections.join("|"),
        verdict,
    }
}

fn is_rule(line: &str) -> bool {
    line.starts_with("---") && line[3..].trim_end_matches([' ', '\t']).is_empty()
}

#[derive(Debug, thiserror::Error)]
#[error("{}", check.verdict)]
pub struct VerdictError {
    pub check: AmendmentCheck,
}

pub fn check_amendment(body: &str, constitution: &str, protected: &[String]) -> anyhow::Result<()> {
    let c = check(body, constitution, &protected.join(", "));
    if c.verdict == "ok" {
        Ok(())
    } else {
        Err(VerdictError { check: c }.into())
    }
}

#[cfg(test)]
mod day_number_tests {
    use super::day_number;

    #[test]
    fn day_number_does_not_panic_on_malformed_non_ascii_date() {
        assert_eq!(day_number("2026-10-0\u{e9}T10:00:00Z"), 0);
        assert_eq!(
            day_number("\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}"),
            0
        );
        assert_eq!(day_number("\u{1f600}2026-10-04"), 0);
        assert_eq!(day_number("2026-10-04T10:00:00Z"), day_number("2026-10-04"));
        assert!(day_number("2026-10-04") > 0);
    }
}
