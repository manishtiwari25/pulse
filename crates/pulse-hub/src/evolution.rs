use crate::server::{Handler, JsonMap};
use anyhow::{Result, anyhow};
use rusqlite::types::ValueRef;
use serde_json::{Number, Value};

use crate::privacy::HELDOUT_SETTING;
const PRIVATE_VALUE: &str = "[owner-private]";

impl Handler {
    pub(crate) fn evolution(&self, project: String) -> Result<JsonMap> {
        let store = self
            .store
            .as_deref()
            .ok_or_else(|| anyhow!("store is not available"))?;
        let records = store.list_records(pulse_store::RecordFilter {
            project: project.clone(),
            kind: "evolution".to_string(),
            ..pulse_store::RecordFilter::default()
        })?;
        let mut archive = JsonMap::new();
        archive.insert("rows".into(), Value::Array(Vec::new()));
        let mut result = JsonMap::new();
        result.insert("constitution".into(), Value::Null);
        result.insert("evaluators".into(), Value::Null);
        result.insert(
            "archive".into(),
            Value::Object(archive.clone().into_iter().collect()),
        );
        result.insert("loop".into(), Value::Null);
        for record in records {
            if record.slug != "constitution"
                && record.slug != "evaluators"
                && record.slug != "archive"
            {
                continue;
            }
            let (head, revision) = store.get_record(&record.id, 0)?;
            let Some(revision) = revision else {
                continue;
            };
            if record.slug == "archive" {
                archive.insert("record".into(), go_value(&crate::addition_record(head)));
                archive.insert("body".into(), Value::String(revision.body));
            } else {
                let (seeded, from) = seeded_record(&head, &revision);
                let mut doc = JsonMap::new();
                doc.insert(
                    "record".into(),
                    go_value(&crate::addition_record(head.clone())),
                );
                doc.insert("body".into(), Value::String(revision.body));
                doc.insert("seeded".into(), Value::Bool(seeded));
                doc.insert("seeded_from".into(), Value::String(from));
                doc.insert("draft".into(), draft_summary(store, &head.id, true));
                doc.insert("draft_job".into(), draft_job_ref(store, &project));
                result.insert(record.slug, Value::Object(doc.into_iter().collect()));
            }
        }
        let conn = store.conn()?;
        let has_archive: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='evolution_archive'",
            [],
            |r| r.get(0),
        )?;
        if has_archive != 0 {
            let rows = query_rows(
                store,
                "SELECT id,project,suggestion_post,hypothesis,branch,baseline_score,variant_score,
			tolerance,kept,sandbox,run_id,at,note FROM evolution_archive WHERE project=? ORDER BY at DESC,id DESC",
                &[&project],
            )?;
            archive.insert(
                "rows".into(),
                Value::Array(
                    rows.into_iter()
                        .map(|m| Value::Object(m.into_iter().collect()))
                        .collect(),
                ),
            );
        }
        let heldout = store.setting(HELDOUT_SETTING, "");
        let mut present = false;
        if !heldout.is_empty() {
            match std::fs::metadata(&heldout) {
                Ok(info) => present = info.is_dir(),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    return Err(anyhow!(
                        "the owner-private held-out directory could not be inspected"
                    ));
                }
            }
        }
        result.insert(
            "archive".into(),
            Value::Object(archive.into_iter().collect()),
        );
        let mut loop_doc = JsonMap::new();
        loop_doc.insert(
            "loop".into(),
            Value::String(store.setting("evolve.loop", "")),
        );
        loop_doc.insert(
            "limits".into(),
            Value::String(store.setting("evolve.limits", "")),
        );
        loop_doc.insert(
            "tolerance".into(),
            Value::String(store.setting("evolve.tolerance", "")),
        );
        loop_doc.insert("stop".into(), Value::String(store.setting("stop", "")));
        loop_doc.insert("heldout_present".into(), Value::Bool(present));
        result.insert("loop".into(), Value::Object(loop_doc.into_iter().collect()));
        let (gates, gates_err) = self.evolution_gates(project.clone());
        result.insert(
            "gates".into(),
            if gates.is_empty() {
                Value::Null
            } else {
                Value::Object(gates.into_iter().collect())
            },
        );
        if !gates_err.is_empty() {
            result.insert("gates_error".into(), Value::String(gates_err));
        }
        let fixtures = self.evolution_fixtures(project)?;
        result.insert(
            "fixtures".into(),
            Value::Array(
                fixtures
                    .into_iter()
                    .map(|m| Value::Object(m.into_iter().collect()))
                    .collect(),
            ),
        );
        if !heldout.is_empty() {
            let mut v = Value::Object(result.clone().into_iter().collect());
            redact_evolution_path(&mut v, &heldout);
            if let Value::Object(map) = v {
                return Ok(map.into_iter().collect());
            }
        }
        Ok(result)
    }

    pub(crate) fn evolution_gates(&self, project: String) -> (JsonMap, String) {
        let Some(store) = self.store.as_deref() else {
            return (JsonMap::new(), "store is not available".to_string());
        };
        match pulse_evolve::status(store, &project) {
            Ok(status) => (go_map(&status), String::new()),
            Err(err) => {
                if let Some(cli) = err.downcast_ref::<pulse_cli::Error>() {
                    (JsonMap::new(), cli.what.clone())
                } else {
                    (JsonMap::new(), err.to_string())
                }
            }
        }
    }

    pub(crate) fn evolution_fixtures(&self, project: String) -> Result<Vec<JsonMap>> {
        let store = self
            .store
            .as_deref()
            .ok_or_else(|| anyhow!("store is not available"))?;
        let conn = store.conn()?;
        let has_fixtures: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='evolution_fixtures'",
            [],
            |r| r.get(0),
        )?;
        if has_fixtures == 0 {
            return Ok(Vec::new());
        }
        let fixtures = pulse_evolve::list_fixtures(store, &project, false)?;
        Ok(fixtures
            .into_iter()
            .map(|f| {
                let mut row = JsonMap::new();
                row.insert("name".into(), Value::String(f.name.clone()));
                row.insert("kind".into(), Value::String(f.kind));
                row.insert("metric".into(), Value::Bool(f.metric));
                row.insert("enabled".into(), Value::Bool(f.enabled));
                row.insert("weight".into(), number_f64(f.weight));
                row.insert("created_by".into(), Value::String(f.created_by));
                row.insert("at".into(), Value::String(f.at));
                row.insert(
                    "starter".into(),
                    Value::Bool(pulse_evolve::is_starter(&f.name)),
                );
                row
            })
            .collect())
    }
}

pub(crate) fn seeded_record(
    head: &pulse_store::Record,
    revision: &pulse_store::Revision,
) -> (bool, String) {
    let from = revision
        .frontmatter
        .get("seeded_from")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    (head.current_rev == 1 && !from.is_empty(), from)
}

pub(crate) fn draft_summary<S: AsRef<str>>(
    s: &pulse_store::Store,
    id: S,
    with_body: bool,
) -> Value {
    let Ok(Some(draft)) = s.get_draft(id.as_ref()) else {
        return Value::Null;
    };
    let mut out = JsonMap::new();
    out.insert("author".into(), Value::String(draft.author));
    out.insert("at".into(), Value::String(draft.at));
    out.insert("note".into(), Value::String(draft.note));
    out.insert(
        "base_rev".into(),
        Value::Number(Number::from(draft.base_rev)),
    );
    out.insert("job_id".into(), Value::String(draft.job_id));
    if with_body {
        out.insert("body".into(), Value::String(draft.body));
    } else {
        out.insert("body".into(), Value::Null);
    }
    Value::Object(out.into_iter().collect())
}

pub(crate) fn draft_job_ref(s: &pulse_store::Store, project: &str) -> Value {
    match pulse_evolve::open_draft_job(s, project) {
        Ok(Some(ref_)) => {
            let mut out = JsonMap::new();
            out.insert("id".into(), Value::String(ref_.id));
            out.insert("status".into(), Value::String(ref_.status));
            Value::Object(out.into_iter().collect())
        }
        _ => Value::Null,
    }
}

pub(crate) fn redact_evolution_path(value: &mut Value, private: &str) {
    match value {
        Value::Object(map) => {
            for child in map.values_mut() {
                if let Value::String(text) = child {
                    *text = text.replace(private, PRIVATE_VALUE);
                } else {
                    redact_evolution_path(child, private);
                }
            }
        }
        Value::Array(items) => {
            for child in items {
                redact_evolution_path(child, private);
            }
        }
        Value::String(text) => *text = text.replace(private, PRIVATE_VALUE),
        _ => {}
    }
}

pub(crate) fn evolution_summary(evolution: JsonMap) -> JsonMap {
    let loop_doc = as_obj(evolution.get("loop"));
    let archive = as_obj(evolution.get("archive"));
    let rows = archive
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let gates = evolution.get("gates").and_then(Value::as_object);
    let baselines = rows
        .iter()
        .filter(|row| {
            let m = row.as_object();
            pulse_evolve::is_baseline_row(
                &m.and_then(|m| m.get("hypothesis"))
                    .map(str_value)
                    .unwrap_or_default(),
                &row.as_object()
                    .and_then(|m| m.get("suggestion_post"))
                    .map(str_value)
                    .unwrap_or_default(),
            )
        })
        .count() as i64;
    let present = !evolution
        .get("constitution")
        .unwrap_or(&Value::Null)
        .is_null()
        || !evolution
            .get("evaluators")
            .unwrap_or(&Value::Null)
            .is_null()
        || archive.contains_key("record")
        || !rows.is_empty();
    let loop_name = if loop_doc.get("loop").map(str_value).unwrap_or_default() == "1" {
        "on"
    } else {
        "off"
    };
    let mut summary = JsonMap::new();
    summary.insert("evolution_present".into(), Value::String(flag(present)));
    summary.insert(
        "stop_present".into(),
        Value::String(loop_doc.get("stop").map(str_value).unwrap_or_default()),
    );
    summary.insert("missing_files".into(), Value::Array(Vec::new()));
    summary.insert("loop".into(), Value::String(loop_name.to_string()));
    summary.insert(
        "limits".into(),
        Value::String(loop_doc.get("limits").map(str_value).unwrap_or_default()),
    );
    summary.insert("limits_note".into(), Value::String(String::new()));
    summary.insert("baseline".into(), Value::String(String::new()));
    summary.insert(
        "count_baselines".into(),
        Value::String(baselines.to_string()),
    );
    summary.insert(
        "count_variants".into(),
        Value::String((rows.len() as i64 - baselines).to_string()),
    );
    summary.insert("count_incidents".into(), Value::String("0".to_string()));
    summary.insert("product_classes_ok".into(), Value::String("0".to_string()));
    summary.insert(
        "product_evaluators_ok".into(),
        Value::String("0".to_string()),
    );
    summary.insert("product_verdict".into(), Value::String(String::new()));
    summary.insert("hygiene_git".into(), Value::String(String::new()));
    summary.insert("last".into(), Value::String("5".to_string()));
    if present {
        let mut missing = Vec::new();
        for slug in ["constitution", "evaluators"] {
            let ok = gates
                .and_then(|g| g.get(slug))
                .and_then(Value::as_bool)
                .unwrap_or_else(|| !evolution.get(slug).unwrap_or(&Value::Null).is_null());
            if !ok {
                missing.push(Value::String(format!("EVO-{slug}")));
            }
        }
        summary.insert("missing_files".into(), Value::Array(missing));
    }
    let Some(gates) = gates else {
        let mut note = "evolution gates unavailable".to_string();
        if let Some(detail) = evolution
            .get("gates_error")
            .map(str_value)
            .filter(|s| !s.is_empty())
        {
            note.push_str(": ");
            note.push_str(&detail);
        }
        summary.insert("limits_note".into(), Value::String(note.clone()));
        summary.insert("product_verdict".into(), Value::String(note));
        return summary;
    };
    summary.insert(
        "limits_note".into(),
        Value::String(
            gates
                .get("limit_problem")
                .map(str_value)
                .unwrap_or_default(),
        ),
    );
    if !gates.get("baseline").unwrap_or(&Value::Null).is_null() {
        summary.insert(
            "baseline".into(),
            Value::String(gates.get("baseline_id").map(str_value).unwrap_or_default()),
        );
    }
    let Some(scope) = gates.get("product_scope").and_then(Value::as_object) else {
        summary.insert(
            "product_verdict".into(),
            Value::String("product scope unavailable".to_string()),
        );
        return summary;
    };
    let Some(allowed) = scope.get("allowed").and_then(Value::as_bool) else {
        summary.insert(
            "product_verdict".into(),
            Value::String("product scope unavailable".to_string()),
        );
        return summary;
    };
    let constitution = gates.get("constitution").and_then(Value::as_bool) == Some(true);
    let evaluators = gates.get("evaluators").and_then(Value::as_bool) == Some(true);
    summary.insert(
        "product_classes_ok".into(),
        Value::String(flag(allowed && constitution)),
    );
    summary.insert(
        "product_evaluators_ok".into(),
        Value::String(flag(allowed && evaluators)),
    );
    summary.insert(
        "product_verdict".into(),
        Value::String(scope.get("reason").map(str_value).unwrap_or_default()),
    );
    summary
}

fn query_rows(
    store: &pulse_store::Store,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<JsonMap>> {
    let conn = store.conn()?;
    let mut stmt = conn.prepare(sql)?;
    let names = stmt
        .column_names()
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let mut rows = stmt.query(args)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let mut m = JsonMap::new();
        for (i, name) in names.iter().enumerate() {
            m.insert(name.clone(), sql_value(row.get_ref(i)?));
        }
        out.push(m);
    }
    Ok(out)
}

fn sql_value(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::Number(Number::from(i)),
        ValueRef::Real(f) => number_f64(f),
        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).to_string()),
        ValueRef::Blob(b) => Value::String(String::from_utf8_lossy(b).to_string()),
    }
}

fn go_value<T: serde::Serialize>(v: &T) -> Value {
    let text = pulse_store::gojson::to_string(v).expect("serialize hub payload");
    serde_json::from_str(&text).expect("reparse hub payload")
}

fn go_map<T: serde::Serialize>(v: &T) -> JsonMap {
    match go_value(v) {
        Value::Object(map) => map.into_iter().collect(),
        _ => JsonMap::new(),
    }
}

fn number_f64(f: f64) -> Value {
    Number::from_f64(f)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

fn as_obj(v: Option<&Value>) -> JsonMap {
    v.and_then(Value::as_object)
        .map(|m| m.clone().into_iter().collect())
        .unwrap_or_default()
}

fn str_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn flag(b: bool) -> String {
    if b { "1" } else { "0" }.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assert_evolution_shape(data: &JsonMap) {
        for key in [
            "archive",
            "constitution",
            "evaluators",
            "fixtures",
            "gates",
            "loop",
        ] {
            assert!(data.contains_key(key), "missing {key}: {data:?}");
        }
        assert!(data["loop"].is_object());
        assert!(data["archive"].is_object());
    }

    fn assert_evolution_summary_status_truth_shape(summary: &JsonMap) {
        let want_keys = [
            "baseline",
            "count_baselines",
            "count_incidents",
            "count_variants",
            "evolution_present",
            "hygiene_git",
            "last",
            "limits",
            "limits_note",
            "loop",
            "missing_files",
            "product_classes_ok",
            "product_evaluators_ok",
            "product_verdict",
            "stop_present",
        ];
        let mut got_keys = summary.keys().map(String::as_str).collect::<Vec<_>>();
        got_keys.sort_unstable();
        assert_eq!(got_keys, want_keys, "summary keys changed: {summary:?}");
        for (key, value) in summary {
            if key == "missing_files" {
                assert!(
                    value.is_array(),
                    "missing_files must remain []string: {value:?}"
                );
            } else {
                assert!(value.is_string(), "{key} must remain a string: {value:?}");
            }
        }
    }

    #[test]
    fn evolution_sql_contract() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_http::additions_store(false);
        crate::test_helpers_http::additions_project(&s, "one");
        let h = crate::test_helpers_http::handler(&s);
        let mut data = h.evolution("one".to_string()).unwrap();
        assert_evolution_shape(&data);
        assert!(data["constitution"].is_null());
        assert!(data["evaluators"].is_null());
        crate::test_helpers_http::additions_record(
            &s,
            "EVO-constitution-one",
            "one",
            "evolution",
            "constitution",
            "## Purpose\nImprove carefully.",
        );
        crate::test_helpers_http::additions_record(
            &s,
            "EVO-evaluators-one",
            "one",
            "evolution",
            "evaluators",
            "Public evaluator descriptions.",
        );
        crate::test_helpers_http::additions_record(
            &s,
            "EVO-archive-one",
            "one",
            "evolution",
            "archive",
            "## Historical archive\n| ID | Decision |\n|---|---|\n| V-001 | kept |",
        );
        data = h.evolution("one".to_string()).unwrap();
        assert_evolution_shape(&data);
        assert_eq!(
            data["constitution"]["body"],
            "## Purpose\nImprove carefully."
        );
        assert_eq!(data["evaluators"]["body"], "Public evaluator descriptions.");
        assert!(data["archive"]["body"].as_str().unwrap().contains("V-001"));
    }

    #[test]
    fn evolution_archive_and_heldout_privacy() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_http::additions_store(false);
        let private = tempfile::tempdir().unwrap();
        for (key, value) in [
            ("evolve.heldout_dir", private.path().to_str().unwrap()),
            ("evolve.loop", "1"),
            ("evolve.limits", "cycles=5"),
            ("evolve.tolerance", "0.05"),
            ("stop", "1"),
        ] {
            s.set_setting("human", key, value).unwrap();
        }
        crate::test_helpers_http::additions_record(
            &s,
            "EVO-evaluators-one",
            "one",
            "evolution",
            "evaluators",
            &format!("Private evaluator location: {}", private.path().display()),
        );
        s.conn().unwrap().execute("INSERT INTO evolution_archive(id,project,baseline_score,variant_score,tolerance,kept,fixtures_json,at,note) VALUES('V-001','one',0.6,0.8,0.02,1,'[\"private-canary.request\"]',?,?)", rusqlite::params![s.now(), format!("Owner directory: {}", private.path().display())]).unwrap();
        let data = crate::test_helpers_http::handler(&s)
            .evolution("one".to_string())
            .unwrap();
        assert_evolution_shape(&data);
        let encoded = pulse_store::gojson::to_string(&data).unwrap();
        assert!(!encoded.contains(&private.path().display().to_string()));
        assert!(encoded.contains("[owner-private]"));
    }

    #[test]
    fn evolution_heldout_presence_without_reading_fixtures() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_http::additions_store(false);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-a-directory");
        std::fs::write(&file, "").unwrap();
        s.set_setting("human", "evolve.heldout_dir", file.to_str().unwrap())
            .unwrap();
        let data = crate::test_helpers_http::handler(&s)
            .evolution("one".to_string())
            .unwrap();
        assert_eq!(data["loop"]["heldout_present"], false);
    }

    #[test]
    fn evolution_summary_status_truth_archive() {
        let _serial = crate::test_helpers::serial();
        for (name, baseline, baselines, variants, rows) in [
            (
                "baseline only, including zero score",
                "B-002",
                "2",
                "0",
                vec![
                    ("B-001", "baseline", "", 0.7, true),
                    ("B-002", "baseline", "", 0.0, true),
                ],
            ),
            (
                "kept and discarded variants",
                "B-002",
                "1",
                "3",
                vec![
                    ("B-001", "baseline", "", 0.7, true),
                    ("B-002", "a measured change", "", 0.8, true),
                    ("B-003", "a discarded change", "", 0.6, false),
                    ("B-004", "baseline", "P-an-actual-suggestion", 0.9, false),
                ],
            ),
        ] {
            let s = crate::test_helpers_http::additions_store(false);
            crate::test_helpers_http::additions_project(&s, "one");
            for slug in ["constitution", "evaluators"] {
                // Product scope is open when the line is absent, so the
                // operating-layer-only flags below need an explicit off.
                let body = if slug == "evaluators" {
                    "Generic starter safety configuration.\nProduct scope: off\n"
                } else {
                    "Generic starter safety configuration.\n"
                };
                s.put_record(
                    pulse_store::Record {
                        id: format!("EVO-{slug}-one"),
                        project: "one".into(),
                        kind: "evolution".into(),
                        slug: slug.into(),
                        ..pulse_store::Record::default()
                    },
                    json!({"seeded_from":"test-only/template"})
                        .as_object()
                        .unwrap()
                        .clone()
                        .into_iter()
                        .collect(),
                    body,
                    "human",
                    "test fixture",
                )
                .unwrap();
            }
            for i in 0..6 {
                s.conn().unwrap().execute(
                    "INSERT INTO evolution_fixtures(id, project, name, kind, request, scorer_cmd, weight, enabled, created_by, at)
                     VALUES(?, 'one', ?, 'public', 'TEST_REQUEST_NOT_EXECUTED', 'TEST_SCORER_NOT_EXECUTED', 1, 1, 'human', ?)",
                    rusqlite::params![format!("F-{i}"), format!("test-safety-{i}"), s.now()],
                ).unwrap();
            }
            for (id, hypothesis, suggestion, score, kept) in rows {
                s.conn().unwrap().execute(
                    "INSERT INTO evolution_archive(id, project, hypothesis, suggestion_post, baseline_score, variant_score, tolerance, kept, at)
                     VALUES(?, 'one', ?, ?, 0.7, ?, 0.02, ?, ?)",
                    rusqlite::params![id, hypothesis, suggestion, score, kept, s.now()],
                ).unwrap();
            }
            s.conn().unwrap().execute(
                "INSERT INTO evolution_archive(id, project, hypothesis, baseline_score, variant_score, tolerance, kept, at)
                 VALUES('B-999', 'another-project', 'baseline', 1, 1, 0.02, 1, ?)",
                rusqlite::params![s.now()],
            ).unwrap();
            let data = crate::test_helpers_http::handler(&s)
                .evolution("one".into())
                .unwrap();
            let gates = data["gates"]
                .as_object()
                .expect("canonical gates unavailable");
            assert_eq!(gates["baseline_id"], baseline, "{name}");
            assert_eq!(gates["public_fixtures"], 6.0, "{name}");
            assert_eq!(data["evaluators"]["seeded"], true, "{name}");
            let before = pulse_store::gojson::to_string(&data).unwrap();
            let summary = evolution_summary(data.clone());
            assert_evolution_summary_status_truth_shape(&summary);
            for (key, want) in [
                ("baseline", baseline),
                ("count_baselines", baselines),
                ("count_variants", variants),
                ("product_classes_ok", "0"),
                ("product_evaluators_ok", "0"),
            ] {
                assert_eq!(summary[key], want, "{name}: {key}");
            }
            assert_eq!(
                summary["product_verdict"], gates["product_scope"]["reason"],
                "{name}: product_verdict"
            );
            let after = pulse_store::gojson::to_string(&data).unwrap();
            assert_eq!(
                before, after,
                "summary mutated canonical gates, records or archive scores"
            );
        }
    }

    #[test]
    fn evolution_summary_status_truth_product_flags() {
        let _serial = crate::test_helpers::serial();
        for (name, constitution, evaluators, allowed, classes, eval_ok) in [
            ("level D operating layer only", true, true, false, "0", "0"),
            ("explicit product scope", true, true, true, "1", "1"),
            ("missing constitution", false, true, true, "0", "1"),
            ("empty evaluators", true, false, false, "0", "0"),
        ] {
            let scope = json!({"allowed": allowed, "reason": "canonical product scope reason"});
            let gates = json!({"constitution": constitution, "evaluators": evaluators, "product_scope": scope, "baseline": 0.75, "baseline_id": "B-042", "level": "D", "limit_problem": "canonical limit problem"});
            let data = json!({
                "constitution": {"body":"record presence is not a permission"},
                "evaluators": {"body":"Product scope: allowed"},
                "archive": {"rows":[]},
                "loop": {"loop":"1","stop":"0","limits":"none"},
                "gates": gates
            });
            let summary =
                evolution_summary(data.as_object().unwrap().clone().into_iter().collect());
            assert_evolution_summary_status_truth_shape(&summary);
            for (key, want) in [
                ("baseline", "B-042"),
                ("product_classes_ok", classes),
                ("product_evaluators_ok", eval_ok),
                ("product_verdict", "canonical product scope reason"),
                ("limits_note", "canonical limit problem"),
            ] {
                assert_eq!(summary[key], want, "{name}: {key}");
            }
            let mut missing = Vec::new();
            if !constitution {
                missing.push(Value::String("EVO-constitution".into()));
            }
            if !evaluators {
                missing.push(Value::String("EVO-evaluators".into()));
            }
            assert_eq!(
                summary["missing_files"],
                Value::Array(missing),
                "{name}: missing_files"
            );
        }
    }

    #[test]
    fn evolution_summary_status_truth_unavailable() {
        let _serial = crate::test_helpers::serial();
        for kind in [
            "unregistered project",
            "missing archive table",
            "missing fixtures table",
            "missing gates",
            "missing product scope",
        ] {
            let s = crate::test_helpers_http::additions_store(false);
            crate::test_helpers_http::additions_record(
                &s,
                "EVO-evaluators-one",
                "one",
                "evolution",
                "evaluators",
                "Product scope: allowed\n",
            );
            if kind != "unregistered project" {
                crate::test_helpers_http::additions_project(&s, "one");
            }
            if kind == "missing archive table" {
                s.conn()
                    .unwrap()
                    .execute("DROP TABLE evolution_archive", [])
                    .unwrap();
            }
            if kind == "missing fixtures table" {
                s.conn()
                    .unwrap()
                    .execute("DROP TABLE evolution_fixtures", [])
                    .unwrap();
            }
            let mut data = crate::test_helpers_http::handler(&s)
                .evolution("one".into())
                .unwrap();
            if kind == "missing gates" {
                data.insert("gates".into(), Value::Null);
                data.remove("gates_error");
            } else if kind == "missing product scope" {
                data.get_mut("gates")
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .remove("product_scope");
            } else {
                assert!(data["gates"].is_null(), "{kind}: gates");
                assert_ne!(
                    data.get("gates_error").map(str_value).unwrap_or_default(),
                    "",
                    "{kind}: gates_error"
                );
            }
            let summary = evolution_summary(data.clone());
            assert_evolution_summary_status_truth_shape(&summary);
            assert_eq!(summary["baseline"], "", "{kind}");
            assert_eq!(summary["count_baselines"], "0", "{kind}");
            assert_eq!(summary["count_variants"], "0", "{kind}");
            assert_eq!(summary["product_classes_ok"], "0", "{kind}");
            assert_eq!(summary["product_evaluators_ok"], "0", "{kind}");
            let note = summary["product_verdict"].as_str().unwrap();
            assert!(note.contains("unavailable"), "{kind}: {summary:?}");
            let gates_error = data.get("gates_error").map(str_value).unwrap_or_default();
            assert!(
                gates_error.is_empty() || note.contains(&gates_error),
                "{kind}: {summary:?}"
            );
        }
    }

    #[test]
    fn evolution_summary_status_truth_empty_and_query_error() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_http::additions_store(false);
        crate::test_helpers_http::additions_project(&s, "one");
        let h = crate::test_helpers_http::handler(&s);
        let data = h.evolution("one".to_string()).unwrap();
        let summary = evolution_summary(data);
        assert_evolution_summary_status_truth_shape(&summary);
        assert_eq!(summary["evolution_present"], "0");
        assert_eq!(summary["baseline"], "");
        assert_eq!(summary["count_baselines"], "0");
        assert_eq!(summary["count_variants"], "0");
        assert_eq!(summary["product_evaluators_ok"], "0");
        s.conn()
            .unwrap()
            .execute(
                "ALTER TABLE evolution_archive RENAME COLUMN variant_score TO absent_score",
                [],
            )
            .unwrap();
        assert!(h.evolution("one".to_string()).is_err());
    }

    #[test]
    fn evolution_gates_fail_softly() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_http::additions_store(false);
        let (gates, err) =
            crate::test_helpers_http::handler(&s).evolution_gates("missing".to_string());
        assert!(gates.is_empty());
        assert!(!err.is_empty());
    }

    #[test]
    fn evolution_gates_and_fixtures_never_expose_texts() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_http::additions_store(false);
        crate::test_helpers_http::additions_project(&s, "one");
        let fixtures = crate::test_helpers_http::handler(&s)
            .evolution_fixtures("one".to_string())
            .unwrap();
        let encoded = pulse_store::gojson::to_string(&fixtures).unwrap();
        assert!(!encoded.contains("TEST_REQUEST_NOT_EXECUTED"));
        assert!(!encoded.contains("TEST_SCORER_NOT_EXECUTED"));
    }

    #[test]
    fn project_rows_and_evolution_tell_seeded_records_apart() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_http::additions_store(false);
        let rec = crate::test_helpers_http::additions_record(
            &s,
            "EVO-evaluators-one",
            "one",
            "evolution",
            "evaluators",
            "Product scope: allowed\n",
        );
        let (_head, rev) = s.get_record(&rec.id, 0).unwrap();
        let (seeded, from) = seeded_record(&rec, &rev.unwrap());
        assert!(!seeded);
        assert_eq!(from, "");
    }
}
