/// Go: func autoAdoptDrafts(s *store.Store, now, actor string) []string
pub(crate) fn auto_adopt_drafts(
    s: &pulse_store::Store,
    _now: String,
    _actor: String,
) -> Vec<String> {
    if s.setting("owner.presence", pulse_store::OWNER_PRESENT) != pulse_store::OWNER_AWAY {
        return Vec::new();
    }
    if s.setting("records.auto_adopt", "0") != "1" {
        return Vec::new();
    }
    let drafts = match pulse_records::drafts(s, "") {
        Ok(drafts) => drafts,
        Err(err) => {
            return vec![format!(
                "auto-adopt drafts: {}",
                crate::run::err_message(&err)
            )];
        }
    };
    let mut lines = Vec::new();
    for d in drafts {
        match pulse_records::adopt(
            s,
            &d.record_id,
            pulse_store::HUMAN_AGENT,
            "auto-adopted while owner is away",
        ) {
            Ok(rec) => lines.push(format!(
                "auto-adopted {} {} r{}",
                rec.kind, rec.id, rec.current_rev
            )),
            Err(err) => lines.push(format!(
                "auto-adopt {}: {}",
                d.record_id,
                crate::run::err_message(&err)
            )),
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_records::{DraftInput, NewOptions};
    use rusqlite::params;

    #[test]
    fn auto_adopt_drafts_only_when_away_and_enabled() {
        let _serial = crate::test_helpers::serial();
        let s = auto_adopt_store();
        let rec = seed_record(&s);
        pulse_records::draft(
            &s,
            &rec.id,
            DraftInput {
                body: "auto body".into(),
                author: "aurora".into(),
                note: "test".into(),
                job_id: "job-1".into(),
            },
        )
        .unwrap();

        let lines = auto_adopt_drafts(&s, s.now(), "daemon".into());
        assert!(
            lines.is_empty(),
            "present owner should not auto-adopt: {lines:?}"
        );

        s.set_setting(
            pulse_store::HUMAN_AGENT,
            "owner.presence",
            pulse_store::OWNER_AWAY,
        )
        .unwrap();
        let lines = auto_adopt_drafts(&s, s.now(), "daemon".into());
        assert!(
            lines.is_empty(),
            "away owner with auto_adopt=0 should not adopt: {lines:?}"
        );

        s.set_setting(pulse_store::HUMAN_AGENT, "records.auto_adopt", "1")
            .unwrap();
        let lines = auto_adopt_drafts(&s, s.now(), "daemon".into());
        assert!(
            lines.len() == 1 && lines[0].contains("auto-adopted"),
            "expected one auto-adopted line, got {lines:?}"
        );
        let draft = s.get_draft(&rec.id).unwrap();
        assert!(
            draft.is_none(),
            "draft should be cleared after auto-adoption: {draft:?}"
        );
    }

    fn auto_adopt_store() -> pulse_store::Store {
        let _env = pulse_testutil::env_lock();
        unsafe { std::env::remove_var("PULSE_JOB") };
        let home = tempfile::tempdir().unwrap();
        pulse_store::Store::open(pulse_store::Options {
            home: home.keep().display().to_string(),
            create: true,
            ..Default::default()
        })
        .unwrap()
    }

    fn seed_record(s: &pulse_store::Store) -> pulse_store::Record {
        let path = tempfile::tempdir().unwrap().keep();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug, name, path, added_at, added_by) VALUES (?, ?, ?, ?, ?)",
                params!["proj", "Proj", path.display().to_string(), s.now(), "human"],
            )
            .unwrap();
        pulse_records::new_record(
            s,
            "memory",
            "proj",
            "A lesson",
            NewOptions {
                body: "# A lesson\n".into(),
                author: "aurora".into(),
                ..Default::default()
            },
        )
        .unwrap()
    }
}
