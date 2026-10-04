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
    .filter_map(|it| match it.kind.as_str() {
        "suggestion" => {
            let sg = it.suggestion?;
            Some(json!({
                "id": sg.id,
                "key": sg.key,
                "summary": sg.summary,
                "body": "",
                "up": sg.up,
                "down": sg.down,
                "kind": "suggestion",
                "hash": ""
            }))
        }
        "amendment" => {
            let a = it.amendment?;
            if !amendment_votable(&a) {
                return None;
            }
            Some(json!({
                "id": a.id,
                "key": a.key,
                "summary": a.summary,
                "body": "",
                "up": a.up,
                "down": a.down,
                "kind": "amendment",
                "hash": a.hash
            }))
        }
        _ => None,
    })
    .collect()
}

/// An amendment belongs on a ballot only while more votes can still change
/// its outcome; stale, ambiguous, malformed, blocked, passed, and closed
/// proposals are decided by a re-proposal, a ratification, or a human.
pub(crate) fn amendment_votable(a: &pulse_tally::Amendment) -> bool {
    a.open && (a.status == "needs votes" || a.status == "short of the supermajority")
}
