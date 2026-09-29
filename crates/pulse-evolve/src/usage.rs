use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, Read};
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct UsageOptions {
    pub runner: String,
    pub days: i64,
    pub home: String,
    pub now: Option<DateTime<Utc>>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageSession {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "models")]
    pub models: Vec<String>,
    #[serde(rename = "input")]
    pub input: Option<i64>,
    #[serde(rename = "output")]
    pub output: Option<i64>,
    #[serde(rename = "cache_read")]
    pub cache_read: Option<i64>,
    #[serde(rename = "cache_write")]
    pub cache_write: Option<i64>,
    #[serde(rename = "cache")]
    pub cache: Option<i64>,
    #[serde(rename = "subagent_total")]
    pub subagent_total: Option<i64>,
    #[serde(rename = "total")]
    pub total: Option<i64>,
    #[serde(rename = "messages")]
    pub messages: i64,
    #[serde(rename = "mode")]
    pub mode: String,
    #[serde(rename = "updated")]
    pub updated: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunnerUsage {
    #[serde(rename = "runner")]
    pub runner: String,
    #[serde(rename = "status")]
    pub status: String,
    #[serde(rename = "sessions")]
    pub sessions: Vec<UsageSession>,
    #[serde(rename = "warnings")]
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone)]
pub(crate) struct LocalUsageFile {
    pub(crate) path: String,
    pub(crate) at: DateTime<Utc>,
}
/// Go: func usageFiles(roots []string, extension string, cutoff time.Time) ([]localUsageFile, int)
pub(crate) fn usage_files(
    roots: &[String],
    extension: &str,
    cutoff: DateTime<Utc>,
) -> (Vec<LocalUsageFile>, i64) {
    let mut files = Vec::new();
    let mut seen = BTreeMap::new();
    let mut problems = 0;
    for root in roots {
        walk_usage(
            Path::new(root),
            extension,
            cutoff,
            &mut seen,
            &mut files,
            &mut problems,
        );
    }
    files.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.path.cmp(&b.path)));
    (files, problems)
}
/// Go: func object(value any) map[string]any
pub(crate) fn object(value: &Value) -> Option<Map<String, Value>> {
    value.as_object().cloned()
}
/// Go: func text(value any) string
pub(crate) fn text(value: &Value) -> String {
    value.as_str().unwrap_or("").to_string()
}
/// Go: func objectOrJSON(value any) map[string]any
pub(crate) fn object_or_json(value: &Value) -> Option<Map<String, Value>> {
    object(value).or_else(|| {
        value
            .as_str()
            .and_then(|s| decode_object(s.as_bytes()).ok())
    })
}
/// Go: func decodeObject(data []byte) (map[string]any, error)
pub(crate) fn decode_object(data: &[u8]) -> Result<Map<String, Value>> {
    let mut de = serde_json::Deserializer::from_slice(data).into_iter::<Value>();
    let Some(value) = de.next() else {
        return Err(anyhow!("EOF"));
    };
    let value = value?;
    if de.next().is_some() {
        return Err(anyhow!("more than one JSON value"));
    }
    object(&value).ok_or_else(|| anyhow!("not an object"))
}
/// Go: func usageLines(file string, each func(map[string]any)) (bad int)
pub(crate) fn usage_lines<F>(file: &str, each: F) -> i64
where
    F: FnMut(Map<String, Value>),
{
    let f = match std::fs::File::open(file) {
        Ok(f) => f,
        Err(_) => return 1,
    };
    let mut bad = 0;
    let reader = std::io::BufReader::new(f);
    let mut each = each;
    for line in reader.lines() {
        match line {
            Ok(line) if line.trim().is_empty() => {}
            Ok(line) => match decode_object(line.as_bytes()) {
                Ok(row) => each(row),
                Err(_) => bad += 1,
            },
            Err(_) => bad += 1,
        }
    }
    bad
}
/// Go: func token(value any) *int64
pub(crate) fn token(value: &Value) -> Option<i64> {
    let n = if let Some(n) = value.as_i64() {
        n
    } else if let Some(s) = value.as_str() {
        s.parse().ok()?
    } else {
        return None;
    };
    (n >= 0).then_some(n)
}
/// Go: func addToken(dest **int64, value *int64)
pub(crate) fn add_token(dest: &mut Option<i64>, value: Option<i64>) {
    if let Some(value) = value {
        match dest {
            Some(dest) => {
                if value <= i64::MAX - *dest {
                    *dest += value;
                }
            }
            None => *dest = Some(value),
        }
    }
}
/// Go: func sumTokens(values ...*int64) *int64
pub(crate) fn sum_tokens(values: &[Option<i64>]) -> Option<i64> {
    let mut total = None;
    for v in values {
        add_token(&mut total, *v);
    }
    total
}
/// Go: func addModel(session *UsageSession, model string)
pub(crate) fn add_model(session: &mut UsageSession, model: &str) {
    if model.is_empty()
        || model == "<synthetic>"
        || pulse_store::secret_shaped(model)
        || model.contains(['\r', '\n', '\0'])
    {
        return;
    }
    if !session.models.iter().any(|m| m == model) {
        session.models.push(model.to_string());
        session.models.sort();
    }
}
/// Go: func newUsage(id string, at time.Time) *UsageSession
pub(crate) fn new_usage(id: &str, at: DateTime<Utc>) -> UsageSession {
    let mut id = id.to_string();
    if pulse_store::secret_shaped(&id) || id.contains(['\r', '\n', '\0']) {
        id = "[local session]".into();
    }
    UsageSession {
        id,
        models: Vec::new(),
        updated: at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        mode: "partial".into(),
        ..UsageSession::default()
    }
}
/// Go: func hasTokens(s *UsageSession) bool
pub(crate) fn has_tokens(s: &UsageSession) -> bool {
    s.input.is_some()
        || s.output.is_some()
        || s.cache_read.is_some()
        || s.cache_write.is_some()
        || s.subagent_total.is_some()
}
/// Go: func collectCopilot(files []localUsageFile) ([]UsageSession, int)
pub(crate) fn collect_copilot(files: &[LocalUsageFile]) -> (Vec<UsageSession>, i64) {
    let mut out = Vec::new();
    let mut bad = 0;
    for f in files {
        if Path::new(&f.path).file_name().and_then(|s| s.to_str()) != Some("events.jsonl") {
            continue;
        }
        let id = Path::new(&f.path)
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let mut live = new_usage(id, f.at);
        let mut shutdown: Option<Map<String, Value>> = None;
        bad += usage_lines(&f.path, |e| {
            let data = object(e.get("data").unwrap_or(&Value::Null)).unwrap_or_default();
            let kind = text(e.get("type").unwrap_or(&Value::Null));
            if matches!(
                kind.as_str(),
                "session.resume"
                    | "assistant.turn_start"
                    | "assistant.message"
                    | "subagent.completed"
                    | "session.model_change"
            ) {
                shutdown = None;
            }
            match kind.as_str() {
                "assistant.message" => {
                    add_token(
                        &mut live.output,
                        token(data.get("outputTokens").unwrap_or(&Value::Null)),
                    );
                    live.messages += 1;
                    add_model(&mut live, &text(data.get("model").unwrap_or(&Value::Null)));
                }
                "subagent.completed" => {
                    add_token(
                        &mut live.subagent_total,
                        token(data.get("totalTokens").unwrap_or(&Value::Null)),
                    );
                    add_model(&mut live, &text(data.get("model").unwrap_or(&Value::Null)));
                }
                "session.start" | "session.resume" => add_model(
                    &mut live,
                    &text(data.get("selectedModel").unwrap_or(&Value::Null)),
                ),
                "session.model_change" => add_model(
                    &mut live,
                    &text(data.get("newModel").unwrap_or(&Value::Null)),
                ),
                "session.shutdown" => shutdown = Some(data),
                _ => {}
            }
        });
        let mut result = live.clone();
        if let Some(shutdown) = shutdown {
            let mut final_s = new_usage(id, f.at);
            final_s.mode = "final".into();
            final_s.messages = live.messages;
            let metrics =
                object(shutdown.get("modelMetrics").unwrap_or(&Value::Null)).unwrap_or_default();
            if !metrics.is_empty() {
                for (model, entry) in metrics {
                    let usage = object(
                        &object(&entry)
                            .unwrap_or_default()
                            .get("usage")
                            .cloned()
                            .unwrap_or(Value::Null),
                    )
                    .unwrap_or_default();
                    add_token(
                        &mut final_s.input,
                        token(usage.get("inputTokens").unwrap_or(&Value::Null)),
                    );
                    add_token(
                        &mut final_s.output,
                        token(usage.get("outputTokens").unwrap_or(&Value::Null)),
                    );
                    add_token(
                        &mut final_s.cache_read,
                        token(usage.get("cacheReadTokens").unwrap_or(&Value::Null)),
                    );
                    add_token(
                        &mut final_s.cache_write,
                        token(usage.get("cacheWriteTokens").unwrap_or(&Value::Null)),
                    );
                    add_model(&mut final_s, &model);
                }
            } else {
                let details = object_or_json(shutdown.get("tokenDetails").unwrap_or(&Value::Null))
                    .unwrap_or_default();
                let obj =
                    |k: &str| object(details.get(k).unwrap_or(&Value::Null)).unwrap_or_default();
                final_s.cache_read =
                    token(obj("cache_read").get("tokenCount").unwrap_or(&Value::Null));
                final_s.cache_write =
                    token(obj("cache_write").get("tokenCount").unwrap_or(&Value::Null));
                final_s.input = sum_tokens(&[
                    token(obj("input").get("tokenCount").unwrap_or(&Value::Null)),
                    final_s.cache_read,
                    final_s.cache_write,
                ]);
                final_s.output = token(obj("output").get("tokenCount").unwrap_or(&Value::Null));
                final_s.models = live.models.clone();
                final_s.mode = "final, main agent".into();
            }
            if has_tokens(&final_s) {
                final_s.cache = sum_tokens(&[final_s.cache_read, final_s.cache_write]);
                final_s.total = sum_tokens(&[final_s.input, final_s.output]);
                result = final_s;
            }
        }
        if has_tokens(&result) {
            out.push(result);
        }
    }
    (out, bad)
}
/// Go: func collectClaude(files []localUsageFile) ([]UsageSession, int)
pub(crate) fn collect_claude(files: &[LocalUsageFile]) -> (Vec<UsageSession>, i64) {
    let mut sessions = BTreeMap::new();
    let mut seen = BTreeMap::new();
    let mut bad = 0;
    for f in files {
        let path = Path::new(&f.path);
        let mut id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if path.file_name().and_then(|s| s.to_str()) == Some("chat.jsonl") {
            id = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .into();
        }
        bad += usage_lines(&f.path, |e| {
            let message = object(e.get("message").unwrap_or(&Value::Null)).unwrap_or_default();
            let usage = object(message.get("usage").unwrap_or(&Value::Null)).unwrap_or_default();
            if usage.is_empty() {
                return;
            }
            let key = format!(
                "{}\0{}",
                text(message.get("id").unwrap_or(&Value::Null)),
                text(e.get("requestId").unwrap_or(&Value::Null))
            );
            if key != "\0" && seen.insert(key, true).is_some() {
                return;
            }
            let s = sessions
                .entry(id.clone())
                .or_insert_with(|| new_usage(&id, f.at));
            add_token(
                &mut s.input,
                token(usage.get("input_tokens").unwrap_or(&Value::Null)),
            );
            add_token(
                &mut s.output,
                token(usage.get("output_tokens").unwrap_or(&Value::Null)),
            );
            add_token(
                &mut s.cache_write,
                token(
                    usage
                        .get("cache_creation_input_tokens")
                        .unwrap_or(&Value::Null),
                ),
            );
            add_token(
                &mut s.cache_read,
                token(usage.get("cache_read_input_tokens").unwrap_or(&Value::Null)),
            );
            add_model(s, &text(message.get("model").unwrap_or(&Value::Null)));
            s.messages += 1;
        });
    }
    for s in sessions.values_mut() {
        s.cache = sum_tokens(&[s.cache_read, s.cache_write]);
        s.total = sum_tokens(&[s.input, s.output, s.cache]);
        s.mode = "observed messages".into();
    }
    (usage_session_list(sessions), bad)
}
/// Go: func collectCodex(files []localUsageFile) ([]UsageSession, int)
pub(crate) fn collect_codex(files: &[LocalUsageFile]) -> (Vec<UsageSession>, i64) {
    let mut sessions = BTreeMap::new();
    let mut bad = 0;
    for f in files {
        let mut s = new_usage(
            Path::new(&f.path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(""),
            f.at,
        );
        let mut usage: Option<Map<String, Value>> = None;
        bad += usage_lines(&f.path, |e| {
            let payload = object(e.get("payload").unwrap_or(&Value::Null)).unwrap_or_default();
            match text(e.get("type").unwrap_or(&Value::Null)).as_str() {
                "session_meta" => {
                    let id = text(payload.get("id").unwrap_or(&Value::Null));
                    if !id.is_empty() {
                        s.id = id;
                    }
                }
                "turn_context" => {
                    add_model(&mut s, &text(payload.get("model").unwrap_or(&Value::Null)))
                }
                _ => {}
            }
            let data = if text(payload.get("type").unwrap_or(&Value::Null)) == "token_count" {
                payload
            } else if text(e.get("type").unwrap_or(&Value::Null)) == "token_count" {
                e
            } else {
                return;
            };
            let info = object(data.get("info").unwrap_or(&Value::Null)).unwrap_or_default();
            let total =
                object(info.get("total_token_usage").unwrap_or(&Value::Null)).unwrap_or_default();
            if !total.is_empty() {
                usage = Some(total);
            }
        });
        if let Some(u) = usage {
            s.input = token(u.get("input_tokens").unwrap_or(&Value::Null));
            s.output = token(u.get("output_tokens").unwrap_or(&Value::Null));
            s.cache_read = token(u.get("cached_input_tokens").unwrap_or(&Value::Null));
            s.cache = s.cache_read;
            s.total = sum_tokens(&[s.input, s.output]);
            s.mode = "session cumulative".into();
            if has_tokens(&s) && !sessions.contains_key(&s.id) {
                sessions.insert(s.id.clone(), s);
            }
        }
    }
    (usage_session_list(sessions), bad)
}
/// Go: func collectOpenCode(files []localUsageFile) ([]UsageSession, int)
pub(crate) fn collect_open_code(files: &[LocalUsageFile]) -> (Vec<UsageSession>, i64) {
    let mut sessions = BTreeMap::new();
    let mut seen = BTreeMap::new();
    let mut bad = 0;
    for f in files {
        let mut data = Vec::new();
        if std::fs::File::open(&f.path)
            .and_then(|mut file| file.by_ref().take((8 << 20) + 1).read_to_end(&mut data))
            .is_err()
            || data.len() > 8 << 20
        {
            bad += 1;
            continue;
        }
        let Ok(row) = decode_object(&data) else {
            bad += 1;
            continue;
        };
        if text(row.get("role").unwrap_or(&Value::Null)) != "assistant" {
            continue;
        }
        let id = crate::card::first_text(&[
            &text(row.get("sessionID").unwrap_or(&Value::Null)),
            Path::new(&f.path)
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or(""),
        ]);
        let msg_id = text(row.get("id").unwrap_or(&Value::Null));
        let key = format!("{id}\0{msg_id}");
        if !msg_id.is_empty() && seen.insert(key, true).is_some() {
            continue;
        }
        let usage = object(row.get("tokens").unwrap_or(&Value::Null)).unwrap_or_default();
        if usage.is_empty() {
            continue;
        }
        let s = sessions
            .entry(id.clone())
            .or_insert_with(|| new_usage(&id, f.at));
        add_token(
            &mut s.input,
            token(usage.get("input").unwrap_or(&Value::Null)),
        );
        add_token(
            &mut s.output,
            token(usage.get("output").unwrap_or(&Value::Null)),
        );
        let cache = object(usage.get("cache").unwrap_or(&Value::Null)).unwrap_or_default();
        add_token(
            &mut s.cache_read,
            token(cache.get("read").unwrap_or(&Value::Null)),
        );
        add_token(
            &mut s.cache_write,
            token(cache.get("write").unwrap_or(&Value::Null)),
        );
        add_model(s, &text(row.get("modelID").unwrap_or(&Value::Null)));
        s.messages += 1;
    }
    for s in sessions.values_mut() {
        s.cache = sum_tokens(&[s.cache_read, s.cache_write]);
        s.total = sum_tokens(&[s.input, s.output, s.cache]);
        s.mode = "observed messages".into();
    }
    (usage_session_list(sessions), bad)
}
/// Go: func usageSessionList(sessions map[string]*UsageSession) []UsageSession
pub(crate) fn usage_session_list(sessions: BTreeMap<String, UsageSession>) -> Vec<UsageSession> {
    let mut out: Vec<_> = sessions.into_values().filter(has_tokens).collect();
    out.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
    out
}
/// Go: func Usage(opts UsageOptions) ([]RunnerUsage, error)
pub fn usage(opts: UsageOptions) -> Result<Vec<RunnerUsage>> {
    let mut opts = opts;
    if opts.days < 0 || opts.days > 36500 {
        return Err(crate::fixtures::input_error(
            "Cannot inspect usage",
            "days must be between 0 (all history) and 36500",
            "pulse usage --days 7",
        ));
    }
    let now = opts.now.unwrap_or_else(Utc::now);
    if opts.home.is_empty() {
        opts.home = std::env::var("HOME").map_err(|e| anyhow!(e))?;
    }
    let cutoff = if opts.days != 0 {
        now - chrono::Duration::days(opts.days)
    } else {
        DateTime::<Utc>::MIN_UTC
    };
    let mut runners = vec![
        "copilot".to_string(),
        "claude".to_string(),
        "codex".to_string(),
        "opencode".to_string(),
    ];
    if !opts.runner.is_empty() && opts.runner != "all" {
        if !runners.iter().any(|r| r == &opts.runner) {
            return Err(crate::fixtures::input_error(
                "Unknown usage runner",
                "choose copilot, claude, codex or opencode",
                "pulse usage --help",
            ));
        }
        runners = vec![opts.runner.clone()];
    }
    let override_env = |name: &str, fallback: String| {
        std::env::var(name)
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or(fallback)
    };
    let mut reports = Vec::new();
    for name in runners {
        let mut extension = ".jsonl";
        let roots = match name.as_str() {
            "copilot" => vec![override_env(
                "COPILOT_STATE_DIR",
                Path::new(&opts.home)
                    .join(".copilot/session-state")
                    .display()
                    .to_string(),
            )],
            "claude" => vec![
                Path::new(&override_env(
                    "CLAUDE_CONFIG_DIR",
                    Path::new(&opts.home)
                        .join(".config/claude")
                        .display()
                        .to_string(),
                ))
                .join("projects")
                .display()
                .to_string(),
                Path::new(&opts.home)
                    .join(".claude/projects")
                    .display()
                    .to_string(),
            ],
            "codex" => vec![
                Path::new(&override_env(
                    "CODEX_HOME",
                    Path::new(&opts.home).join(".codex").display().to_string(),
                ))
                .join("sessions")
                .display()
                .to_string(),
            ],
            "opencode" => {
                extension = ".json";
                if let Ok(root) = std::env::var("OPENCODE_STORE") {
                    if !root.is_empty() {
                        vec![root]
                    } else {
                        vec![
                            Path::new(&opts.home)
                                .join(".local/share/opencode/storage/message")
                                .display()
                                .to_string(),
                            Path::new(&opts.home)
                                .join(".config/opencode/storage/message")
                                .display()
                                .to_string(),
                        ]
                    }
                } else {
                    vec![
                        Path::new(&opts.home)
                            .join(".local/share/opencode/storage/message")
                            .display()
                            .to_string(),
                        Path::new(&opts.home)
                            .join(".config/opencode/storage/message")
                            .display()
                            .to_string(),
                    ]
                }
            }
            _ => Vec::new(),
        };
        let (files, bad) = usage_files(&roots, extension, cutoff);
        let (sessions, parse_bad) = match name.as_str() {
            "copilot" => collect_copilot(&files),
            "claude" => collect_claude(&files),
            "codex" => collect_codex(&files),
            "opencode" => collect_open_code(&files),
            _ => (Vec::new(), 0),
        };
        let mut report = RunnerUsage {
            runner: name,
            status: "not found".into(),
            sessions,
            warnings: Vec::new(),
        };
        if !report.sessions.is_empty() {
            report.status = "found".into();
        }
        if bad + parse_bad > 0 {
            report.warnings.push(format!(
                "{} unreadable or malformed local record(s) skipped",
                bad + parse_bad
            ));
        }
        reports.push(report);
    }
    Ok(reports)
}

fn walk_usage(
    root: &Path,
    extension: &str,
    cutoff: DateTime<Utc>,
    seen: &mut BTreeMap<String, bool>,
    files: &mut Vec<LocalUsageFile>,
    problems: &mut i64,
) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for ent in rd {
        let Ok(ent) = ent else {
            *problems += 1;
            continue;
        };
        let path = ent.path();
        let Ok(ft) = ent.file_type() else {
            *problems += 1;
            continue;
        };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            walk_usage(&path, extension, cutoff, seen, files, problems);
            continue;
        }
        let name = path.display().to_string();
        if !name.ends_with(extension) || seen.insert(name.clone(), true).is_some() {
            continue;
        }
        let Ok(meta) = ent.metadata() else {
            *problems += 1;
            continue;
        };
        if !meta.is_file() {
            *problems += 1;
            continue;
        }
        let at: DateTime<Utc> = meta
            .modified()
            .map(DateTime::<Utc>::from)
            .unwrap_or(DateTime::<Utc>::MIN_UTC);
        if at >= cutoff {
            files.push(LocalUsageFile { path: name, at });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use std::collections::BTreeMap;
    use std::path::Path;

    #[test]
    fn usage_collectors_only_read_local_token_fields() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let home = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("HOME", home.path());
            for key in [
                "COPILOT_STATE_DIR",
                "CLAUDE_CONFIG_DIR",
                "CODEX_HOME",
                "OPENCODE_STORE",
            ] {
                std::env::set_var(key, "");
            }
        }
        put_file(
            home.path()
                .join(".copilot/session-state/copilot-final/events.jsonl"),
            r#"{"type":"assistant.message","data":{"outputTokens":9,"content":"RAW_CONTENT_NOT_FOR_OUTPUT"}}
{"type":"subagent.completed","data":{"totalTokens":99}}
{"type":"session.shutdown","data":{"modelMetrics":{"model":{"usage":{"inputTokens":100,"outputTokens":25,"cacheReadTokens":80,"cacheWriteTokens":5}}}}}
"#,
            0o600,
        );
        put_file(
            home.path()
                .join(".copilot/session-state/copilot-live/events.jsonl"),
            r#"{"type":"assistant.message","data":{"outputTokens":2}}
{"type":"session.shutdown","data":{"tokenDetails":{"input":{"tokenCount":50}}}}
{"type":"assistant.message","data":{"outputTokens":3}}
{"type":"subagent.completed","data":{"totalTokens":7}}
"#,
            0o600,
        );
        put_file(
            home.path()
                .join(".copilot/session-state/copilot-details/events.jsonl"),
            "{\"type\":\"session.shutdown\",\"data\":{\"tokenDetails\":\"{\\\"input\\\":{\\\"tokenCount\\\":10},\\\"output\\\":{\\\"tokenCount\\\":4},\\\"cache_read\\\":{\\\"tokenCount\\\":2},\\\"cache_write\\\":{\\\"tokenCount\\\":3}}\"}}\n",
            0o600,
        );
        let claude = "{\"requestId\":\"request\",\"message\":{\"id\":\"message\",\"model\":\"claude\",\"content\":\"RAW_CONTENT_NOT_FOR_OUTPUT\",\"usage\":{\"input_tokens\":10,\"output_tokens\":20,\"cache_creation_input_tokens\":30,\"cache_read_input_tokens\":40}}}\n";
        put_file(
            home.path()
                .join(".claude/projects/project/claude-session.jsonl"),
            &(claude.to_string() + claude + "malformed\n"),
            0o600,
        );
        put_file(
            home.path().join(".codex/sessions/2026/09/rollout.jsonl"),
            r#"{"type":"session_meta","payload":{"id":"codex-session"}}
{"type":"turn_context","payload":{"model":"codex"}}
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":2}}}}
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"output_tokens":20,"cached_input_tokens":50},"last_token_usage":{"input_tokens":90,"output_tokens":18}}}}
"#,
            0o600,
        );
        let opencode = r#"{"id":"one","role":"assistant","sessionID":"open-session","modelID":"open","tokens":{"input":7,"output":2,"cache":{"read":3,"write":1}}}"#;
        put_file(
            home.path()
                .join(".config/opencode/storage/message/open-session/one.json"),
            opencode,
            0o600,
        );
        put_file(
            home.path()
                .join(".local/share/opencode/storage/message/open-session/one.json"),
            opencode,
            0o600,
        );
        let before = fingerprint(home.path());
        let reports = usage(UsageOptions {
            home: home.path().display().to_string(),
            days: 7,
            now: Some(Utc::now()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(reports.len(), 4);
        let mut by_id = BTreeMap::new();
        for report in &reports {
            assert_eq!(
                report.status, "found",
                "runner {} fixture was not found",
                report.runner
            );
            for session in &report.sessions {
                by_id.insert(session.id.clone(), session.clone());
            }
            if report.runner == "claude" {
                assert_eq!(report.warnings.len(), 1);
            }
        }
        assert_eq!(by_id["copilot-final"].input, Some(100));
        assert_eq!(by_id["copilot-final"].total, Some(125));
        assert_eq!(by_id["copilot-final"].cache, Some(85));
        assert_eq!(by_id["copilot-details"].total, Some(19));
        assert_eq!(by_id["copilot-live"].output, Some(5));
        assert_eq!(by_id["copilot-live"].subagent_total, Some(7));
        assert!(by_id["copilot-live"].input.is_none());
        assert!(by_id["copilot-live"].total.is_none());
        assert_eq!(by_id["claude-session"].total, Some(100));
        assert_eq!(by_id["claude-session"].messages, 1);
        assert_eq!(by_id["codex-session"].total, Some(120));
        assert_eq!(by_id["open-session"].total, Some(13));
        let encoded = pulse_store::gojson::to_string(&reports).unwrap();
        assert_private_absent(
            &encoded,
            &[
                "RAW_CONTENT_NOT_FOR_OUTPUT".into(),
                home.path().display().to_string(),
            ],
        );
        assert_eq!(before, fingerprint(home.path()));
    }

    #[test]
    fn usage_days_missing_and_configuration_overrides() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let home = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("HOME", home.path());
            for key in [
                "COPILOT_STATE_DIR",
                "CLAUDE_CONFIG_DIR",
                "CODEX_HOME",
                "OPENCODE_STORE",
            ] {
                std::env::set_var(key, "");
            }
        }
        let now = Utc::now();
        let file = home.path().join(".codex/sessions/old.jsonl");
        put_file(
            &file,
            "{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n",
            0o600,
        );
        assert!(
            std::process::Command::new("touch")
                .args(["-t", "202001010000"])
                .arg(&file)
                .status()
                .unwrap()
                .success()
        );
        let reports = usage(UsageOptions {
            home: home.path().display().to_string(),
            days: 7,
            now: Some(now),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(reports.len(), 4);
        for r in &reports {
            assert_eq!(r.status, "not found");
            assert!(r.sessions.is_empty());
        }
        let reports = usage(UsageOptions {
            home: home.path().display().to_string(),
            runner: "codex".into(),
            days: 0,
            now: Some(now),
        })
        .unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].status, "found");
        assert!(
            usage(UsageOptions {
                home: home.path().display().to_string(),
                runner: "unsupported".into(),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            usage(UsageOptions {
                home: home.path().display().to_string(),
                days: -1,
                ..Default::default()
            })
            .is_err()
        );
        let custom = tempfile::tempdir().unwrap();
        put_file(
            custom.path().join("override/events.jsonl"),
            "{\"type\":\"assistant.message\",\"data\":{\"outputTokens\":7}}\n",
            0o600,
        );
        unsafe { std::env::set_var("COPILOT_STATE_DIR", custom.path()) };
        let reports = usage(UsageOptions {
            home: home.path().display().to_string(),
            runner: "copilot".into(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(reports[0].sessions.len(), 1);
        assert_eq!(reports[0].sessions[0].output, Some(7));
    }

    fn fingerprint(root: &Path) -> Vec<(String, Vec<u8>)> {
        fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
            for ent in std::fs::read_dir(dir).unwrap().flatten() {
                let path = ent.path();
                if path.is_dir() {
                    walk(root, &path, out);
                } else if path.is_file() {
                    out.push((
                        path.strip_prefix(root).unwrap().display().to_string(),
                        std::fs::read(&path).unwrap(),
                    ));
                }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        out.sort();
        out
    }
}
