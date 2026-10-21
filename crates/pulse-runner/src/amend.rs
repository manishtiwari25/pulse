use std::collections::BTreeMap;

pub(crate) type AmendRefusals = BTreeMap<String, String>;

pub(crate) fn auto_apply_amendments(
    s: &pulse_store::Store,
    slug: String,
    now: String,
    refused: &mut AmendRefusals,
) -> Vec<String> {
    let mut lines = Vec::new();
    let th = pulse_tally::load_thresholds(
        s,
        &slug,
        pulse_tally::Thresholds {
            now,
            ..Default::default()
        },
    );
    let passed = match pulse_tally::passed_amendments(s, &slug, th.clone()) {
        Ok(passed) => passed,
        Err(err) => {
            return vec![format!(
                "amendments of {slug}: {}",
                crate::run::err_message(&err)
            )];
        }
    };
    for amendment in passed {
        if refused.contains_key(&amendment.id) {
            continue;
        }
        let applied = match pulse_tally::apply_amendment(s, &amendment.id, th.clone()) {
            Ok(applied) => applied,
            Err(err) => {
                let mut why = crate::run::err_message(&err);
                if let Some(ae) = err.downcast_ref::<pulse_tally::ApplyError>() {
                    why = format!("{}: {}", ae.what, ae.why).trim().to_string();
                    if !ae.fix.is_empty() {
                        why.push_str(&format!(" ({})", ae.fix));
                    }
                }
                refused.insert(amendment.id.clone(), why.clone());
                lines.push(format!(
                    "amendment {} of {} left to the human: {}",
                    amendment.id, slug, why
                ));
                continue;
            }
        };
        if !applied.already_at.is_empty() {
            lines.push(format!(
                "amendment {} of {} was applied at {}; ratifying",
                amendment.id, slug, applied.already_at
            ));
        } else {
            lines.push(format!(
                "amendment {} of {} applied: {} rev {}",
                amendment.id, slug, applied.record_id, applied.rev
            ));
        }
        if let Err(err) = pulse_tally::ratify(s, &applied, "") {
            let why = crate::run::err_message(&err);
            refused.insert(amendment.id.clone(), why.clone());
            lines.push(format!(
                "amendment {} of {} applied but not ratified: {}",
                amendment.id, slug, why
            ));
        }
    }
    lines
}
