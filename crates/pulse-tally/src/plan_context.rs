use std::collections::BTreeMap;

use crate::card_class::{CARD_CLASS_OPERATING_LAYER, card_class};
use crate::gate::GATE_DUPLICATE;
use crate::gate::GATE_OPERATING_LAYER_CAP;
use crate::tally::{TallyResult, duplicate_map};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanContext {
    pub duplicates: BTreeMap<String, String>,
    pub operating_layer_today: i32,
    pub operating_layer_cap: i32,
    pub operating_layer_cap_set: bool,
}

pub fn plan_context_for(
    s: &pulse_store::Store,
    project: &str,
    now: &str,
    res: Option<&TallyResult>,
) -> PlanContext {
    let mut ctx = PlanContext {
        operating_layer_cap: s.setting_int("plan.operating_layer_per_day") as i32,
        ..PlanContext::default()
    };
    ctx.operating_layer_cap_set = ctx.operating_layer_cap > 0;
    if let Some(res) = res {
        ctx.duplicates = res.duplicates.clone();
        if ctx.duplicates.is_empty() && !res.posts.is_empty() {
            ctx.duplicates = duplicate_map(&res.posts, &res.suggestions);
        }
    }
    if !ctx.operating_layer_cap_set || project.is_empty() || now.len() < 10 {
        return ctx;
    }
    let Ok(conn) = s.conn() else {
        return ctx;
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT COALESCE(p.summary, ''), COALESCE(p.body, '') FROM jobs j \
		JOIN posts p ON p.id = substr(j.source, ?) \
		WHERE j.project = ? AND j.kind = 'improve' AND j.source LIKE 'suggestion:%' AND j.created_at LIKE ?",
    ) else {
        return ctx;
    };
    let rows = stmt.query_map(
        rusqlite::params![
            "suggestion:".len() as i64 + 1,
            project,
            format!("{}%", &now[..10])
        ],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    );
    if let Ok(rows) = rows {
        for row in rows.flatten() {
            if card_class(&row.0, &row.1) == CARD_CLASS_OPERATING_LAYER {
                ctx.operating_layer_today += 1;
            }
        }
    }
    ctx
}

pub fn duplicate_gate(id: &str) -> String {
    format!("{GATE_DUPLICATE} {id}").trim().to_string()
}

pub fn operating_layer_cap_gate(today: i32, cap: i32) -> String {
    format!(
        "{GATE_OPERATING_LAYER_CAP}: {today} of {cap} today; product cards go first (plan.operating_layer_per_day)"
    )
}
