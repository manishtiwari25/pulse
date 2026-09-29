use anyhow::Result;

use crate::tally::{TallyResult, compute};
use crate::thresholds::Thresholds;

pub fn records(
    s: &pulse_store::Store,
    project: &str,
    area: &str,
    th: Thresholds,
) -> Result<String> {
    Ok(compute(s, project, area, th)?.records())
}

impl TallyResult {
    pub fn records(&self) -> String {
        let mut b = String::new();
        fn add(b: &mut String, fields: &[String]) {
            b.push_str(&fields.join("\t"));
            b.push('\n');
        }
        macro_rules! p {
            ($($x:expr),* $(,)?) => {{
                add(&mut b, &[$(field($x)),*]);
            }};
        }
        let th = &self.params;
        p!("param", "board_present", 1);
        p!("param", "repo", &self.repo);
        p!("param", "area", &self.area);
        p!("param", "now", &self.now);
        p!("param", "quorum", th.quorum);
        p!("param", "expiry_days", th.expiry_days);
        p!("param", "stale_days", th.stale_days);
        p!("param", "amend_quorum", th.amend_quorum);
        p!(
            "param",
            "amend_majority",
            format!("{}/{}", th.amend_num, th.amend_den)
        );
        p!("param", "protected", &th.protected);
        p!("param", "constitution_present", th.constitution_present);
        p!("param", "stop_present", th.stop_present);
        for x in &self.posts {
            p!(
                "post",
                format!("store:{}", x.id),
                &x.area,
                &x.thread,
                &x.id,
                &x.kind,
                &x.agent,
                &x.date,
                &x.expires,
                &x.on_post,
                &x.vote,
                &x.resolution,
                x.summary.replace('\t', " "),
                &x.session,
                &x.text_hash,
                x.has_read_line
            );
        }
        p!("nokind", self.nokind);
        for c in &self.claims {
            p!(
                "claim", &c.id, &c.key, &c.agent, &c.expires, &c.status, &c.summary
            );
        }
        for t in &self.threads {
            p!(
                "thread",
                &t.key,
                t.posts,
                &t.last,
                &t.status,
                &t.resolution,
                &t.last_summary
            );
        }
        for pr in &self.proposals {
            p!(
                "proposal",
                &pr.key,
                &pr.id,
                &pr.agent,
                &pr.date,
                &pr.status,
                &pr.result_id,
                &pr.result_date,
                &pr.summary
            );
        }
        for v in &self.svotes {
            p!(
                "svote", &v.on, &v.agent, &v.vote_id, &v.date, &v.vote, &v.class
            );
        }
        for x in &self.suggestions {
            p!(
                "suggestion",
                x.score,
                &x.date,
                &x.id,
                &x.key,
                x.up,
                x.down,
                x.voters,
                &x.status,
                &x.agent,
                &x.summary
            );
        }
        for v in &self.avotes {
            p!(
                "avote", &v.id, &v.session, &v.agent, &v.vote_id, &v.date, &v.vote, &v.class
            );
        }
        for a in &self.amendments {
            p!(
                "amendment",
                &a.date,
                &a.id,
                &a.key,
                &a.status,
                a.up,
                a.down,
                a.voters,
                a.quorum,
                a.pct,
                a.nid,
                a.self_,
                a.badhash,
                a.noread,
                &a.hash,
                &a.section,
                &a.agent,
                &a.summary
            );
        }
        b
    }
}

fn field(v: impl Field) -> String {
    v.field()
}

trait Field {
    fn field(self) -> String;
}
impl Field for &str {
    fn field(self) -> String {
        self.to_string()
    }
}
impl Field for &String {
    fn field(self) -> String {
        self.clone()
    }
}
impl Field for String {
    fn field(self) -> String {
        self
    }
}
impl Field for i32 {
    fn field(self) -> String {
        self.to_string()
    }
}
impl Field for i64 {
    fn field(self) -> String {
        self.to_string()
    }
}
impl Field for bool {
    fn field(self) -> String {
        if self { "1" } else { "0" }.into()
    }
}
