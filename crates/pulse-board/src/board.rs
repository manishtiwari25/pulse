use anyhow::{Result, anyhow};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use regex::Regex;
use std::process::Command as ProcessCommand;
use std::sync::OnceLock;

pub const MAX_SUMMARY_CHARS: usize = 240;
pub const KINDS: &[&str] = &[
    "claim",
    "handoff",
    "finding",
    "question",
    "answer",
    "suggestion",
    "opinion",
    "vote",
    "amendment",
    "result",
];
pub const POST_KINDS: &[&str] = KINDS;
pub const RESOLUTIONS: &[&str] = &[
    "answered",
    "promoted",
    "tested",
    "ratified",
    "agreed",
    "accepted",
    "declined",
    "dropped",
    "proceeded",
];
pub const USEFUL_KINDS: &[&str] = &[
    "finding",
    "answer",
    "result",
    "vote",
    "amendment",
    "opinion",
];
pub const JOB_KINDS: &[&str] = &[
    "onboard",
    "task",
    "improve",
    "answer",
    "ballot",
    "clean",
    "reflect",
    "experiment",
    "lead",
    "baseline",
];
pub const JOB_END_STATES: &[&str] = &["done", "failed", "cancelled", "blocked", "queued"];
pub const JOB_RECOVER_ACTION: &str = "job.recover";
pub const NAME_WORDS: &[&str] = &[
    "aurora", "borealis", "cinder", "delta", "ember", "fjord", "granite", "harbor", "iris",
    "juniper", "kestrel", "lumen", "meridian", "nimbus", "orbit", "pallas", "quartz", "ripple",
    "sable", "tundra", "umber", "vesper", "willow", "xenon", "yarrow", "zephyr",
];
pub const LOCAL_EXECUTOR: &str = "local";
pub(crate) const TIME_LAYOUT: &str = "%Y-%m-%dT%H:%M:%SZ";

pub(crate) fn refuse(code: i32, what: impl Into<String>) -> anyhow::Error {
    pulse_cli::Error {
        what: what.into(),
        why: String::new(),
        fix: String::new(),
        code,
    }
    .into()
}

pub(crate) fn refuse_fix(
    code: i32,
    what: impl Into<String>,
    why: impl Into<String>,
    fix: impl Into<String>,
) -> anyhow::Error {
    pulse_cli::Error {
        what: what.into(),
        why: why.into(),
        fix: fix.into(),
        code,
    }
    .into()
}

pub(crate) fn contains(list: &[&str], v: &str) -> bool {
    list.contains(&v)
}

pub fn py_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

pub fn slugify(name: &str) -> String {
    slugify_n(name, 40)
}

pub(crate) fn slugify_n(name: &str, limit: usize) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"[^a-z0-9]+").unwrap());
    let mut slug = re
        .replace_all(&name.to_ascii_lowercase(), "-")
        .trim_matches('-')
        .to_string();
    if slug.len() > limit {
        slug.truncate(limit);
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "x".to_string()
    } else {
        slug
    }
}

pub fn valid_name(name: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[a-z][a-z0-9-]{1,39}$").unwrap())
        .is_match(name)
}

pub fn parse_iso(value: &str) -> Result<DateTime<Utc>> {
    let t = DateTime::parse_from_rfc3339(value).map_err(|_| {
        refuse(
            2,
            format!(
                "Not a UTC time like 2026-09-05T08:00:00Z: {}",
                py_repr(value)
            ),
        )
    })?;
    Ok(t.with_timezone(&Utc))
}

pub fn plus_hours(iso: &str, hours: f64) -> Result<String> {
    let t = parse_iso(iso)?;
    let micros = (hours * 3_600_000_000.0) as i64;
    Ok((t + Duration::microseconds(micros))
        .format(TIME_LAYOUT)
        .to_string())
}

pub(crate) fn stamp_of(iso: &str) -> String {
    iso.replace(['-', ':'], "")
}

pub(crate) fn sha256_text(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    hex::encode(h.finalize())
}

pub(crate) fn trunc(text: &str, n: usize) -> String {
    let mut chars: Vec<char> = text.chars().collect();
    if chars.len() > n {
        chars.truncate(n.saturating_sub(1));
        chars.push('~');
    }
    chars.into_iter().collect()
}

#[allow(dead_code)]
pub(crate) fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

pub fn has_read_line(body: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?m)^Read:[ \t]*\S").unwrap())
        .is_match(body)
}

pub fn fenced_block(body: &str, heading: &str) -> Option<Vec<String>> {
    let head_re = Regex::new(&format!(r"^## {}[ \t]*$", regex::escape(heading))).ok()?;
    let fence_re = Regex::new(r"^(`+)").ok()?;
    let mut state = 0;
    let mut fence = String::new();
    let mut lines = Vec::new();
    for raw in body.split('\n') {
        match state {
            0 if head_re.is_match(raw) => state = 1,
            1 if raw.starts_with("## ") => state = -1,
            1 => {
                if let Some(m) = fence_re.captures(raw) {
                    fence = m[1].to_string();
                    state = 2;
                }
            }
            2 if Regex::new(&format!(r"^{}[ \t]*$", regex::escape(&fence)))
                .ok()?
                .is_match(raw) =>
            {
                state = 3
            }
            2 => lines.push(raw.trim_end_matches([' ', '\t']).to_string()),
            _ => {}
        }
        if state < 0 || state == 3 {
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

pub fn block_hash(lines: &[String]) -> String {
    sha256_text(&(lines.join("\n") + "\n"))
}

pub(crate) fn git(repo: &str, args: &[&str]) -> (String, bool) {
    let mut cmd = ProcessCommand::new("git");
    cmd.arg("-C").arg(repo).args(args);
    cmd.env_clear();
    for kv in pulse_gitenv::env() {
        if let Some((k, v)) = kv.split_once('=') {
            cmd.env(k, v);
        }
    }
    match cmd.output() {
        Ok(out) if out.status.success() => (
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
            true,
        ),
        _ => (String::new(), false),
    }
}

pub(crate) fn git_branch(repo: &str) -> String {
    let (out, ok) = git(repo, &["rev-parse", "--abbrev-ref", "HEAD"]);
    if ok { out } else { String::new() }
}

pub(crate) fn git_top_level(dir: &str) -> String {
    let (out, ok) = git(dir, &["rev-parse", "--show-toplevel"]);
    if ok { out } else { String::new() }
}

pub(crate) fn day_number(value: &str) -> i64 {
    if value.len() < 10 {
        return 0;
    }
    NaiveDate::parse_from_str(&value[..10], "%Y-%m-%d")
        .ok()
        .and_then(|d| Utc.from_local_datetime(&d.and_hms_opt(0, 0, 0)?).single())
        .map_or(0, |t| t.timestamp() / 86400 + 719_163)
}

pub(crate) fn event<D: Into<pulse_store::Detail>>(
    s: &pulse_store::Store,
    actor: &str,
    action: &str,
    subject: &str,
    detail: D,
) -> Result<()> {
    let _ = pulse_bus::emit(s, actor, action, subject, detail)?;
    Ok(())
}

pub(crate) fn event_conn<D: Into<pulse_store::Detail>>(
    conn: &rusqlite::Connection,
    s: &pulse_store::Store,
    actor: &str,
    action: &str,
    subject: &str,
    detail: D,
) -> Result<()> {
    let _ = pulse_bus::emit_conn(conn, &s.now(), actor, action, subject, detail)?;
    Ok(())
}

pub(crate) fn store(c: &pulse_cli::Context) -> Result<&pulse_store::Store> {
    c.store
        .as_ref()
        .ok_or_else(|| anyhow!("command requires a store"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        let _serial = crate::test_helpers::serial();
        let a50 = "a".repeat(50);
        let a40 = "a".repeat(40);
        let a39_bcd = format!("{}-bcd", "a".repeat(39));
        let a39 = "a".repeat(39);
        for (input, want) in [
            ("Runner Speed", "runner-speed"),
            ("!!!", "x"),
            ("", "x"),
            ("A--B", "a-b"),
            (a50.as_str(), a40.as_str()),
            (a39_bcd.as_str(), a39.as_str()),
        ] {
            assert_eq!(slugify(input), want, "slugify({input:?})");
        }
        let a41 = "a".repeat(41);
        for (input, ok) in [
            ("alice", true),
            ("a", false),
            ("1abc", false),
            ("Alice", false),
            ("a-1", true),
            (a41.as_str(), false),
        ] {
            assert_eq!(valid_name(input), ok, "valid_name({input:?}) != {ok}");
        }
        assert!(
            py_repr("it's") == r#""it's""#
                && py_repr("plain") == "'plain'"
                && py_repr(r#"a"b'c"#) == r#"'a"b\'c'"#,
            "pyRepr: {} {} {}",
            py_repr("it's"),
            py_repr("plain"),
            py_repr(r#"a"b'c"#)
        );
        assert!(
            trunc("abcdef", 4) == "abc~" && trunc("abc", 4) == "abc",
            "trunc"
        );
        assert_eq!(
            plus_hours("2026-09-06T10:00:00Z", -0.5).unwrap(),
            "2026-09-06T09:30:00Z",
            "plus_hours"
        );
        assert_eq!(
            fenced_block("## Proposed text\n```\n\n  x  \n\n```\n", "Proposed text").unwrap()[0],
            "  x",
            "fenced block normalization"
        );
        assert!(
            fenced_block("## Proposed text\n## Other\n```\nx\n```\n", "Proposed text").is_none(),
            "heading before fence must end the search"
        );
        assert!(
            has_read_line("a\nRead: yes\n") && !has_read_line("Read:\n"),
            "read line"
        );
    }
}
