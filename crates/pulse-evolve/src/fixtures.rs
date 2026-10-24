use anyhow::{Result, anyhow};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

/// Fixture is a public evaluator.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Fixture {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "project")]
    pub project: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(skip)]
    pub request: String,
    #[serde(skip)]
    pub scorer: String,
    #[serde(rename = "weight")]
    pub weight: f64,
    #[serde(rename = "kind")]
    pub kind: String,
    #[serde(rename = "metric")]
    pub metric: bool,
    #[serde(rename = "enabled")]
    pub enabled: bool,
    #[serde(rename = "created_by")]
    pub created_by: String,
    #[serde(rename = "at")]
    pub at: String,
    #[serde(rename = "answer_keys")]
    pub answer_keys: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct FixtureInput {
    pub project: String,
    pub name: String,
    pub request: String,
    pub scorer: String,
    pub kind: String,
    pub metric: bool,
    pub actor: String,
    pub weight: f64,
    pub answer_keys: Vec<String>,
}

pub(crate) static FIXTURE_NAME: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,79}$").unwrap());

pub(crate) fn finite(v: f64) -> bool {
    !v.is_nan() && !v.is_infinite()
}

pub(crate) fn score_value(v: f64) -> bool {
    finite(v) && (0.0..=1.0).contains(&v)
}

pub(crate) fn input_error(what: &str, why: &str, fix: &str) -> anyhow::Error {
    anyhow!(pulse_cli::errorf(2, what, why, fix))
}

pub(crate) fn valid_project(s: &pulse_store::Store, slug: &str) -> Result<pulse_store::Project> {
    let p = s.project(slug)?;
    match p {
        Some(p) if p.slug != pulse_store::COMMONS_SLUG => Ok(p),
        _ => Err(input_error(
            "Evolution needs a registered project",
            "the commons has no experiment checkout",
            "pulse projects",
        )),
    }
}

pub(crate) fn relative_path(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty()
        || value.contains(['\0', '\r', '\n', '\\', '*', '?', '[', ']'])
        || Path::new(value).is_absolute()
    {
        return Err(anyhow!("use one literal repository-relative path"));
    }
    let clean = path_clean(value);
    if clean == "."
        || clean == ".."
        || clean.starts_with("../")
        || clean == ".git"
        || clean.starts_with(".git/")
    {
        return Err(anyhow!(
            "the path must stay inside the checkout, outside .git"
        ));
    }
    for part in value.split('/') {
        if part == ".." {
            return Err(anyhow!("parent traversal is not allowed"));
        }
    }
    Ok(clean)
}

pub fn add_fixture(s: &pulse_store::Store, input: FixtureInput) -> Result<Fixture> {
    let mut input = input;
    valid_project(s, &input.project)?;
    if !FIXTURE_NAME.is_match(&input.name) || input.name == "." || input.name == ".." {
        return Err(input_error(
            "Cannot add fixture",
            "name must be 1-80 letters, digits, dots, dashes or underscores",
            "pulse evolve fixture add --help",
        ));
    }
    if (!input.metric && input.request.trim().is_empty()) || input.scorer.trim().is_empty() {
        return Err(input_error(
            "Cannot add fixture",
            "a request and scorer are required unless --metric is set",
            "pulse evolve fixture add --help",
        ));
    }
    if input.request.len() > crate::private::MAX_FIXTURE_BYTES
        || input.scorer.len() > crate::private::MAX_FIXTURE_BYTES
        || pulse_store::secret_shaped(&input.request)
        || pulse_store::secret_shaped(&input.scorer)
    {
        return Err(input_error(
            "Cannot add fixture",
            "requests and scorers must be bounded, secret-free text",
            "pulse evolve fixture add --request FILE --scorer CMD --project P --name N",
        ));
    }
    if input.request.contains('\0') || input.scorer.contains('\0') {
        return Err(input_error(
            "Cannot add fixture",
            "NUL bytes are not allowed",
            "pulse evolve fixture add --help",
        ));
    }
    if !finite(input.weight) || input.weight <= 0.0 {
        return Err(input_error(
            "Cannot add fixture",
            "weight must be a finite positive number",
            "pulse evolve fixture add --weight 1 --help",
        ));
    }
    if input.kind.is_empty() {
        input.kind = "public".to_string();
    }
    if input.kind != "public" && input.kind != "signal" {
        return Err(input_error(
            "Cannot add fixture",
            "kind must be public or signal; private fixtures are never stored here",
            "pulse evolve setup DIR",
        ));
    }
    let mut keys = Vec::new();
    let mut seen = BTreeMap::new();
    for key in &input.answer_keys {
        let k = relative_path(key).map_err(|_| {
            input_error(
                "Cannot add answer key",
                "answer keys must be secret-free repository-relative paths",
                "pulse evolve fixture add --answer-key PATH --help",
            )
        })?;
        if pulse_store::secret_shaped(key) {
            return Err(input_error(
                "Cannot add answer key",
                "answer keys must be secret-free repository-relative paths",
                "pulse evolve fixture add --answer-key PATH --help",
            ));
        }
        if seen.insert(k.clone(), true).is_none() {
            keys.push(k);
        }
    }
    let tx = s.begin()?;
    let exists: Option<i64> = tx
        .query_row(
            "SELECT 1 FROM evolution_fixtures WHERE project = ? AND name = ?",
            params![input.project, input.name],
            |r| r.get(0),
        )
        .optional()?;
    if exists.is_some() {
        return Err(input_error(
            "Fixture already exists",
            "disable it and add a new name rather than silently changing an evaluator",
            &format!("pulse evolve fixture list --project {}", input.project),
        ));
    }
    if input.actor.is_empty() {
        input.actor = pulse_store::HUMAN_AGENT.to_string();
    }
    let f = Fixture {
        id: pulse_board::new_id(s, "fixture"),
        project: input.project.clone(),
        name: input.name.clone(),
        request: input.request.clone(),
        scorer: input.scorer.clone(),
        weight: input.weight,
        kind: input.kind.clone(),
        metric: input.metric,
        enabled: true,
        created_by: input.actor.clone(),
        at: s.now(),
        answer_keys: keys,
    };
    tx.execute(
        "INSERT INTO evolution_fixtures
		(id, project, name, request, scorer_cmd, weight, kind, metric, enabled, created_by, at)
		VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
        params![
            f.id,
            f.project,
            f.name,
            f.request,
            f.scorer,
            f.weight,
            f.kind,
            if f.metric { 1 } else { 0 },
            f.created_by,
            f.at
        ],
    )?;
    for key in &f.answer_keys {
        tx.execute(
            "INSERT INTO evolution_fixture_keys (fixture_id, path) VALUES (?, ?)",
            params![f.id, key],
        )?;
    }
    tx.event(
        &input.actor,
        "evolution.fixture.added",
        &f.id,
        serde_json::json!({"project": f.project, "name": f.name, "kind": f.kind, "metric": f.metric}),
    )?;
    tx.commit()?;
    Ok(f)
}

pub fn list_fixtures(
    s: &pulse_store::Store,
    project: &str,
    enabled_only: bool,
) -> Result<Vec<Fixture>> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        "SELECT id, project, name, request, scorer_cmd, weight, kind, COALESCE(metric, 0), enabled, created_by, at
		FROM evolution_fixtures WHERE (? = '' OR project = ?) AND (? = 0 OR enabled = 1) ORDER BY project, name",
    )?;
    let mut out: Vec<Fixture> = stmt
        .query_map(params![project, project, enabled_only], |r| {
            Ok(Fixture {
                id: r.get(0)?,
                project: r.get(1)?,
                name: r.get(2)?,
                request: r.get(3)?,
                scorer: r.get(4)?,
                weight: r.get(5)?,
                kind: r.get(6)?,
                metric: r.get::<_, i64>(7)? != 0,
                enabled: r.get::<_, i64>(8)? != 0,
                created_by: r.get(9)?,
                at: r.get(10)?,
                answer_keys: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut by_id = BTreeMap::new();
    for (i, f) in out.iter().enumerate() {
        by_id.insert(f.id.clone(), i);
    }
    let mut stmt = conn.prepare(
        "SELECT k.fixture_id, k.path FROM evolution_fixture_keys k
		JOIN evolution_fixtures f ON f.id = k.fixture_id WHERE (? = '' OR f.project = ?) ORDER BY k.path",
    )?;
    let mut rows = stmt.query(params![project, project])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let key: String = row.get(1)?;
        if let Some(i) = by_id.get(&id) {
            out[*i].answer_keys.push(key);
        }
    }
    Ok(out)
}

pub fn set_fixture_enabled(
    s: &pulse_store::Store,
    project: &str,
    name: &str,
    enabled: bool,
    actor: &str,
) -> Result<()> {
    let tx = s.begin()?;
    let n = tx.execute(
        "UPDATE evolution_fixtures SET enabled = ? WHERE project = ? AND name = ?",
        params![enabled, project, name],
    )?;
    if n != 1 {
        return Err(input_error(
            "No such public fixture",
            "name is not registered in this project",
            &format!("pulse evolve fixture list --project {project}"),
        ));
    }
    tx.event(
        actor,
        "evolution.fixture.enabled",
        name,
        serde_json::json!({"project": project, "enabled": enabled}),
    )?;
    tx.commit()
}

/// Give a derived fixture a new scorer and a fresh date. Only the loop's own
/// `derived-*` fixtures may change in place; a hand-made evaluator is
/// disabled and re-added under a new name, so its history stays readable.
pub fn rearm_fixture(
    s: &pulse_store::Store,
    project: &str,
    name: &str,
    scorer: &str,
    actor: &str,
) -> Result<()> {
    if !name.starts_with(crate::derive::DERIVED_PREFIX) {
        return Err(input_error(
            "Cannot re-arm fixture",
            "only derived fixtures change in place; disable a hand-made one and add a new name",
            &format!("pulse evolve fixture list --project {project}"),
        ));
    }
    if scorer.trim().is_empty()
        || scorer.len() > crate::private::MAX_FIXTURE_BYTES
        || pulse_store::secret_shaped(scorer)
        || scorer.contains('\0')
    {
        return Err(input_error(
            "Cannot re-arm fixture",
            "scorers must be bounded, secret-free text",
            &format!("pulse evolve fixture list --project {project}"),
        ));
    }
    let tx = s.begin()?;
    let previous: Option<String> = tx
        .query_row(
            "SELECT scorer_cmd FROM evolution_fixtures WHERE project = ? AND name = ? AND enabled = 1",
            params![project, name],
            |r| r.get(0),
        )
        .optional()?;
    let Some(previous) = previous else {
        return Err(input_error(
            "No such enabled derived fixture",
            "name is not an enabled fixture in this project",
            &format!("pulse evolve fixture list --project {project}"),
        ));
    };
    tx.execute(
        "UPDATE evolution_fixtures SET scorer_cmd = ?, at = ? WHERE project = ? AND name = ?",
        params![scorer, s.now(), project, name],
    )?;
    tx.event(
        actor,
        "evolution.fixture.rearmed",
        name,
        serde_json::json!({"project": project, "scorer": scorer, "previous": previous}),
    )?;
    tx.commit()
}

fn path_clean(value: &str) -> String {
    pulse_store::filepath::clean(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::private::{heldout_count, load_private, setup};
    use crate::test_helpers::*;

    #[test]
    fn fixtures_crud_and_secret_refusal() {
        let _serial = crate::test_helpers::serial();
        let s = test_store();
        test_project(&s, "demo");
        let f = fixture(&s, "demo", "F01", "printf '0.5\\n'", 2.0);
        set_fixture_enabled(&s, "demo", &f.name, false, "human").unwrap();
        let enabled = list_fixtures(&s, "demo", true).unwrap();
        assert!(enabled.is_empty(), "disabled fixture remained selected");
        set_fixture_enabled(&s, "demo", &f.name, true, "human").unwrap();
        let enabled = list_fixtures(&s, "demo", true).unwrap();
        assert_eq!(enabled.len(), 1, "enabled fixture missing");
        assert_eq!(enabled[0].answer_keys.len(), 1, "answer keys not retained");
        assert!(!enabled[0].metric);

        let base = FixtureInput {
            project: "demo".into(),
            name: "bad".into(),
            request: "safe request".into(),
            scorer: "printf 1".into(),
            weight: 1.0,
            ..Default::default()
        };
        let mut cases = Vec::new();
        let mut x = base.clone();
        x.request = format!("ghp_{}", "a".repeat(40));
        cases.push(x);
        let mut x = base.clone();
        x.scorer = format!("printf ghp_{}", "b".repeat(40));
        cases.push(x);
        let mut x = base.clone();
        x.weight = 0.0;
        cases.push(x);
        let mut x = base.clone();
        x.kind = "heldout".into();
        cases.push(x);
        let mut x = base;
        x.answer_keys = vec!["../outside".into()];
        cases.push(x);
        for (i, input) in cases.into_iter().enumerate() {
            assert!(
                add_fixture(&s, input).is_err(),
                "invalid public fixture case {i} accepted"
            );
        }
        assert_eq!(count_rows(&s, "evolution_fixtures"), 1);
        let metric = add_fixture(
            &s,
            FixtureInput {
                project: "demo".into(),
                name: "metric-headroom".into(),
                request: String::new(),
                scorer: "printf 0.5".into(),
                metric: true,
                weight: 1.0,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(metric.request.is_empty());
        let listed = list_fixtures(&s, "demo", true).unwrap();
        assert!(
            listed
                .iter()
                .any(|f| f.name == "metric-headroom" && f.metric)
        );
        assert!(
            add_fixture(
                &s,
                FixtureInput {
                    project: "demo".into(),
                    name: "empty-non-metric".into(),
                    request: String::new(),
                    scorer: "printf 0.5".into(),
                    weight: 1.0,
                    ..Default::default()
                },
            )
            .is_err(),
            "non-metric fixture accepted an empty request"
        );
        let conn = s.conn().unwrap();
        assert!(
            conn.execute("DELETE FROM evolution_fixtures WHERE id = ?", [&f.id])
                .is_err(),
            "fixture deletion bypassed disable-only history"
        );
    }

    #[test]
    fn private_setup_counts_only_non_overwriting() {
        let _serial = crate::test_helpers::serial();
        let s = test_store();
        let repo = test_project(&s, "demo");
        let dir = tempfile::tempdir().unwrap();
        let owner = dir.path().join("owner");
        let first = setup(&s, owner.to_str().unwrap()).unwrap();
        assert_eq!((first.heldout_state.pending, first.created), (2, 4));
        let request = owner.join("H01.request");
        put_file(&request, "owner-controlled request", 0o600);
        let second = setup(&s, owner.to_str().unwrap()).unwrap();
        assert_eq!(
            second.created, 0,
            "repeated setup overwrote an existing template"
        );
        assert_eq!(
            std::fs::read_to_string(&request).unwrap(),
            "owner-controlled request"
        );
        let encoded = pulse_store::gojson::to_string(&second).unwrap();
        assert_private_absent(
            &encoded,
            &[
                owner.display().to_string(),
                "H01".into(),
                "H02".into(),
                "owner-controlled request".into(),
            ],
        );
        assert_eq!(s.setting("evolve.loop", ""), "0");
        assert_eq!(count_rows(&s, "evolution_archive"), 0);
        assert!(
            setup(
                &s,
                &std::path::Path::new(&repo)
                    .join("private")
                    .display()
                    .to_string()
            )
            .is_err()
        );
        let alias_dir = tempfile::tempdir().unwrap();
        let alias = alias_dir.path().join("alias");
        std::os::unix::fs::symlink(&repo, &alias).unwrap();
        assert!(setup(&s, &alias.join("private").display().to_string()).is_err());
        let other = test_project(&s, "second");
        assert!(
            setup(
                &s,
                &std::path::Path::new(&other)
                    .join("private")
                    .display()
                    .to_string()
            )
            .is_err()
        );
    }

    #[test]
    fn private_validation_and_import_guard() {
        let _serial = crate::test_helpers::serial();
        let s = test_store();
        test_project(&s, "demo");
        let (dir, forbidden) = private_set(&s);
        let state = heldout_count(&s);
        assert_eq!(state.count, 2);
        assert!(state.problem.is_empty());
        let err = crate::commands::read_request(
            &s,
            &std::path::Path::new(&dir)
                .join("owner-private-first.request")
                .display()
                .to_string(),
        )
        .expect_err("private request could be imported into the public table");
        assert_private_absent(&err.to_string(), &forbidden);
        let file = std::path::Path::new(&dir).join("owner-private-first.score");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o744)).unwrap();
        }
        let err = load_private(&s).expect_err("world-readable private scorer accepted");
        assert_private_absent(&err.to_string(), &forbidden);
        let encoded = pulse_store::gojson::to_string(&heldout_count(&s)).unwrap();
        assert_private_absent(&encoded, &forbidden);
    }

    #[test]
    fn private_setup_failure_preserves_the_configured_set() {
        let _serial = crate::test_helpers::serial();
        let s = test_store();
        test_project(&s, "demo");
        let (good, forbidden) = private_set(&s);
        let bad = tempfile::tempdir().unwrap().path().join("unpaired");
        put_file(bad.join("unpaired.request"), "Incomplete fixture", 0o600);
        let err =
            setup(&s, bad.to_str().unwrap()).expect_err("setup accepted an unpaired private set");
        let mut all = forbidden.clone();
        all.extend([bad.display().to_string(), "unpaired".into()]);
        assert_private_absent(&err.to_string(), &all);
        assert_eq!(s.setting("evolve.heldout_dir", ""), good);
        assert_eq!(heldout_count(&s).count, 2);
    }
}
