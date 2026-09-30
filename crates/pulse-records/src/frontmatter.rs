use crate::records::Frontmatter;
use anyhow::{Result, anyhow};
use serde_json::{Number, Value};

pub fn parse_frontmatter(text: &str) -> Result<(Frontmatter, String)> {
    let front = Frontmatter::new();
    let Some((first, mut rest)) = text.split_once('\n') else {
        return Ok((front, text.to_string()));
    };
    if first.trim_end_matches('\r') != "---" {
        return Ok((front, text.to_string()));
    }
    let mut lines = Vec::new();
    loop {
        let Some((line, remaining)) = rest.split_once('\n') else {
            return Err(anyhow!("frontmatter: missing closing ---"));
        };
        if line.trim_end_matches('\r') == "---" {
            let mut p = YamlSubset { lines, pos: 0 };
            return Ok((p.mapping(0)?, remaining.to_string()));
        }
        lines.push(line.trim_end_matches('\r').to_string());
        rest = remaining;
    }
}

struct YamlSubset {
    lines: Vec<String>,
    pos: usize,
}

fn indentation(s: &str) -> usize {
    s.len() - s.trim_start_matches(' ').len()
}

impl YamlSubset {
    fn skip(&mut self) {
        while self.pos < self.lines.len() {
            let s = self.lines[self.pos].trim();
            if !s.is_empty() && !s.starts_with('#') {
                return;
            }
            self.pos += 1;
        }
    }
    fn fail<T>(&self, message: impl AsRef<str>) -> Result<T> {
        Err(anyhow!(
            "frontmatter line {}: {}",
            self.pos + 2,
            message.as_ref()
        ))
    }
    fn mapping(&mut self, indent: usize) -> Result<Frontmatter> {
        let mut out = Frontmatter::new();
        loop {
            self.skip();
            if self.pos == self.lines.len() || indentation(&self.lines[self.pos]) < indent {
                return Ok(out);
            }
            let line = self.lines[self.pos].clone();
            if indentation(&line) != indent || line.trim_start().starts_with("- ") {
                return self.fail("unexpected indentation or list item");
            }
            let Some((key_raw, raw_raw)) = line.trim().split_once(':') else {
                return self.fail("expected key: value");
            };
            let key = key_raw.trim();
            if key.is_empty() || key.contains([' ', '\t', '[', ']', '{', '}', '&', '*', '!']) {
                return self.fail("expected key: value");
            }
            if out.contains_key(key) {
                return self.fail(format!("duplicate key {key}"));
            }
            self.pos += 1;
            let raw = strip_comment(raw_raw).trim().to_string();
            let value = if is_block_scalar(&raw) {
                self.block_scalar(&raw, indent)?
            } else if raw.is_empty() {
                self.skip();
                if self.pos < self.lines.len()
                    && indentation(&self.lines[self.pos]) >= indent
                    && is_list_item(self.lines[self.pos].trim())
                {
                    Value::Array(self.sequence(indentation(&self.lines[self.pos]))?)
                } else if self.pos < self.lines.len() && indentation(&self.lines[self.pos]) > indent
                {
                    Value::Object(
                        self.mapping(indentation(&self.lines[self.pos]))?
                            .into_iter()
                            .collect(),
                    )
                } else {
                    Value::Null
                }
            } else {
                parse_value(&raw)
                    .map_err(|e| anyhow!("frontmatter line {}: {key}: {e}", self.pos + 1))?
            };
            out.insert(key.to_string(), value);
        }
    }
    fn sequence(&mut self, indent: usize) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        loop {
            self.skip();
            if self.pos == self.lines.len() || indentation(&self.lines[self.pos]) < indent {
                return Ok(out);
            }
            let line = self.lines[self.pos].clone();
            let trimmed = line.trim();
            if indentation(&line) == indent && !is_list_item(trimmed) {
                return Ok(out);
            }
            if indentation(&line) != indent || !is_list_item(trimmed) {
                return self.fail("expected - item");
            }
            let raw = strip_comment(trimmed.trim_start_matches('-'))
                .trim()
                .to_string();
            self.pos += 1;
            let value = if is_block_scalar(&raw) {
                self.block_scalar(&raw, indent)?
            } else {
                parse_value(&raw).map_err(|e| anyhow!("frontmatter line {}: {e}", self.pos + 1))?
            };
            out.push(value);
        }
    }
    fn block_scalar(&mut self, marker: &str, parent_indent: usize) -> Result<Value> {
        let start = self.pos;
        let mut indent: Option<usize> = None;
        while self.pos < self.lines.len() {
            let line = &self.lines[self.pos];
            if !line.trim().is_empty() {
                let n = indentation(line);
                if n <= parent_indent {
                    break;
                }
                if indent.is_none() {
                    indent = Some(n);
                }
                if n < indent.unwrap() {
                    return self.fail("inconsistent block scalar indentation");
                }
            }
            self.pos += 1;
        }
        if start == self.pos {
            return Ok(Value::String(String::new()));
        }
        let indent = indent.unwrap_or(parent_indent + 1);
        let mut lines = Vec::new();
        for s in &self.lines[start..self.pos] {
            if s.trim().is_empty() {
                lines.push(String::new());
            } else {
                lines.push(s[indent..].to_string());
            }
        }
        let mut b = String::new();
        for (i, s) in lines.iter().enumerate() {
            b.push_str(s);
            if marker.starts_with('>') && i + 1 < lines.len() {
                let next = &lines[i + 1];
                if !s.is_empty()
                    && !next.is_empty()
                    && !s.starts_with(' ')
                    && !next.starts_with(' ')
                {
                    b.push(' ');
                } else if s.is_empty()
                    && !next.is_empty()
                    && i > 0
                    && !lines[i - 1].is_empty()
                    && !next.starts_with(' ')
                {
                } else {
                    b.push('\n');
                }
            } else {
                b.push('\n');
            }
        }
        let text = if marker.ends_with('-') {
            b.trim_end_matches('\n').to_string()
        } else if marker.ends_with('+') {
            b
        } else {
            format!("{}\n", b.trim_end_matches('\n'))
        };
        Ok(Value::String(text))
    }
}

fn is_list_item(s: &str) -> bool {
    s == "-" || s.starts_with("- ")
}
fn is_block_scalar(s: &str) -> bool {
    matches!(s, "|" | "|-" | "|+" | ">" | ">-" | ">+")
}

fn strip_comment(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut quote = 0u8;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if quote != 0 {
            if c == b'\\' && quote == b'"' {
                i += 1;
            } else if c == quote {
                if quote == b'\'' && i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    i += 1;
                } else {
                    quote = 0;
                }
            }
        } else if (c == b'\'' || c == b'"') && (i == 0 || b" \t[:,{".contains(&bytes[i - 1])) {
            quote = c;
        } else if c == b'#' && (i == 0 || bytes[i - 1] == b' ' || bytes[i - 1] == b'\t') {
            return s[..i].trim_end_matches([' ', '\t']);
        }
        i += 1;
    }
    s
}

fn parse_value(raw: &str) -> Result<Value> {
    let raw = raw.trim();
    if raw.is_empty() || raw == "~" || raw.eq_ignore_ascii_case("null") {
        return Ok(Value::Null);
    }
    match raw.as_bytes()[0] {
        b'"' => {
            let s: String =
                serde_json::from_str(raw).map_err(|e| anyhow!("invalid quoted string: {e}"))?;
            Ok(Value::String(s))
        }
        b'\'' => {
            if raw.len() < 2 || !raw.ends_with('\'') {
                return Err(anyhow!("unterminated quoted string"));
            }
            let inside = &raw[1..raw.len() - 1];
            let bytes = inside.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'\'' {
                    if i + 1 >= bytes.len() || bytes[i + 1] != b'\'' {
                        return Err(anyhow!("single quote must be doubled"));
                    }
                    i += 1;
                }
                i += 1;
            }
            Ok(Value::String(inside.replace("''", "'")))
        }
        b'[' => {
            if !raw.ends_with(']') {
                return Err(anyhow!("unclosed list"));
            }
            let mut out = Vec::new();
            for part in split_flow(&raw[1..raw.len() - 1])? {
                out.push(parse_value(&part)?);
            }
            Ok(Value::Array(out))
        }
        b'{' => {
            if !raw.ends_with('}') {
                return Err(anyhow!("unclosed map"));
            }
            let mut out = serde_json::Map::new();
            for part in split_flow(&raw[1..raw.len() - 1])? {
                let Some((key, val)) = part.split_once(':') else {
                    return Err(anyhow!("expected map key: value"));
                };
                let k = parse_value(key.trim())?;
                let Some(name) = k.as_str() else {
                    return Err(anyhow!("map keys must be strings"));
                };
                if name.is_empty() {
                    return Err(anyhow!("map keys must be strings"));
                }
                if out.contains_key(name) {
                    return Err(anyhow!("duplicate map key {name}"));
                }
                out.insert(name.to_string(), parse_value(val)?);
            }
            Ok(Value::Object(out))
        }
        b'&' | b'*' | b'!' | b'|' | b'>' => Err(anyhow!("unsupported YAML value {raw:?}")),
        _ => {
            if raw == "true" || raw == "false" {
                return Ok(Value::Bool(raw == "true"));
            }
            if serde_json::from_str::<Value>(raw).is_ok()
                && let Ok(n) = serde_json::from_str::<f64>(raw)
                && let Some(num) = Number::from_f64(n)
            {
                return Ok(Value::Number(num));
            }
            Ok(Value::String(raw.to_string()))
        }
    }
}

fn split_flow(s: &str) -> Result<Vec<String>> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut depth = 0i32;
    let mut quote = 0u8;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if quote != 0 {
            if c == b'\\' && quote == b'"' {
                i += 1;
            } else if c == quote {
                if quote == b'\'' && i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    i += 1;
                } else {
                    quote = 0;
                }
            }
            i += 1;
            continue;
        }
        match c {
            b'\'' | b'"'
                if s[start..i].trim().is_empty() || (i > 0 && b" [{:".contains(&bytes[i - 1])) =>
            {
                quote = c;
            }
            b'[' | b'{' => depth += 1,
            b']' | b'}' => {
                depth -= 1;
                if depth < 0 {
                    return Err(anyhow!("unbalanced flow collection"));
                }
            }
            b',' if depth == 0 => {
                let part = s[start..i].trim();
                if part.is_empty() {
                    return Err(anyhow!("empty flow collection item"));
                }
                out.push(part.to_string());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if quote != 0 || depth != 0 {
        return Err(anyhow!("unclosed quote or flow collection"));
    }
    let part = s[start..].trim();
    if !part.is_empty() {
        out.push(part.to_string());
    }
    Ok(out)
}

pub fn render_markdown(_rec: &pulse_store::Record, rev: &pulse_store::Revision) -> String {
    if rev.frontmatter.is_empty() {
        return rev.body.clone();
    }
    let mut b = String::from("---\n");
    for (key, value) in &rev.frontmatter {
        b.push_str(key);
        b.push_str(": ");
        b.push_str(&render_value(value));
        b.push('\n');
    }
    b.push_str("---\n");
    b.push_str(&rev.body);
    b
}

fn render_value(v: &Value) -> String {
    pulse_store::gojson::to_string(v)
        .unwrap_or_else(|e| panic!("records: frontmatter must contain JSON values: {e}"))
}

pub use parse_frontmatter as ParseFrontmatter;
pub use render_markdown as RenderMarkdown;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::apply_columns;
    use serde_json::json;
    use std::path::PathBuf;
    use std::process::Command;

    const FIXTURE_REF: &str = "02691f8";

    fn source_repo() -> Option<PathBuf> {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .ok()?;
        let ok = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["cat-file", "-e", &format!("{FIXTURE_REF}^{{commit}}")])
            .status()
            .ok()?
            .success();
        ok.then_some(repo)
    }

    fn git_fixture(repo: &std::path::Path, args: &[&str]) -> String {
        let out = Command::new(pulse_gitenv::git_program())
            .arg("-C")
            .arg(repo)
            .args(args)
            .env_clear()
            .envs(crate::import::git_repo_env())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@x")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@x")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?}: {}\n{}",
            args,
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    #[test]
    fn parse_frontmatter() {
        let _serial = crate::test_helpers::serial();
        let text = "---\r\nid: ADR-019\r\ntitle: \"A title: with # and quotes\"\r\ntier: 2\r\nareas: [agents, 'data, storage', \"a#b\"] # comment\r\ntags:\r\n  - one\r\n  - 'it''s two'\r\ndelivered:\r\nenv: {\"MODE\": \"test\"}\r\nactive: true\r\n---\r\n\r\n# Body\r\n---\r\n";
        let (front, body) = super::parse_frontmatter(text).unwrap();
        assert_eq!(front["id"], json!("ADR-019"));
        assert_eq!(front["title"], json!("A title: with # and quotes"));
        assert_eq!(front["tier"], json!(2.0));
        assert_eq!(front["areas"], json!(["agents", "data, storage", "a#b"]));
        assert_eq!(front["tags"], json!(["one", "it's two"]));
        assert!(front["delivered"].is_null());
        assert_eq!(front["env"], json!({"MODE": "test"}));
        assert_eq!(front["active"], json!(true));
        assert_eq!(body, "\r\n# Body\r\n---\r\n");
        let rec = pulse_store::Record::default();
        let rev = pulse_store::Revision {
            frontmatter: front.clone(),
            body: body.clone(),
            ..pulse_store::Revision::default()
        };
        let (again, again_body) = super::parse_frontmatter(&render_markdown(&rec, &rev)).unwrap();
        assert_eq!(again, front);
        assert_eq!(again_body, body);
    }

    #[test]
    fn multiline_frontmatter() {
        let _serial = crate::test_helpers::serial();
        let cases = [
            (
                "folded",
                ">\n  a long\n  description\n\n  second paragraph\n",
                "a long description\nsecond paragraph\n",
            ),
            (
                "literal",
                "|\n  a long\n  description\n\n",
                "a long\ndescription\n",
            ),
            (
                "stripped",
                "|-\n  line one\n  line two\n\n",
                "line one\nline two",
            ),
            ("kept", "|+\n  first\n\n\n", "first\n\n\n"),
            (
                "folded-indent",
                ">-\n  first\n    indented\n  last\n",
                "first\n  indented\nlast",
            ),
        ];
        for (name, value, want) in cases {
            let (front, body) = super::parse_frontmatter(&format!(
                "---\ndescription: {value}next: okay\n---\nBody\n"
            ))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(front["description"], json!(want), "{name}");
            assert_eq!(front["next"], json!("okay"), "{name}");
            assert_eq!(body, "Body\n");
        }
    }

    #[test]
    fn frontmatter_errors() {
        let _serial = crate::test_helpers::serial();
        for text in [
            "---\nx: y\n",
            "---\nnot a key\n---\n",
            "---\nx: a\nx: b\n---\n",
            "---\nx: [a, b\n---\n",
            "---\nx: 'unterminated\n---\n",
            "---\nx: [a,,b]\n---\n",
            "---\nx: *alias\n---\n",
            "---\n  unexpected: indentation\n---\n",
            "---\nx: {a: 1, a: 2}\n---\n",
        ] {
            assert!(super::parse_frontmatter(text).is_err(), "{text:?}");
        }
        for text in ["# Heading\n\nBody\n", "", "not YAML\n---\nbody"] {
            let (front, body) = super::parse_frontmatter(text).unwrap();
            assert!(front.is_empty());
            assert_eq!(body, text);
        }
    }

    #[test]
    fn real_frontmatter_samples() {
        let _serial = crate::test_helpers::serial();
        let Some(repo) = source_repo() else {
            return;
        };
        let samples = [
            (
                "docs/decisions/ADR-019-agent-first-pulse-store-and-runner.md",
                "Agent-First PULSE",
            ),
            (
                "docs/plans/PLAN-026-project-environment-recipes.md",
                "Project Environment Recipes",
            ),
            ("docs/plans/WU-022-decision-hook.md", "Decision Hook"),
            (
                "docs/features/016-project-environment-recipes.md",
                "Project Environment Recipes",
            ),
            ("docs/memory/product.md", "Memory - Product"),
            ("docs/workflows/board.md", "Workflow - Agents' Board"),
            ("docs/skills/pulse-plan/SKILL.md", "PULSE Plan"),
            ("docs/guides/new-repo.md", "Start a New Repository"),
            ("docs/context/environment.md", "Environment"),
            (
                "docs/architecture/overview.md",
                "PULSE Architecture Overview",
            ),
            (
                "docs/evolution/constitution.md",
                "PULSE Evolution Constitution",
            ),
            ("docs/usage/usage-log.md", "Archived Token Usage Log"),
            (
                "docs/prompts/shared/eli5.prompt.md",
                "Shared ELI5 Output Format",
            ),
            ("AGENTS.md", "PULSE - Agent Operating Guide"),
        ];
        for (file, title) in samples {
            let text = git_fixture(&repo, &["show", &format!("{FIXTURE_REF}:{file}")]);
            let (front, body) = super::parse_frontmatter(&text).unwrap();
            let mut rec = pulse_store::Record::default();
            apply_columns(&mut rec, &front, &body).unwrap();
            assert!(
                rec.title.starts_with(title),
                "{file}: title = {:?}, want prefix {:?}",
                rec.title,
                title
            );
            let rev = pulse_store::Revision {
                frontmatter: front.clone(),
                body: body.clone(),
                ..pulse_store::Revision::default()
            };
            let (round_front, round_body) =
                super::parse_frontmatter(&render_markdown(&rec, &rev)).unwrap();
            assert_eq!(round_front, front, "{file}");
            assert_eq!(round_body, body, "{file}");
        }
    }
}
