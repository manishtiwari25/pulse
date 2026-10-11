use anyhow::{Result as AnyResult, anyhow};
use rusqlite::{OptionalExtension, Row, ToSql};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;

use crate::cache::cached_compute;
use crate::card_class::{card_fields, first_card_text, normalized_field, normalized_summary};
use crate::text::{check, day_number};
use crate::thresholds::{Thresholds, load_thresholds};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Post {
    pub id: String,
    pub project: String,
    pub area: String,
    pub thread: String,
    pub kind: String,
    pub agent: String,
    pub runner: String,
    pub model: String,
    pub session: String,
    pub date: String,
    pub branch: String,
    pub expires: String,
    #[serde(rename = "on_post")]
    pub on_post: String,
    pub vote: String,
    pub text_hash: String,
    pub has_read_line: i32,
    pub resolution: String,
    pub summary: String,
    pub body: String,
    pub links: String,
    pub files: String,
    pub name: String,
    pub origin_project: String,
    pub extra: String,
    pub origin: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "is_false", default)]
    pub for_owner: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

pub const POST_COLS: &str = "id, project, area, thread, kind, agent, runner, model, session, date, branch, expires, on_post, vote, text_hash, has_read_line, resolution, summary, body, links, files, name, origin_project, extra, origin, created_at";

fn scan_post(row: &Row<'_>) -> rusqlite::Result<Post> {
    Ok(Post {
        id: row.get(0)?,
        project: row.get(1)?,
        area: row.get(2)?,
        thread: row.get(3)?,
        kind: row.get(4)?,
        agent: row.get(5)?,
        runner: row.get(6)?,
        model: row.get(7)?,
        session: row.get(8)?,
        date: row.get(9)?,
        branch: row.get(10)?,
        expires: row.get(11)?,
        on_post: row.get(12)?,
        vote: row.get(13)?,
        text_hash: row.get(14)?,
        has_read_line: row.get(15)?,
        resolution: row.get(16)?,
        summary: row.get(17)?,
        body: row.get(18)?,
        links: row.get(19)?,
        files: row.get(20)?,
        name: row.get(21)?,
        origin_project: row.get(22)?,
        extra: row.get(23)?,
        origin: row.get(24)?,
        created_at: row.get(25)?,
        for_owner: false,
    })
}

fn finish_post(mut p: Post) -> AnyResult<Post> {
    p.for_owner =
        pulse_store::post_for_owner(&p.extra).map_err(|e| anyhow!("post {}: {e}", p.id))?;
    Ok(p)
}

pub fn posts_for(s: &pulse_store::Store, project: &str, area: &str) -> AnyResult<Vec<Post>> {
    let conn = s.conn()?;
    let mut q = format!("SELECT {POST_COLS} FROM posts WHERE project = ?");
    let mut args: Vec<&dyn ToSql> = vec![&project];
    if !area.is_empty() {
        q.push_str(" AND area = ?");
        args.push(&area);
    }
    q.push_str(" ORDER BY area, thread, id");
    let mut stmt = conn.prepare(&q)?;
    let rows = stmt.query_map(args.as_slice(), scan_post)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(finish_post(row?)?);
    }
    Ok(out)
}

/// The posts of `project` whose kind is one of `kinds`, in `posts_for`'s
/// order. Readers that only follow a few kinds (the planner's handoffs and
/// claims) take this instead of every post with its body: the daemon asked
/// for a board's whole history several times a tick.
pub fn posts_of_kinds(
    s: &pulse_store::Store,
    project: &str,
    kinds: &[&str],
) -> AnyResult<Vec<Post>> {
    if kinds.is_empty() {
        return Ok(Vec::new());
    }
    let conn = s.conn()?;
    let marks = vec!["?"; kinds.len()].join(", ");
    let q = format!(
        "SELECT {POST_COLS} FROM posts WHERE project = ? AND kind IN ({marks}) ORDER BY area, thread, id"
    );
    let mut args: Vec<&dyn ToSql> = vec![&project];
    args.extend(kinds.iter().map(|k| k as &dyn ToSql));
    let mut stmt = conn.prepare(&q)?;
    let rows = stmt.query_map(args.as_slice(), scan_post)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(finish_post(row?)?);
    }
    Ok(out)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    pub id: String,
    pub key: String,
    pub agent: String,
    pub expires: String,
    pub status: String,
    pub summary: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub key: String,
    pub posts: i32,
    pub last: String,
    pub status: String,
    pub resolution: String,
    pub last_summary: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub project: String,
    pub key: String,
    pub id: String,
    pub agent: String,
    pub date: String,
    pub status: String,
    pub result_id: String,
    pub result_date: String,
    pub summary: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SVote {
    pub on: String,
    pub agent: String,
    pub vote_id: String,
    pub date: String,
    pub vote: String,
    pub class: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    pub score: i32,
    pub date: String,
    pub id: String,
    pub key: String,
    pub up: i32,
    pub down: i32,
    pub voters: i32,
    pub status: String,
    pub agent: String,
    pub summary: String,
    pub session: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub job: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub job_status: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AVote {
    pub id: String,
    pub session: String,
    pub agent: String,
    pub vote_id: String,
    pub date: String,
    pub vote: String,
    pub class: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Amendment {
    pub date: String,
    pub id: String,
    pub key: String,
    pub status: String,
    pub up: i32,
    pub down: i32,
    pub voters: i32,
    pub quorum: i32,
    pub pct: i32,
    pub nid: i32,
    #[serde(rename = "self")]
    pub self_: i32,
    pub badhash: i32,
    pub noread: i32,
    pub hash: String,
    pub section: String,
    pub agent: String,
    pub summary: String,
    pub open: bool,
    pub session: String,
}

/// Every list here serializes as `null` when empty, never `[]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TallyResult {
    pub project: String,
    pub repo: String,
    pub area: String,
    pub now: String,
    pub params: Thresholds,
    #[serde(with = "pulse_store::gojson::nil_slice", default)]
    pub posts: Vec<Post>,
    pub nokind: i32,
    #[serde(with = "pulse_store::gojson::nil_slice", default)]
    pub claims: Vec<Claim>,
    #[serde(with = "pulse_store::gojson::nil_slice", default)]
    pub threads: Vec<Thread>,
    #[serde(with = "pulse_store::gojson::nil_slice", default)]
    pub proposals: Vec<Proposal>,
    #[serde(with = "pulse_store::gojson::nil_slice", default)]
    pub svotes: Vec<SVote>,
    #[serde(with = "pulse_store::gojson::nil_slice", default)]
    pub suggestions: Vec<Suggestion>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub duplicates: BTreeMap<String, String>,
    #[serde(with = "pulse_store::gojson::nil_slice", default)]
    pub avotes: Vec<AVote>,
    #[serde(with = "pulse_store::gojson::nil_slice", default)]
    pub amendments: Vec<Amendment>,
}

// A map that remembers insertion order: the map is only ever indexed,
// iteration always walks `keys`.
#[derive(Clone)]
struct OMap<K: Eq + Hash + Clone, V> {
    keys: Vec<K>,
    m: HashMap<K, V>,
}

impl<K: Eq + Hash + Clone, V> Default for OMap<K, V> {
    fn default() -> Self {
        Self {
            keys: Vec::new(),
            m: HashMap::new(),
        }
    }
}

impl<K: Eq + Hash + Clone, V> OMap<K, V> {
    fn set(&mut self, k: K, v: V) {
        if !self.m.contains_key(&k) {
            self.keys.push(k.clone());
        }
        self.m.insert(k, v);
    }
    fn get(&self, k: &K) -> Option<&V> {
        self.m.get(k)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenSuggestionJob {
    pub id: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct VoteKey {
    on: String,
    who: String,
}

#[derive(Clone, Default)]
struct ClaimInfo {
    key: String,
    agent: String,
    expires: String,
    summary: String,
}
#[derive(Clone, Default)]
struct SuggInfo {
    key: String,
    agent: String,
    date: String,
    summary: String,
    session: String,
}
#[derive(Clone, Default)]
struct SVoteInfo {
    date: String,
    vote: String,
    id: String,
    session: String,
}
#[derive(Clone, Default)]
struct AmendInfo {
    key: String,
    session: String,
    date: String,
    summary: String,
    agent: String,
    body: String,
}
#[derive(Clone, Default)]
struct AVoteInfo {
    date: String,
    vote: String,
    hash: String,
    read: i32,
    agent: String,
    id: String,
}
#[derive(Clone, Default)]
struct PropInfo {
    id: String,
    agent: String,
    date: String,
    summary: String,
}

pub(crate) fn same_session(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && a == b
}

#[derive(Clone, Default)]
pub(crate) struct OpenSuggestionJobInternal {
    pub id: String,
    pub status: String,
    pub created: String,
}

struct Tally {
    now: String,
    today: i32,
    params: Thresholds,
    built: BTreeMap<String, String>,
    open: BTreeMap<String, OpenSuggestionJobInternal>,
    rows: Vec<Post>,
    nokind: i32,
    order: Vec<String>,
    nposts: BTreeMap<String, i32>,
    last: BTreeMap<String, String>,
    lastsum: BTreeMap<String, String>,
    closed: BTreeMap<String, bool>,
    resol: BTreeMap<String, String>,
    res_id: BTreeMap<String, String>,
    res_date: BTreeMap<String, String>,
    prop: OMap<String, PropInfo>,
    claims: OMap<String, ClaimInfo>,
    suggs: OMap<String, SuggInfo>,
    votes: OMap<VoteKey, SVoteInfo>,
    a_closed: BTreeMap<String, bool>,
    a_resol: BTreeMap<String, String>,
    amends: OMap<String, AmendInfo>,
    avotes: OMap<VoteKey, AVoteInfo>,
}

fn new_tally(
    rows: Vec<Post>,
    params: Thresholds,
    now: &str,
    area: &str,
    built: BTreeMap<String, String>,
    open: BTreeMap<String, OpenSuggestionJobInternal>,
) -> Tally {
    // Amendments are tallied over every area, so the unfiltered rows are kept
    // when an area narrows `t.rows`; without one they are the same vector.
    let (tallied, all) = if area.is_empty() {
        (rows, Vec::new())
    } else {
        (
            rows.iter().filter(|r| r.area == area).cloned().collect(),
            rows,
        )
    };
    let mut t = Tally {
        now: now.to_string(),
        today: day_number(now),
        params,
        built,
        open,
        rows: tallied,
        nokind: 0,
        order: Vec::new(),
        nposts: BTreeMap::new(),
        last: BTreeMap::new(),
        lastsum: BTreeMap::new(),
        closed: BTreeMap::new(),
        resol: BTreeMap::new(),
        res_id: BTreeMap::new(),
        res_date: BTreeMap::new(),
        prop: OMap::default(),
        claims: OMap::default(),
        suggs: OMap::default(),
        votes: OMap::default(),
        a_closed: BTreeMap::new(),
        a_resol: BTreeMap::new(),
        amends: OMap::default(),
        avotes: OMap::default(),
    };
    for r in &t.rows {
        let key = format!("{}/{}", r.area, r.thread);
        if !t.nposts.contains_key(&key) {
            t.order.push(key.clone());
        }
        *t.nposts.entry(key.clone()).or_default() += 1;
        if t.last.get(&key).is_none_or(|last| r.date >= *last) {
            t.last.insert(key.clone(), r.date.clone());
            t.lastsum.insert(key.clone(), r.summary.clone());
        }
        if !r.resolution.is_empty() {
            t.closed.insert(key.clone(), true);
            t.resol.insert(key.clone(), r.resolution.clone());
            t.res_id.insert(key.clone(), r.id.clone());
            t.res_date.insert(key.clone(), r.date.clone());
        }
        if r.resolution == "agreed" {
            t.prop.set(
                key.clone(),
                PropInfo {
                    id: r.id.clone(),
                    agent: r.agent.clone(),
                    date: r.date.clone(),
                    summary: r.summary.clone(),
                },
            );
        }
        if r.kind == "claim" {
            t.claims.set(
                r.id.clone(),
                ClaimInfo {
                    key: key.clone(),
                    agent: r.agent.clone(),
                    expires: r.expires.clone(),
                    summary: r.summary.clone(),
                },
            );
        }
        if r.kind == "suggestion" {
            t.suggs.set(
                r.id.clone(),
                SuggInfo {
                    key: key.clone(),
                    agent: r.agent.clone(),
                    date: r.date.clone(),
                    summary: r.summary.clone(),
                    session: r.session.clone(),
                },
            );
        }
        if r.kind == "vote" && !r.on_post.is_empty() {
            let vk = VoteKey {
                on: r.on_post.clone(),
                who: r.agent.clone(),
            };
            if t.votes.get(&vk).is_none_or(|v| r.date >= v.date) {
                t.votes.set(
                    vk,
                    SVoteInfo {
                        date: r.date.clone(),
                        vote: r.vote.clone(),
                        id: r.id.clone(),
                        session: r.session.clone(),
                    },
                );
            }
        }
        if r.kind.is_empty() {
            t.nokind += 1;
        }
    }
    for r in if area.is_empty() {
        t.rows.iter()
    } else {
        all.iter()
    } {
        let key = format!("{}/{}", r.area, r.thread);
        if !r.resolution.is_empty() {
            t.a_closed.insert(key.clone(), true);
            t.a_resol.insert(key.clone(), r.resolution.clone());
        }
        if r.kind == "amendment" {
            t.amends.set(
                r.id.clone(),
                AmendInfo {
                    key: key.clone(),
                    session: r.session.clone(),
                    date: r.date.clone(),
                    summary: r.summary.clone(),
                    agent: r.agent.clone(),
                    body: r.body.clone(),
                },
            );
        }
        if r.kind == "vote" && !r.on_post.is_empty() {
            let vk = VoteKey {
                on: r.on_post.clone(),
                who: r.session.clone(),
            };
            if t.avotes.get(&vk).is_none_or(|v| r.date >= v.date) {
                t.avotes.set(
                    vk,
                    AVoteInfo {
                        date: r.date.clone(),
                        vote: r.vote.clone(),
                        hash: r.text_hash.clone(),
                        read: r.has_read_line,
                        agent: r.agent.clone(),
                        id: r.id.clone(),
                    },
                );
            }
        }
    }
    t
}

impl Tally {
    fn claim_status(&self, cid: &str) -> String {
        let c = &self.claims.m[cid];
        if self.closed.get(&c.key).copied().unwrap_or(false) {
            "closed".into()
        } else if !c.expires.is_empty() && c.expires.as_str() > self.now.as_str() {
            "active".into()
        } else if c.expires.is_empty() {
            "no expiry".into()
        } else {
            "expired".into()
        }
    }
    fn thread_status(&self, key: &str) -> String {
        if self.closed.get(key).copied().unwrap_or(false) {
            "closed".into()
        } else if self.today - day_number(self.last.get(key).map_or("", String::as_str))
            > self.params.stale_days
        {
            "stale".into()
        } else {
            "open".into()
        }
    }
    fn proposal_status(&self, key: &str) -> String {
        if self.resol.get(key).is_some_and(|s| s == "agreed") {
            "awaiting".into()
        } else {
            self.resol.get(key).cloned().unwrap_or_default()
        }
    }
    fn suggestion_records(&self) -> Vec<Suggestion> {
        let mut out = Vec::new();
        // Group the votes by target once, in vote order, rather than scanning
        // every vote for every suggestion.
        let mut votes_on: HashMap<&str, Vec<&VoteKey>> = HashMap::new();
        for vk in &self.votes.keys {
            votes_on.entry(vk.on.as_str()).or_default().push(vk);
        }
        for sid in &self.suggs.keys {
            let s = &self.suggs.m[sid];
            let is_closed = self.closed.get(&s.key).copied().unwrap_or(false);
            let (mut up, mut down, mut voters) = (0, 0, 0);
            for vk in votes_on.get(sid.as_str()).into_iter().flatten() {
                let v = &self.votes.m[*vk];
                if vk.who == s.agent || same_session(&v.session, &s.session) {
                    continue;
                }
                voters += 1;
                if v.vote == "down" {
                    down += 1;
                } else {
                    up += 1;
                }
            }
            let age = self.today - day_number(&s.date);
            let status = if is_closed {
                "closed".into()
            } else if !self.built.get(sid).cloned().unwrap_or_default().is_empty() {
                self.built[sid].clone()
            } else if voters >= self.params.quorum && up > down {
                "eligible".into()
            } else if voters >= self.params.quorum && up < down {
                "rejected".into()
            } else if voters == 0 && age > self.params.expiry_days {
                "expired".into()
            } else {
                "needs votes".into()
            };
            let mut sg = Suggestion {
                score: up - down,
                date: s.date.clone(),
                id: sid.clone(),
                key: s.key.clone(),
                up,
                down,
                voters,
                status,
                agent: s.agent.clone(),
                summary: s.summary.clone(),
                session: s.session.clone(),
                ..Suggestion::default()
            };
            if let Some(j) = self.open.get(sid)
                && !j.id.is_empty()
            {
                sg.job = j.id.clone();
                sg.job_status = j.status.clone();
            }
            out.push(sg);
        }
        out.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.date.cmp(&b.date))
                .then_with(|| a.id.cmp(&b.id))
        });
        out
    }
    fn svote_records(&self) -> Vec<SVote> {
        let mut out = Vec::new();
        for vk in &self.votes.keys {
            let v = &self.votes.m[vk];
            let Some(s) = self.suggs.get(&vk.on) else {
                continue;
            };
            let class = if vk.who == s.agent || same_session(&v.session, &s.session) {
                "self"
            } else {
                "counted"
            };
            out.push(SVote {
                on: vk.on.clone(),
                agent: vk.who.clone(),
                vote_id: v.id.clone(),
                date: v.date.clone(),
                vote: v.vote.clone(),
                class: class.into(),
            });
        }
        out
    }
    fn amendment_records(&self) -> (Vec<Amendment>, Vec<AVote>) {
        let p = &self.params;
        let mut out = Vec::new();
        let mut avotes = Vec::new();
        let mut avotes_on: HashMap<&str, Vec<&VoteKey>> = HashMap::new();
        for vk in &self.avotes.keys {
            avotes_on.entry(vk.on.as_str()).or_default().push(vk);
        }
        for aid in &self.amends.keys {
            let a = &self.amends.m[aid];
            let chk = check(&a.body, &p.constitution, &p.protected);
            let is_closed = self.a_closed.get(&a.key).copied().unwrap_or(false);
            let (mut up, mut down, mut voters, mut selfv, mut badhash, mut noread) =
                (0, 0, 0, 0, 0, 0);
            let mut idents: BTreeMap<String, bool> = BTreeMap::new();
            for vk in avotes_on.get(aid.as_str()).into_iter().flatten() {
                let v = &self.avotes.m[*vk];
                let cls = if vk.who == a.session || vk.who.is_empty() {
                    selfv += 1;
                    "self"
                } else if v.hash != chk.hash {
                    badhash += 1;
                    "hash mismatch"
                } else if v.read != 1 {
                    noread += 1;
                    "no Read line"
                } else {
                    voters += 1;
                    if v.vote == "down" {
                        down += 1;
                    } else {
                        up += 1;
                    }
                    idents.insert(v.agent.clone(), true);
                    "counted"
                };
                avotes.push(AVote {
                    id: aid.clone(),
                    session: vk.who.clone(),
                    agent: v.agent.clone(),
                    vote_id: v.id.clone(),
                    date: v.date.clone(),
                    vote: v.vote.clone(),
                    class: cls.into(),
                });
            }
            let total = up + down;
            let pct = if total > 0 {
                ((100.0 * f64::from(up) / f64::from(total)) + 0.5) as i32
            } else {
                0
            };
            let anchor = if chk.anchor.is_empty() {
                0
            } else {
                atoi(&chk.anchor)
            };
            let mut status = if chk.hash.is_empty() {
                "malformed".into()
            } else if chk.verdict.starts_with("malformed") || chk.verdict.starts_with("unchecked") {
                chk.verdict.clone()
            } else if chk.verdict != "ok" {
                format!("blocked: {}", chk.verdict)
            } else if anchor == 0 {
                "stale: the replaced text is no longer in the constitution; re-propose".into()
            } else if anchor > 1 {
                format!(
                    "ambiguous: the replaced text appears {} times; quote more context",
                    chk.anchor
                )
            } else if voters < p.amend_quorum {
                "needs votes".into()
            } else if up * p.amend_den >= p.amend_num * total {
                if p.stop_present {
                    "passed; ratification paused (STOP)".into()
                } else {
                    "passed: ratify with pulse amend apply".into()
                }
            } else {
                "short of the supermajority".into()
            };
            if is_closed {
                status = format!(
                    "closed: {}",
                    self.a_resol.get(&a.key).cloned().unwrap_or_default()
                );
            }
            out.push(Amendment {
                date: a.date.clone(),
                id: aid.clone(),
                key: a.key.clone(),
                status,
                up,
                down,
                voters,
                quorum: p.amend_quorum,
                pct,
                nid: idents.len() as i32,
                self_: selfv,
                badhash,
                noread,
                hash: chk.hash,
                section: chk.section,
                agent: a.agent.clone(),
                summary: a.summary.clone(),
                open: !is_closed,
                session: a.session.clone(),
            });
        }
        out.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.id.cmp(&b.id)));
        (out, avotes)
    }
}

pub(crate) fn atoi(s: &str) -> i32 {
    let mut n = 0;
    for c in s.chars() {
        if !c.is_ascii_digit() {
            return n;
        }
        n = n * 10 + (c as i32 - '0' as i32);
    }
    n
}

pub(crate) fn suggestion_jobs(
    s: &pulse_store::Store,
) -> AnyResult<(
    BTreeMap<String, String>,
    BTreeMap<String, OpenSuggestionJobInternal>,
)> {
    let conn = s.conn()?;
    let mut out = BTreeMap::new();
    {
        let mut stmt = conn.prepare("SELECT source, status, kind FROM jobs WHERE source LIKE 'suggestion:%' AND status IN ('done', 'failed', 'cancelled') ORDER BY id")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (source, status, kind) = row?;
            let sid = source.trim_start_matches("suggestion:").to_string();
            let seen = out.contains_key(&sid);
            if status == "done" || !seen {
                let v = if status == "done" && kind == "reflect" {
                    "carded"
                } else if status == "done" {
                    "built"
                } else {
                    "tried"
                };
                out.insert(sid, v.into());
            }
        }
    }
    let mut open = BTreeMap::new();
    let mut stmt = conn.prepare("SELECT id, source, status, created_at FROM jobs WHERE source LIKE 'suggestion:%' AND status IN ('queued', 'leased', 'blocked') ORDER BY id")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (id, source, status, created) = row?;
        let sid = source.trim_start_matches("suggestion:").to_string();
        let cand = OpenSuggestionJobInternal {
            id,
            status,
            created,
        };
        if better_open_job(&cand, open.get(&sid)) {
            open.insert(sid, cand);
        }
    }
    Ok((out, open))
}

pub fn open_suggestion_jobs(
    s: &pulse_store::Store,
) -> AnyResult<BTreeMap<String, OpenSuggestionJob>> {
    let (_, open) = suggestion_jobs(s)?;
    Ok(open
        .into_iter()
        .map(|(id, j)| {
            (
                id,
                OpenSuggestionJob {
                    id: j.id,
                    status: j.status,
                },
            )
        })
        .collect())
}

fn better_open_job(a: &OpenSuggestionJobInternal, b: Option<&OpenSuggestionJobInternal>) -> bool {
    let Some(b) = b else {
        return true;
    };
    fn rank(s: &str) -> i32 {
        match s {
            "blocked" => 1,
            "queued" => 2,
            "leased" => 3,
            _ => 0,
        }
    }
    rank(&a.status)
        .cmp(&rank(&b.status))
        .then_with(|| a.created.cmp(&b.created))
        .then_with(|| a.id.cmp(&b.id))
        .is_gt()
}

pub(crate) fn project_repo(s: &pulse_store::Store, slug: &str) -> AnyResult<String> {
    if slug == pulse_store::COMMONS_SLUG {
        return Ok(String::new());
    }
    let Some(p) = s.project(slug)? else {
        return Err(NoProjectError { slug: slug.into() }.into());
    };
    Ok(p.path)
}

#[derive(Debug, thiserror::Error)]
#[error("No project '{}' (pulse projects lists them)", slug)]
pub struct NoProjectError {
    pub slug: String,
}

pub fn compute(
    s: &pulse_store::Store,
    project: &str,
    area: &str,
    th: Thresholds,
) -> AnyResult<TallyResult> {
    let mut res = compute_light(s, project, area, th)?;
    res.posts = posts_for(s, project, area)?;
    Ok(res)
}

pub fn compute_light(
    s: &pulse_store::Store,
    project: &str,
    area: &str,
    th: Thresholds,
) -> AnyResult<TallyResult> {
    cached_compute(s, project, area, th.clone(), || {
        compute_light_uncached(s, project, area, th)
    })
}

fn compute_light_uncached(
    s: &pulse_store::Store,
    project: &str,
    area: &str,
    th: Thresholds,
) -> AnyResult<TallyResult> {
    let repo = project_repo(s, project)?;
    let mut params = load_thresholds(s, project, th.clone());
    let now = if th.now.is_empty() { s.now() } else { th.now };
    params.now = now.clone();
    let rows = posts_for(s, project, "")?;
    let (built, open) = suggestion_jobs(s)?;
    let t = new_tally(rows, params.clone(), &now, area, built, open);
    let mut res = TallyResult {
        project: project.into(),
        repo,
        area: area.into(),
        now,
        params,
        nokind: t.nokind,
        ..TallyResult::default()
    };
    for cid in &t.claims.keys {
        let c = &t.claims.m[cid];
        res.claims.push(Claim {
            id: cid.clone(),
            key: c.key.clone(),
            agent: c.agent.clone(),
            expires: c.expires.clone(),
            status: t.claim_status(cid),
            summary: c.summary.clone(),
        });
    }
    for key in &t.order {
        let mut th = Thread {
            key: key.clone(),
            posts: t.nposts[key],
            last: t.last[key].clone(),
            status: t.thread_status(key),
            last_summary: t.lastsum[key].clone(),
            ..Thread::default()
        };
        if t.closed.get(key).copied().unwrap_or(false) {
            th.resolution = t.resol[key].clone();
        }
        res.threads.push(th);
    }
    for key in &t.prop.keys {
        let pr = &t.prop.m[key];
        let st = t.proposal_status(key);
        let mut p = Proposal {
            project: project.into(),
            key: key.clone(),
            id: pr.id.clone(),
            agent: pr.agent.clone(),
            date: pr.date.clone(),
            status: st.clone(),
            summary: pr.summary.clone(),
            ..Proposal::default()
        };
        if st != "awaiting" {
            p.result_id = t.res_id.get(key).cloned().unwrap_or_default();
            p.result_date = t.res_date.get(key).cloned().unwrap_or_default();
        }
        res.proposals.push(p);
    }
    res.svotes = t.svote_records();
    res.suggestions = t.suggestion_records();
    // Suggestions only ever point at tallied rows, so the area-filtered rows
    // find every one of them.
    res.duplicates = duplicate_map(&t.rows, &res.suggestions);
    (res.amendments, res.avotes) = t.amendment_records();
    Ok(res)
}

pub fn duplicate_map(posts: &[Post], suggestions: &[Suggestion]) -> BTreeMap<String, String> {
    let post_by_id: BTreeMap<&str, &Post> = posts.iter().map(|p| (p.id.as_str(), p)).collect();
    let mut open: Vec<&Suggestion> = suggestions
        .iter()
        .filter(|sg| sg.status == "needs votes" || sg.status == "eligible")
        .collect();
    open.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.id.cmp(&b.id)));
    let mut seen_summary: BTreeMap<String, String> = BTreeMap::new();
    let mut seen_pair: BTreeMap<String, String> = BTreeMap::new();
    let mut dups = BTreeMap::new();
    let none = Post::default();
    for sg in open {
        let p = post_by_id.get(sg.id.as_str()).copied().unwrap_or(&none);
        let summary = if !p.summary.is_empty() {
            p.summary.clone()
        } else {
            sg.summary.clone()
        };
        let sig = normalized_summary(&summary);
        if !sig.is_empty() {
            if let Some(earlier) = seen_summary.get(&sig) {
                dups.insert(sg.id.clone(), earlier.clone());
                continue;
            }
            seen_summary.insert(sig, sg.id.clone());
        }
        let fields = card_fields(&p.body);
        let file = normalized_field(&first_card_text(&[
            fields.get("file").cloned().unwrap_or_default(),
            fields.get("path").cloned().unwrap_or_default(),
        ]));
        let variable = normalized_field(fields.get("variable").map_or("", String::as_str));
        if !file.is_empty() && !variable.is_empty() {
            let sig = format!("{file}\0{variable}");
            if let Some(earlier) = seen_pair.get(&sig) {
                dups.insert(sg.id.clone(), earlier.clone());
                continue;
            }
            seen_pair.insert(sig, sg.id.clone());
        }
    }
    dups
}

pub fn duplicate_of(res: Option<&TallyResult>, sg: &Suggestion) -> String {
    res.and_then(|r| r.duplicates.get(&sg.id).cloned())
        .unwrap_or_default()
}

pub fn load_post(s: &pulse_store::Store, id: &str) -> AnyResult<Post> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare(&format!("SELECT {POST_COLS} FROM posts WHERE id = ?"))?;
    let p = stmt.query_row([id], scan_post).optional()?;
    match p {
        Some(p) => finish_post(p),
        None => Err(NoPostError { id: id.into() }.into()),
    }
}

#[derive(Debug, thiserror::Error)]
#[error("No post {id:?}")]
pub struct NoPostError {
    pub id: String,
}
