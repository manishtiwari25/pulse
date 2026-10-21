use crate::lock::lock_writes;
use crate::records::{input_error, text_value};
use anyhow::Result;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default)]
pub struct DraftInput {
    pub body: String,
    pub author: String,
    pub note: String,
    pub job_id: String,
}

pub fn draft(
    s: &pulse_store::Store,
    id: &str,
    mut input: DraftInput,
) -> Result<Option<pulse_store::RecordDraft>> {
    if input.author.is_empty() {
        input.author = pulse_store::HUMAN_AGENT.into();
    }
    if input.body.trim().is_empty() {
        return Err(input_error(
            "Empty draft",
            "the body has no content",
            &format!("pulse record draft {id} --body-file F with the complete text"),
        ));
    }
    let (rec, rev) = crate::records::get(s, id, 0)?;
    if input.body.trim() == rev.body.trim() {
        return Err(input_error(
            "Draft changes nothing",
            "the body is the record's current text",
            "write the text the record should have and file that",
        ));
    }
    if rec.kind == "evolution"
        && matches!(rec.slug.as_str(), "constitution" | "evaluators")
        && !check_loop_draft(&rec.slug, &rev.body, &input.body).is_empty()
    {
        let problem = check_loop_draft(&rec.slug, &rev.body, &input.body);
        return Err(input_error(
            &format!("Draft refused for {id}"),
            &problem,
            &format!("fix the text and file it again with pulse record draft {id} --body-file F"),
        ));
    }
    let _unlock = lock_writes(s)?;
    s.put_draft(
        id,
        &input.body,
        &input.author,
        input.note.trim(),
        &input.job_id,
    )
}

pub fn adopt(
    s: &pulse_store::Store,
    id: &str,
    actor: &str,
    words: &str,
) -> Result<pulse_store::Record> {
    if !actor.is_empty() && actor != pulse_store::HUMAN_AGENT {
        return Err(input_error(
            "Adoption is human only",
            &format!("agents may not adopt a draft into {id}"),
            &format!(
                "leave it for the owner: the hub shows it under the record, or pulse record adopt {id}"
            ),
        ));
    }
    let _unlock = lock_writes(s)?;
    let (rec, rev) = crate::records::get(s, id, 0)?;
    let Some(draft) = s.get_draft(id)? else {
        return Err(input_error(
            "No draft to adopt",
            &format!("{id} has no draft filed"),
            "pulse record drafts",
        ));
    };
    let mut note = format!("adopted the draft by {}", draft.author);
    if draft.base_rev != rec.current_rev {
        note.push_str(&format!(" (written against r{})", draft.base_rev));
    }
    let words = words.trim();
    if !words.is_empty() {
        note.push_str(": ");
        note.push_str(words);
    }
    s.put_record(
        rec.clone(),
        rev.frontmatter,
        &draft.body,
        pulse_store::HUMAN_AGENT,
        &note,
    )?;
    s.delete_draft(
        id,
        pulse_store::HUMAN_AGENT,
        "record.adopted",
        serde_json::json!({"author": draft.author, "job": draft.job_id, "base_rev": draft.base_rev}),
    )?;
    Ok(s.get_record(id, 0)?.0)
}

pub fn discard(s: &pulse_store::Store, id: &str, actor: &str, words: &str) -> Result<bool> {
    let actor = if actor.is_empty() {
        pulse_store::HUMAN_AGENT
    } else {
        actor
    };
    crate::records::get(s, id, 0)?;
    let _unlock = lock_writes(s)?;
    s.delete_draft(
        id,
        actor,
        "record.draft.discarded",
        serde_json::json!({"words": words.trim()}),
    )
}

pub fn drafts(s: &pulse_store::Store, project: &str) -> Result<Vec<pulse_store::RecordDraft>> {
    s.list_drafts(project)
}

const DEFAULT_PROTECTED: &str = "Hard stops, Kill switch, Amendments, frontmatter";

pub fn check_loop_draft(slug: &str, current: &str, draft: &str) -> String {
    let mut left = placeholders(draft);
    if !left.is_empty() {
        if left.len() > 3 {
            let more = left.len() - 3;
            left.truncate(3);
            left.push(format!("and {more} more"));
        }
        return format!(
            "a placeholder is still in the text: {} - replace it with the project's fact",
            left.join(", ")
        );
    }
    if slug != "constitution" {
        return String::new();
    }
    let have = sections_of(draft);
    let was = sections_of(current);
    for name in section_order(current) {
        if !have.contains_key(&name) {
            return format!(
                "the \"## {}\" section is missing - a constitution keeps every section of the current text",
                section_title(current, &name)
            );
        }
    }
    let protected =
        table_cell_of(current, "Not amendable by vote").unwrap_or_else(|| DEFAULT_PROTECTED.into());
    for item in protected.replace('`', "").split(',') {
        let name = item.trim().to_ascii_lowercase();
        if name.is_empty() || name == "frontmatter" {
            continue;
        }
        let Some(before) = was.get(&name) else {
            continue;
        };
        if have.get(&name) != Some(before) {
            return format!(
                "the \"{}\" section differs from the current text; it is protected (Not amendable by vote) and changes only by a human's own edit - keep it verbatim",
                item.trim()
            );
        }
    }
    String::new()
}

fn sections_of(body: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut name = String::new();
    let mut lines: Vec<String> = Vec::new();
    let flush = |name: &str, lines: &[String], out: &mut BTreeMap<String, String>| {
        if !name.is_empty() {
            out.insert(name.to_string(), lines.join("\n").trim().to_string());
        }
    };
    for line in body.split('\n') {
        if let Some(rest) = line.strip_prefix("## ") {
            flush(&name, &lines, &mut out);
            name = rest.trim().to_ascii_lowercase();
            lines.clear();
        } else if line.starts_with("# ") {
            flush(&name, &lines, &mut out);
            name.clear();
            lines.clear();
        } else {
            lines.push(line.trim_end_matches([' ', '\t']).to_string());
        }
    }
    flush(&name, &lines, &mut out);
    out
}

fn section_order(body: &str) -> Vec<String> {
    body.split('\n')
        .filter_map(|line| {
            line.strip_prefix("## ")
                .map(|s| s.trim().to_ascii_lowercase())
        })
        .collect()
}

fn section_title(body: &str, name: &str) -> String {
    for line in body.split('\n') {
        if let Some(rest) = line.strip_prefix("## ")
            && rest.trim().eq_ignore_ascii_case(name)
        {
            return rest.trim().into();
        }
    }
    name.into()
}

fn table_cell_of(text: &str, name: &str) -> Option<String> {
    for line in text.split('\n') {
        let cells: Vec<_> = line.trim_matches([' ', '\t', '|']).split('|').collect();
        if cells.len() > 1
            && cells[0]
                .trim()
                .trim_matches(['*', '`', ' '])
                .eq_ignore_ascii_case(name)
        {
            return Some(cells[1].trim().into());
        }
    }
    None
}

pub fn placeholders(body: &str) -> Vec<String> {
    let code_re = Regex::new(r"`[^`\n]*`").unwrap();
    let placeholder_re = Regex::new(r"<[A-Za-z][^<>\n]*>").unwrap();
    let masked = code_re.replace_all(body, |caps: &regex::Captures<'_>| " ".repeat(caps[0].len()));
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for m in placeholder_re.find_iter(&masked) {
        let p = &body[m.start()..m.end()];
        if seen.insert(p.to_string()) {
            out.push(p.to_string());
        }
    }
    out
}

pub fn seeded(
    head: Option<&pulse_store::Record>,
    revision: Option<&pulse_store::Revision>,
) -> (bool, String) {
    let (Some(head), Some(revision)) = (head, revision) else {
        return (false, String::new());
    };
    let from = text_value(revision.frontmatter.get("seeded_from"));
    (head.current_rev == 1 && !from.is_empty(), from)
}

pub use adopt as Adopt;
pub use discard as Discard;
pub use draft as Draft;
pub use drafts as Drafts;
pub use placeholders as Placeholders;
pub use seeded as Seeded;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DRAFT_TEMPLATE: &str = "# <Project> Evolution Constitution\n\nRules the loop can never change.\n\n## Hard stops\n\n- Never push to `main` directly.\n- Never touch production, deploy, billing.\n\n## Immutable paths\n\n| Path | Why |\n| --- | --- |\n| <product hard-stop surfaces only> | Tier 2 surfaces |\n\n## Editable scope by stage\n\n| Stage | Paths |\n| --- | --- |\n| Product | <product paths, e.g. `src/`> |\n\n## Budget\n\n| Setting | Value |\n| --- | --- |\n| Runs per fixture | <N> |\n\n## Vote rules\n\n| Rule | Value |\n| --- | --- |\n| Suggestion expiry with no votes | <N> days |\n| Not amendable by vote | Hard stops, Kill switch, Amendments, frontmatter |\n\n## Amendments\n\nA human edits this record; `git diff <parent>..<variant>` shows the change.\n\n## Kill switch\n\n`loop: off` in the frontmatter stops everything.\n";
    const TEMPLATE_EVALUATORS: &str =
        "# <Project> Evaluators\n\nWhat better means for <Project>.\n";

    fn complete_draft(project: &str) -> String {
        DRAFT_TEMPLATE
            .replace("<Project>", project)
            .replace(
                "<product hard-stop surfaces only>",
                "`deploy/`, `migrations/`",
            )
            .replace("<product paths, e.g. `src/`>", "`cmd/`, `internal/`")
            .replace("| <N> |", "| 3 |")
            .replace("<N> days", "14 days")
    }

    struct TestStore {
        _home: tempfile::TempDir,
        store: pulse_store::Store,
    }
    impl std::ops::Deref for TestStore {
        type Target = pulse_store::Store;
        fn deref(&self) -> &Self::Target {
            &self.store
        }
    }
    fn temp_store() -> TestStore {
        let home = tempfile::tempdir().unwrap();
        pulse_testutil::neutralize_runner_environment();
        let store = pulse_store::Store::open(pulse_store::Options {
            home: home.path().display().to_string(),
            create: true,
            now: "2026-09-06T21:00:00Z".into(),
            ..pulse_store::Options::default()
        })
        .unwrap();
        TestStore { _home: home, store }
    }

    fn add_project(s: &pulse_store::Store, slug: &str, brain: bool) {
        let dir = std::path::Path::new(&s.home).join(format!("project-{slug}"));
        std::fs::create_dir_all(&dir).unwrap();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug,name,path,brain,added_at) VALUES (?,?,?,?,?)",
                rusqlite::params![slug, slug, dir.display().to_string(), brain, s.now()],
            )
            .unwrap();
    }

    fn seed_draft_project(s: &pulse_store::Store, slug: &str) -> String {
        add_project(s, "brain", true);
        let rec = pulse_store::Record {
            id: "TPL-evolution-constitution".into(),
            project: "brain".into(),
            kind: "template".into(),
            slug: "evolution-constitution".into(),
            title: "<Project> Evolution Constitution".into(),
            status: "Active".into(),
            path: "docs/evolution/_template-constitution.md".into(),
            origin: "markdown:x@0000000".into(),
            ..pulse_store::Record::default()
        };
        let front = [
            ("id".into(), json!("CONSTITUTION")),
            ("title".into(), json!("<Project> Evolution Constitution")),
            ("status".into(), json!("Active")),
            ("date".into(), json!("YYYY-MM-DD")),
            ("loop".into(), json!("off")),
        ]
        .into();
        s.put_record(rec, front, DRAFT_TEMPLATE, "tester", "import")
            .unwrap();
        let evaluators = pulse_store::Record {
            id: "TPL-evolution-evaluators".into(),
            project: "brain".into(),
            kind: "template".into(),
            slug: "evolution-evaluators".into(),
            title: "<Project> Evaluators".into(),
            status: "Active".into(),
            path: "docs/evolution/_template-evaluators.md".into(),
            origin: "markdown:y@0000000".into(),
            ..pulse_store::Record::default()
        };
        s.put_record(
            evaluators,
            [
                ("id".into(), json!("EVALUATORS")),
                ("title".into(), json!("<Project> Evaluators")),
                ("status".into(), json!("Active")),
                ("date".into(), json!("YYYY-MM-DD")),
                ("baseline".into(), json!("pending")),
            ]
            .into(),
            TEMPLATE_EVALUATORS,
            "tester",
            "import",
        )
        .unwrap();
        add_project(s, slug, false);
        s.conn()
            .unwrap()
            .execute(
                "UPDATE projects SET name = ? WHERE slug = ?",
                rusqlite::params![format!("{}{}", slug[..1].to_uppercase(), &slug[1..]), slug],
            )
            .unwrap();
        crate::seed::seed_loop_records(s, slug, "alice").unwrap();
        format!("EVO-constitution-{slug}")
    }

    fn event_count(s: &pulse_store::Store, action: &str) -> i64 {
        s.conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM events WHERE action = ?",
                [action],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn revision_count(s: &pulse_store::Store, id: &str) -> usize {
        s.revisions(id).unwrap().len()
    }

    #[test]
    fn draft_checks_the_loop_record_before_filing_it() {
        let _serial = crate::test_helpers::serial();
        let s = temp_store();
        let id = seed_draft_project(&s, "shop");
        let (_, current) = crate::records::get(&s, &id, 0).unwrap();
        let (head, _) = s.get_record(&id, 0).unwrap();
        let (is_seeded, from) = seeded(Some(&head), Some(&current));
        assert!(
            is_seeded && from == "brain/TPL-evolution-constitution@r1",
            "seeded = {is_seeded:?} {from:?}"
        );
        let got = placeholders(&current.body);
        assert_eq!(
            got.join("|"),
            "<product hard-stop surfaces only>|<product paths, e.g. `src/`>|<N>",
            "placeholders: {got:?}"
        );
        let complete = complete_draft("Shop");
        let refusals = [
            ("empty", "   \n".to_string()),
            ("unchanged", current.body.clone()),
            ("placeholder", complete.replacen("| 3 |", "| <N> |", 1)),
            (
                "section",
                complete.replacen("## Vote rules", "## Voting", 1),
            ),
            (
                "protected",
                complete.replacen(
                    "Never push to `main` directly.",
                    "Push to `main` when tests pass.",
                    1,
                ),
            ),
            (
                "killswitch",
                complete.replacen("stops everything.", "stops most things.", 1),
            ),
        ];
        let reasons = [
            ("empty", "no content"),
            ("unchanged", "current text"),
            ("placeholder", "<N>"),
            ("section", "\"## Vote rules\" section is missing"),
            ("protected", "\"Hard stops\" section differs"),
            ("killswitch", "\"Kill switch\" section differs"),
        ]
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>();
        for (name, body) in refusals {
            let err = draft(
                &s,
                &id,
                DraftInput {
                    body,
                    author: "bot".into(),
                    ..DraftInput::default()
                },
            )
            .err();
            let want = reasons[name];
            assert!(
                err.as_ref()
                    .is_some_and(|err| err.to_string().contains(want)),
                "{name}: err = {err:?}, want {want:?}"
            );
        }
        assert!(
            s.get_draft(&id).unwrap().is_none() && event_count(&s, "record.draft") == 0,
            "a refused draft was stored"
        );
        let _guard = pulse_testutil::setenv("PULSE_JOB", "job-1");
        let filed = draft(
            &s,
            &id,
            DraftInput {
                body: complete.clone(),
                author: "bot".into(),
                note: "  from the README and Makefile ".into(),
                job_id: "job-1".into(),
            },
        )
        .unwrap()
        .unwrap();
        assert!(
            filed.record_id == id
                && filed.base_rev == 1
                && filed.author == "bot"
                && filed.note == "from the README and Makefile"
                && filed.job_id == "job-1"
                && filed.body == complete
                && filed.project == "shop"
                && filed.kind == "evolution"
                && filed.title == "Shop Evolution Constitution"
                && !filed.at.is_empty(),
            "draft row: {filed:?}"
        );
        let rows = drafts(&s, "shop").unwrap();
        assert_eq!(rows.len(), 1);
        let again = complete.replacen("| 3 |", "| 5 |", 1);
        draft(
            &s,
            &id,
            DraftInput {
                body: again.clone(),
                author: "bot2".into(),
                ..DraftInput::default()
            },
        )
        .unwrap();
        let rows = drafts(&s, "shop").unwrap();
        assert!(
            rows.len() == 1
                && rows[0].body == again
                && rows[0].author == "bot2"
                && rows[0].note.is_empty()
                && rows[0].job_id.is_empty(),
            "drafts after refiling: {rows:?}"
        );
        assert!(
            drafts(&s, "elsewhere").unwrap().is_empty(),
            "project filter ignored"
        );
        assert!(
            event_count(&s, "record.draft") == 2 && revision_count(&s, &id) == 1,
            "filing a draft must record an event and never revise the record"
        );
        let plan = pulse_store::Record {
            id: "PLAN-001".into(),
            project: "shop".into(),
            kind: "plan".into(),
            slug: "x".into(),
            title: "Plan".into(),
            status: "Active".into(),
            ..pulse_store::Record::default()
        };
        s.put_record(
            plan,
            [("id".into(), json!("PLAN-001"))].into(),
            "# Plan\n\n<N> steps.\n",
            "alice",
            "new",
        )
        .unwrap();
        draft(
            &s,
            "PLAN-001",
            DraftInput {
                body: "# Plan\n\n<M> steps.\n".into(),
                ..DraftInput::default()
            },
        )
        .unwrap();
        assert_eq!(
            s.get_draft("PLAN-001").unwrap().unwrap().author,
            pulse_store::HUMAN_AGENT
        );
    }

    #[test]
    fn adopt_is_the_humans_and_writes_the_next_revision() {
        let _serial = crate::test_helpers::serial();
        let s = temp_store();
        let id = seed_draft_project(&s, "shop");
        let complete = complete_draft("Shop");
        assert!(
            adopt(&s, &id, "", "")
                .unwrap_err()
                .to_string()
                .contains("no draft filed")
        );
        draft(
            &s,
            &id,
            DraftInput {
                body: complete.clone(),
                author: "bot".into(),
                job_id: "job-9".into(),
                ..DraftInput::default()
            },
        )
        .unwrap();
        assert!(
            adopt(&s, &id, "bot", "looks right")
                .unwrap_err()
                .to_string()
                .contains("human only")
        );
        assert_eq!(
            revision_count(&s, &id),
            1,
            "a refused adoption revised the record"
        );
        let rec = adopt(&s, &id, "", "  checked the paths  ").unwrap();
        assert_eq!(rec.current_rev, 2);
        let (head, rev) = crate::records::get(&s, &id, 0).unwrap();
        assert_eq!(rev.body, complete);
        assert_eq!(rev.author, pulse_store::HUMAN_AGENT);
        assert_eq!(rev.note, "adopted the draft by bot: checked the paths");
        assert_eq!(
            rev.frontmatter["seeded_from"],
            json!("brain/TPL-evolution-constitution@r1")
        );
        assert_eq!(rev.frontmatter["loop"], json!("off"));
        assert_eq!(
            rev.frontmatter["title"],
            json!("Shop Evolution Constitution")
        );
        assert!(!seeded(Some(&head), Some(&rev)).0);
        assert!(s.get_draft(&id).unwrap().is_none());
        assert!(
            event_count(&s, "record.adopted") == 1 && event_count(&s, "record.revised") == 1,
            "events: adopted {} revised {}",
            event_count(&s, "record.adopted"),
            event_count(&s, "record.revised")
        );
        draft(
            &s,
            &id,
            DraftInput {
                body: complete.replacen("| 3 |", "| 5 |", 1),
                author: "bot".into(),
                ..DraftInput::default()
            },
        )
        .unwrap();
        let (_, rev) = crate::records::get(&s, &id, 0).unwrap();
        s.put_record(
            head,
            rev.frontmatter,
            complete.replacen("14 days", "21 days", 1).as_str(),
            pulse_store::HUMAN_AGENT,
            "owner edit",
        )
        .unwrap();
        let rec = adopt(&s, &id, pulse_store::HUMAN_AGENT, "").unwrap();
        let (_, rev) = crate::records::get(&s, &id, 0).unwrap();
        assert_eq!(rec.current_rev, 4);
        assert_eq!(rev.note, "adopted the draft by bot (written against r2)");
    }

    #[test]
    fn discard_drops_the_draft_once() {
        let _serial = crate::test_helpers::serial();
        let s = temp_store();
        let id = seed_draft_project(&s, "shop");
        let gone = discard(&s, &id, "", "").unwrap();
        assert!(!gone, "discard with nothing filed: {gone:?}");
        assert!(
            discard(&s, "EVO-constitution-nope", "", "").is_err(),
            "discard on a missing record must fail"
        );
        draft(
            &s,
            &id,
            DraftInput {
                body: complete_draft("Shop"),
                author: "bot".into(),
                ..DraftInput::default()
            },
        )
        .unwrap();
        let gone = discard(&s, &id, "bot", "wrong paths").unwrap();
        assert!(gone, "discard: {gone:?}");
        assert!(
            s.get_draft(&id).unwrap().is_none()
                && event_count(&s, "record.draft.discarded") == 1
                && revision_count(&s, &id) == 1,
            "discard must clear the draft, record one event and leave the record alone"
        );
        let gone = discard(&s, &id, "", "").unwrap();
        assert!(
            !gone && event_count(&s, "record.draft.discarded") == 1,
            "a second discard has nothing to do"
        );
    }
}
