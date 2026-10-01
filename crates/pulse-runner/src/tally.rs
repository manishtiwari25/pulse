//! Port of internal/runner/tally.go.

use serde_json::json;

/// Go: func suggestionRecords(s *store.Store, slug, now string) []tally.Suggestion
#[allow(dead_code)]
pub(crate) fn suggestion_records(
    s: &pulse_store::Store,
    slug: String,
    now: String,
) -> Vec<pulse_tally::Suggestion> {
    match pulse_tally::compute_light(
        s,
        &slug,
        "",
        pulse_tally::Thresholds {
            now,
            ..Default::default()
        },
    ) {
        Ok(res) => res.suggestions,
        Err(_) => Vec::new(),
    }
}

/// Go: func ballotItemsFor(s *store.Store, slug, agent, now, session string) []ballotItem
pub(crate) fn ballot_items_for(
    s: &pulse_store::Store,
    slug: String,
    agent: String,
    now: String,
    session: String,
) -> Vec<serde_json::Value> {
    let mut session = session;
    if !agent.is_empty() && session.is_empty() {
        let conn = match s.conn() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        session = crate::board::agent_session(crate::board::agent_row(&conn, agent.clone()));
    }
    let res = match pulse_tally::compute_light(
        s,
        &slug,
        "",
        pulse_tally::Thresholds {
            now,
            ..Default::default()
        },
    ) {
        Ok(res) => res,
        Err(_) => return Vec::new(),
    };
    let quorum = res.params.quorum;
    pulse_tally::limit_ballot_items(
        res.ballot(&agent, &session),
        quorum,
        pulse_tally::ballot_cap(s),
    )
    .into_iter()
    .filter_map(|it| {
        let sg = it.suggestion?;
        if it.kind != "suggestion" {
            return None;
        }
        Some(json!({
            "id": sg.id,
            "key": sg.key,
            "summary": sg.summary,
            "body": "",
            "up": sg.up,
            "down": sg.down
        }))
    })
    .collect()
}
