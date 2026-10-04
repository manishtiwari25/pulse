use anyhow::{Result, anyhow};
use regex::Regex;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

pub(crate) type JsonMap = BTreeMap<String, Value>;

/// Go's nil map read: every key is absent.
static EMPTY_ROW: JsonMap = JsonMap::new();

/// Go: d.ByID[id]
fn post_at<'a>(
    by_id: &BTreeMap<String, usize>,
    posts: &'a [JsonMap],
    id: &str,
) -> Option<&'a JsonMap> {
    by_id.get(id).map(|&i| &posts[i])
}

pub trait TallySource: Send + Sync {
    fn records(&self, project: &str, area: &str) -> Result<String>;
}

/// Go: type nativeTally struct{ store *store.Store }
pub struct NativeTally {
    pub store: std::sync::Arc<pulse_store::Store>,
}

impl TallySource for NativeTally {
    fn records(&self, project: &str, area: &str) -> Result<String> {
        pulse_tally::records(
            &self.store,
            project,
            area,
            pulse_tally::Thresholds::default(),
        )
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BoardData {
    pub(crate) meta: JsonMap,
    pub(crate) params: JsonMap,
    pub(crate) posts: Vec<JsonMap>,
    /// Go's ByID shares the post maps with Posts; here it indexes into them.
    pub(crate) by_id: BTreeMap<String, usize>,
    pub(crate) claims: Vec<JsonMap>,
    pub(crate) threads: Vec<JsonMap>,
    pub(crate) proposals: Vec<JsonMap>,
    /// Open questions addressed to the owner (`for_owner`, and the evolution
    /// loop's stop questions by construction): what waits for a yes or no
    /// that no worker may give.
    pub(crate) questions: Vec<JsonMap>,
    pub(crate) suggestions: Vec<JsonMap>,
    pub(crate) amendments: Vec<JsonMap>,
    pub(crate) evolution: JsonMap,
}

impl crate::server::Handler {
    pub(crate) fn entry<S: AsRef<str>>(&self, slug: S) -> Result<pulse_store::Project> {
        let slug = slug.as_ref();
        let store = self
            .store
            .as_deref()
            .ok_or_else(|| anyhow!("the store is unavailable"))?;
        if slug == pulse_store::COMMONS_SLUG {
            let (mut value, _) = store.meta("commons_path")?;
            if value.is_empty() {
                value = Path::new(&store.home)
                    .join("commons")
                    .to_string_lossy()
                    .to_string();
            }
            return Ok(pulse_store::Project {
                slug: slug.to_string(),
                name: slug.to_string(),
                path: value,
                status: "active".to_string(),
                ..Default::default()
            });
        }
        let Some(p) = store.project(slug)? else {
            return Err(pulse_tally::NoProjectError { slug: slug.into() }.into());
        };
        if p.status != "active" {
            return Err(pulse_tally::NoProjectError { slug: slug.into() }.into());
        }
        Ok(p)
    }

    pub(crate) fn snapshot(&self, project: &str, area: &str) -> Result<BoardData> {
        let store = self
            .store
            .as_deref()
            .ok_or_else(|| anyhow!("the store is unavailable"))?;
        let p = self.entry(project)?;
        let text = if let Some(source) = &self.source {
            source.records(project, area)?
        } else {
            pulse_tally::records(store, project, area, pulse_tally::Thresholds::default())?
        };
        let raw_posts = pulse_tally::posts_for(store, project, "")?;
        let open_jobs = pulse_tally::open_suggestion_jobs(store)?;
        let db_posts = raw_posts
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect::<BTreeMap<_, _>>();
        let mut d = BoardData {
            params: JsonMap::new(),
            by_id: BTreeMap::new(),
            posts: Vec::new(),
            claims: Vec::new(),
            threads: Vec::new(),
            proposals: Vec::new(),
            suggestions: Vec::new(),
            amendments: Vec::new(),
            ..Default::default()
        };
        let mut params = BTreeMap::<String, String>::new();
        let mut svotes = BTreeMap::<String, Vec<JsonMap>>::new();
        let mut avotes = BTreeMap::<String, Vec<JsonMap>>::new();
        let mut nokind = "0".to_string();
        for line in text.trim_end_matches('\n').split('\n') {
            if line.is_empty() {
                continue;
            }
            let fields = line.split('\t').collect::<Vec<_>>();
            if fields.is_empty() {
                continue;
            }
            let kind = fields[0];
            let mut f = fields[1..].to_vec();
            while f.len() < 17 {
                f.push("");
            }
            match kind {
                "param" => {
                    params.insert(f[0].to_string(), f[1].to_string());
                }
                "nokind" => nokind = f[0].to_string(),
                "post" => {
                    let post = db_posts
                        .get(f[3])
                        .ok_or_else(|| anyhow!("tally references missing post {:?}", f[3]))?;
                    let fm = post_frontmatter(post);
                    let mut missing = Vec::new();
                    for key in ["agent", "runner", "model", "session", "date", "branch"] {
                        if str_value(fm.get(key).unwrap_or(&Value::Null)).is_empty() {
                            missing.push(key.to_string());
                        }
                    }
                    let row = object(
                        "id rel_path area thread thread_key kind agent runner model session date branch expires on_post vote text_hash has_read_line resolution summary links files frontmatter body provenance_ok provenance_missing in_area name project",
                        vec![
                            json!(f[3]), json!(f[0]), json!(f[1]), json!(f[2]), json!(format!("{}/{}", f[1], f[2])),
                            json!(f[4]), json!(f[5]), json!(post.runner), json!(post.model), json!(f[12]),
                            json!(f[6]), json!(post.branch), json!(f[7]), json!(f[8]), json!(f[9]), json!(f[13]),
                            json!(if f[14] == "1" { 1 } else { 0 }), json!(f[10]), json!(f[11]), json!(post.links),
                            json!(post.files), json!(fm), json!(post.body.trim_matches('\n')), json!(if missing.is_empty() { 1 } else { 0 }),
                            json!(missing), json!(if area.is_empty() || area == f[1] { 1 } else { 0 }),
                            json!(truncate(&post.name, 40)), json!(truncate(&post.origin_project, 60)),
                        ],
                    );
                    d.by_id.insert(f[3].to_string(), d.posts.len());
                    d.posts.push(row);
                }
                "claim" => d.claims.push(object("id thread_key agent expires status summary", vals(&f[0..6]))),
                "thread" => {
                    let (area, thread) = f[0].split_once('/').unwrap_or((f[0], ""));
                    d.threads.push(object(
                        "key area thread posts last status resolution summary",
                        vec![json!(f[0]), json!(area), json!(thread), json!(num_str(f[1])), json!(f[2]), json!(f[3]), json!(f[4]), json!(f[5])],
                    ));
                }
                "proposal" => d.proposals.push(object("thread_key id agent date status decided_id decided_date summary", vals(&f[0..8]))),
                "suggestion" => d.suggestions.push(object(
                    "score date id thread_key up down voters status agent summary",
                    vec![json!(num_str(f[0])), json!(f[1]), json!(f[2]), json!(f[3]), json!(num_str(f[4])), json!(num_str(f[5])), json!(num_str(f[6])), json!(f[7]), json!(f[8]), json!(f[9])],
                )),
                "svote" => svotes.entry(f[0].to_string()).or_default().push(object("agent post_id date vote classification", vals(&f[1..6]))),
                "amendment" => d.amendments.push(object(
                    "date id thread_key status up down voters quorum pct_up identities uncounted_self uncounted_hash_mismatch uncounted_no_read_line text_hash section agent summary",
                    vec![json!(f[0]), json!(f[1]), json!(f[2]), json!(f[3]), json!(num_str(f[4])), json!(num_str(f[5])), json!(num_str(f[6])), json!(num_str(f[7])), json!(num_str(f[8])), json!(num_str(f[9])), json!(num_str(f[10])), json!(num_str(f[11])), json!(num_str(f[12])), json!(f[13]), json!(f[14]), json!(f[15]), json!(f[16])],
                )),
                "avote" => avotes.entry(f[0].to_string()).or_default().push(object("session agent post_id date vote classification", vals(&f[1..7]))),
                _ => return Err(anyhow!("unknown tally record kind {:?}", kind)),
            }
        }
        d.params = object(
            "area now quorum expiry_days stale_days amend_quorum amend_majority protected_sections constitution_present stop_present",
            vec![
                json!(params.get("area").cloned().unwrap_or_default()),
                json!(params.get("now").cloned().unwrap_or_default()),
                json!(num_str(
                    params.get("quorum").map(String::as_str).unwrap_or("")
                )),
                json!(num_str(
                    params.get("expiry_days").map(String::as_str).unwrap_or("")
                )),
                json!(num_str(
                    params.get("stale_days").map(String::as_str).unwrap_or("")
                )),
                json!(num_str(
                    params.get("amend_quorum").map(String::as_str).unwrap_or("")
                )),
                json!(params.get("amend_majority").cloned().unwrap_or_default()),
                json!(params.get("protected").cloned().unwrap_or_default()),
                json!(params.get("constitution_present").is_some_and(|v| v == "1")),
                json!(params.get("stop_present").is_some_and(|v| v == "1")),
            ],
        );
        let now = params.get("now").cloned().unwrap_or_default();
        let plan_res = pulse_tally::compute_light(
            store,
            project,
            "",
            pulse_tally::Thresholds {
                now: now.clone(),
                ..Default::default()
            },
        )
        .ok();
        let plan_ctx = pulse_tally::plan_context_for(store, project, &now, plan_res.as_ref());
        d.enrich_threads();
        d.collect_owner_questions(&decided_posts(store, project)?);
        for row in &mut d.proposals {
            let post = post_at(&d.by_id, &d.posts, &str_key(row, "id")).unwrap_or(&EMPTY_ROW);
            let decided =
                post_at(&d.by_id, &d.posts, &str_key(row, "decided_id")).unwrap_or(&EMPTY_ROW);
            for key in ["name", "session", "rel_path", "body", "kind"] {
                row.insert(key.to_string(), json!(str_key(post, key)));
            }
            for key in ["summary", "agent", "name"] {
                row.insert(format!("decided_{key}"), json!(str_key(decided, key)));
            }
            row.insert(
                "proposal_text".to_string(),
                json!(proposal_text(&str_key(row, "body"))),
            );
            row.insert("can_decide".to_string(), json!(true));
        }
        order(&mut d.proposals, &["-date", "id"]);
        d.proposals.sort_by(|a, b| {
            let aa = str_key(a, "status") == "awaiting";
            let bb = str_key(b, "status") == "awaiting";
            bb.cmp(&aa)
        });
        d.suggestions.sort_by(suggestion_cmp);
        let mut rank = 0;
        for row in &mut d.suggestions {
            let id = str_key(row, "id");
            let post = post_at(&d.by_id, &d.posts, &id).unwrap_or(&EMPTY_ROW);
            row.insert("name".into(), json!(str_key(post, "name")));
            row.insert("session".into(), json!(str_key(post, "session")));
            let job = open_jobs.get(&id).cloned().unwrap_or_default();
            row.insert("job".into(), json!(job.id));
            row.insert("job_status".into(), json!(job.status));
            let mut gate = String::new();
            if str_key(row, "status") == "eligible" {
                let post_ref = db_posts.get(&id);
                gate = pulse_tally::plan_gate_with(
                    store,
                    plan_ctx.clone(),
                    &p.path,
                    &pulse_tally::Suggestion {
                        score: num_value(row.get("score").unwrap_or(&Value::Null)) as i32,
                        id: id.clone(),
                        key: str_key(row, "thread_key"),
                        agent: str_key(row, "agent"),
                        summary: str_key(row, "summary"),
                        job: str_key(row, "job"),
                        job_status: str_key(row, "job_status"),
                        ..Default::default()
                    },
                    post_ref,
                );
            }
            let plannable = str_key(row, "status") == "eligible" && gate.is_empty();
            row.insert("gate".into(), json!(gate));
            row.insert("plannable".into(), json!(plannable));
            row.insert("rank".into(), Value::Null);
            if suggestion_priority(&str_key(row, "status")) < 3 {
                rank += 1;
                row.insert("rank".into(), json!(rank));
            }
            let mut votes = svotes.remove(&id).unwrap_or_default();
            for vote in &mut votes {
                let post =
                    post_at(&d.by_id, &d.posts, &str_key(vote, "post_id")).unwrap_or(&EMPTY_ROW);
                vote.insert("reason".into(), json!(str_key(post, "summary")));
                vote.insert("name".into(), json!(str_key(post, "name")));
                vote.insert("session".into(), json!(str_key(post, "session")));
            }
            order(&mut votes, &["date", "agent"]);
            row.insert("votes".into(), json!(votes));
        }
        order(&mut d.amendments, &["date", "id"]);
        d.amendments.sort_by(|a, b| {
            let ac = str_key(a, "status").starts_with("closed:");
            let bc = str_key(b, "status").starts_with("closed:");
            ac.cmp(&bc)
        });
        for row in &mut d.amendments {
            let id = str_key(row, "id");
            let post = post_at(&d.by_id, &d.posts, &id).unwrap_or(&EMPTY_ROW);
            row.insert("name".into(), json!(str_key(post, "name")));
            row.insert("session".into(), json!(str_key(post, "session")));
            let mut votes = avotes.remove(&id).unwrap_or_default();
            for vote in &mut votes {
                let post =
                    post_at(&d.by_id, &d.posts, &str_key(vote, "post_id")).unwrap_or(&EMPTY_ROW);
                vote.insert("reason".into(), json!(str_key(post, "summary")));
                vote.insert("name".into(), json!(str_key(post, "name")));
                let mut read_line = String::new();
                for line in str_key(post, "body").split('\n') {
                    if let Some(rest) = line.strip_prefix("Read:") {
                        read_line = rest.trim().to_string();
                        break;
                    }
                }
                vote.insert("read_line".into(), json!(read_line));
            }
            order(&mut votes, &["date", "session"]);
            row.insert("votes".into(), json!(votes));
        }
        d.evolution = self.evolution(project.to_string()).unwrap_or_else(|_| {
            object(
                "loop archive",
                vec![json!({"loop":"","stop":"","limits":""}), json!({"rows":[]})],
            )
        });
        let evo_summary = crate::evolution::evolution_summary(d.evolution.clone());
        d.meta = object(
            "schema_version built_at now repo repo_name remote branch head dirty board_present evolution_present tally_script tally_mode board_params posts_without_kind last_error",
            vec![
                json!("3"),
                json!(store.now()),
                json!(now),
                json!(p.path),
                json!(
                    Path::new(&p.path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                ),
                json!(git_value(
                    &p.path,
                    &["config", "--get", "remote.origin.url"]
                )),
                json!(git_value(&p.path, &["rev-parse", "--abbrev-ref", "HEAD"])),
                json!(git_value(&p.path, &["rev-parse", "--short", "HEAD"])),
                json!(if git_value(
                    &p.path,
                    &[
                        "status",
                        "--porcelain",
                        "--",
                        "docs/board",
                        "docs/evolution"
                    ]
                )
                .is_empty()
                {
                    "0"
                } else {
                    "1"
                }),
                json!(params.get("board_present").cloned().unwrap_or_default()),
                evo_summary
                    .get("evolution_present")
                    .cloned()
                    .unwrap_or_else(|| json!("")),
                json!(""),
                json!("go"),
                json!(d.params.clone()),
                json!(nokind),
                json!(""),
            ],
        );
        if let Ok(metadata) = self.metadata() {
            for (k, v) in metadata {
                d.meta.insert(k, v);
            }
        }
        Ok(d)
    }
}

impl BoardData {
    pub(crate) fn thread(&self, key: &str) -> JsonMap {
        for thread in &self.threads {
            if str_key(thread, "key") != key {
                continue;
            }
            let mut posts = self.posts.iter()
                .filter(|p| str_key(p, "thread_key") == key)
                .map(|p| pick(p, "id rel_path kind agent name project runner model session date branch expires on_post vote text_hash has_read_line resolution summary provenance_ok provenance_missing"))
                .collect::<Vec<_>>();
            order(&mut posts, &["date", "id"]);
            return object("thread posts", vec![json!(thread), json!(posts)]);
        }
        JsonMap::new()
    }

    pub(crate) fn enrich_threads(&mut self) {
        let mut threads = BTreeMap::<String, usize>::new();
        for (idx, thread) in self.threads.iter_mut().enumerate() {
            let key = str_key(thread, "key");
            threads.insert(key, idx);
            thread.insert("participants".into(), json!(Vec::<String>::new()));
            thread.insert("last_by".into(), json!(""));
            thread.insert("last_kind".into(), json!(""));
            thread.insert("open_questions".into(), json!(0));
            thread.insert("open_handoffs".into(), json!(0));
            thread.insert("active_claims".into(), json!(Vec::<String>::new()));
        }
        // Go sorts a copy of the slice; the maps behind it are shared.
        let mut posts = self.posts.iter().collect::<Vec<_>>();
        order(&mut posts, &["date", "id"]);
        let mut seen = BTreeMap::<String, BTreeSet<String>>::new();
        let mut by_thread = BTreeMap::<String, Vec<&JsonMap>>::new();
        for &post in &posts {
            let key = str_key(post, "thread_key");
            let Some(&idx) = threads.get(&key) else {
                continue;
            };
            by_thread.entry(key.clone()).or_default().push(post);
            let agent = str_key(post, "agent");
            if !agent.is_empty() && seen.entry(key.clone()).or_default().insert(agent.clone()) {
                let mut values = self.threads[idx]
                    .get("participants")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                values.push(json!(agent));
                self.threads[idx].insert("participants".into(), Value::Array(values));
            }
            if str_key(post, "date") >= str_key(&self.threads[idx], "last") {
                self.threads[idx].insert("last_by".into(), json!(str_key(post, "agent")));
                self.threads[idx].insert("last_kind".into(), json!(str_key(post, "kind")));
            }
        }
        for claim in &self.claims {
            if str_key(claim, "status") != "active" {
                continue;
            }
            let key = str_key(claim, "thread_key");
            if let Some(&idx) = threads.get(&key) {
                let mut values = self.threads[idx]
                    .get("active_claims")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                values.push(json!(str_key(claim, "agent")));
                self.threads[idx].insert("active_claims".into(), Value::Array(values));
            }
        }
        let answered = posts
            .iter()
            .filter(|p| {
                matches!(str_key(p, "kind").as_str(), "answer" | "result")
                    && !str_key(p, "on_post").is_empty()
            })
            .map(|p| str_key(p, "on_post"))
            .collect::<BTreeSet<_>>();
        for (key, thread_posts) in by_thread {
            let Some(&idx) = threads.get(&key) else {
                continue;
            };
            if str_key(&self.threads[idx], "status") == "closed" {
                continue;
            }
            for (i, post) in thread_posts.iter().enumerate() {
                match str_key(post, "kind").as_str() {
                    "question"
                        if str_key(post, "area") != "jobs"
                            && !answered.contains(&str_key(post, "id"))
                            && !later_hub_answer_date(&thread_posts, i) =>
                    {
                        inc(&mut self.threads[idx], "open_questions");
                    }
                    "handoff" if !later_hub_kind(&thread_posts, i, &["claim", "result"]) => {
                        inc(&mut self.threads[idx], "open_handoffs");
                    }
                    _ => {}
                }
            }
        }
    }

    /// Questions that wait for the owner: posts marked `for_owner` and the
    /// evolution loop's stop questions (asks for the owner by construction).
    /// A question is open until the owner decides it, an answer lands on it or
    /// later in its thread, or its thread closes. A stop question ignores a
    /// closure dated before it: decisions close the loop-gate thread, and the
    /// loop keeps posting new stops into it. Card-needed asks are the
    /// planner's own work since ADR-044 and never wait for the owner.
    pub(crate) fn collect_owner_questions(&mut self, decided: &BTreeSet<String>) {
        let threads = self
            .threads
            .iter()
            .map(|t| {
                (
                    str_key(t, "key"),
                    (str_key(t, "status"), str_key(t, "last")),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut posts = self.posts.iter().collect::<Vec<_>>();
        order(&mut posts, &["date", "id"]);
        let answered = posts
            .iter()
            .filter(|p| str_key(p, "kind") == "answer" && !str_key(p, "on_post").is_empty())
            .map(|p| str_key(p, "on_post"))
            .collect::<BTreeSet<_>>();
        let mut by_thread = BTreeMap::<String, Vec<&JsonMap>>::new();
        for &post in &posts {
            by_thread
                .entry(str_key(post, "thread_key"))
                .or_default()
                .push(post);
        }
        let mut rows = Vec::new();
        for (key, thread_posts) in &by_thread {
            let (status, last) = threads.get(key).cloned().unwrap_or_default();
            for (i, post) in thread_posts.iter().enumerate() {
                if str_key(post, "kind") != "question" || str_key(post, "area") == "jobs" {
                    continue;
                }
                let stop = str_key(post, "summary") == pulse_store::EVOLUTION_STOP_SUMMARY;
                let for_owner = post
                    .get("frontmatter")
                    .and_then(|fm| fm.get("for_owner"))
                    .is_some_and(|v| str_value(v) == "true");
                if !(stop || for_owner) {
                    continue;
                }
                let id = str_key(post, "id");
                if decided.contains(&id)
                    || answered.contains(&id)
                    || later_hub_answer_date(thread_posts, i)
                {
                    continue;
                }
                if stop && str_key(post, "body").contains(pulse_evolve::CARD_NEEDED) {
                    continue;
                }
                let resolved = if stop {
                    thread_posts[i + 1..]
                        .iter()
                        .any(|p| !str_key(p, "resolution").is_empty())
                } else {
                    status == "closed"
                };
                if resolved {
                    continue;
                }
                rows.push(object(
                    "id thread_key area thread kind agent name session date summary body on_post stop stale thread_last",
                    vec![
                        json!(id),
                        json!(key),
                        json!(str_key(post, "area")),
                        json!(str_key(post, "thread")),
                        json!("question"),
                        json!(str_key(post, "agent")),
                        json!(str_key(post, "name")),
                        json!(str_key(post, "session")),
                        json!(str_key(post, "date")),
                        json!(str_key(post, "summary")),
                        json!(str_key(post, "body")),
                        json!(str_key(post, "on_post")),
                        json!(stop),
                        json!(status == "stale"),
                        json!(last),
                    ],
                ));
            }
        }
        order(&mut rows, &["-date", "id"]);
        rows.sort_by_key(|r| !r.get("stop").and_then(Value::as_bool).unwrap_or(false));
        self.questions = rows;
    }

    /// Go: d.ByID[id]
    pub(crate) fn post_by_id(&self, id: &str) -> Option<&JsonMap> {
        post_at(&self.by_id, &self.posts, id)
    }

    pub(crate) fn post(&self, id: &str) -> JsonMap {
        let Some(p) = self.post_by_id(id) else {
            return JsonMap::new();
        };
        let mut result = p.clone();
        let mut responses = self
            .posts
            .iter()
            .filter(|row| str_key(row, "on_post") == id)
            .map(|row| pick(row, "id kind agent name session date vote summary"))
            .collect::<Vec<_>>();
        order(&mut responses, &["date"]);
        result.insert("responses".into(), json!(responses));
        result
    }

    pub(crate) fn overview(&self) -> JsonMap {
        let (mut active, mut stale) = (Vec::new(), Vec::new());
        for claim in &self.claims {
            let mut row = claim.clone();
            let post = self.post_by_id(&str_key(&row, "id"));
            row.insert(
                "name".into(),
                json!(post.map(|p| str_key(p, "name")).unwrap_or_default()),
            );
            row.insert(
                "session".into(),
                json!(post.map(|p| str_key(p, "session")).unwrap_or_default()),
            );
            match str_key(&row, "status").as_str() {
                "active" => active.push(row),
                "expired" | "no expiry" => stale.push(row),
                _ => {}
            }
        }
        order(&mut active, &["expires"]);
        order(&mut stale, &["-expires"]);
        let mut open = self
            .threads
            .iter()
            .filter(|t| matches!(str_key(t, "status").as_str(), "open" | "stale"))
            .cloned()
            .collect::<Vec<_>>();
        order(&mut open, &["-last"]);
        // Go picks every post and stable-sorts the picks by -date; pick keeps
        // "date", so sorting references first yields the same head.
        let mut by_date = self.posts.iter().collect::<Vec<_>>();
        order(&mut by_date, &["-date"]);
        let recent = by_date
            .iter()
            .take(12)
            .map(|p| {
                pick(
                    p,
                    "id thread_key kind agent name session date summary provenance_ok",
                )
            })
            .collect::<Vec<_>>();
        let mut kinds = BTreeMap::<String, i64>::new();
        let mut areas = BTreeSet::<String>::new();
        // Go keys the agent rows by agent + "\x00" + session and walks them sorted.
        let mut agents = BTreeMap::<String, JsonMap>::new();
        let mut without = 0;
        for p in &self.posts {
            let mut kind = str_key(p, "kind");
            if kind.is_empty() {
                kind = "(none)".into();
            }
            *kinds.entry(kind).or_default() += 1;
            areas.insert(str_key(p, "area"));
            without += 1 - num_value(p.get("provenance_ok").unwrap_or(&Value::Null));
            let key = format!("{}\x00{}", str_key(p, "agent"), str_key(p, "session"));
            let row = agents.entry(key).or_insert_with(|| {
                object(
                    "agent session name posts last",
                    vec![
                        p.get("agent").cloned().unwrap_or(Value::Null),
                        p.get("session").cloned().unwrap_or(Value::Null),
                        Value::Null,
                        json!(0),
                        json!(""),
                    ],
                )
            });
            let posts = num_value(row.get("posts").unwrap_or(&Value::Null)) + 1;
            row.insert("posts".into(), json!(posts));
            if str_key(p, "date") > str_key(row, "last") {
                row.insert("last".into(), p.get("date").cloned().unwrap_or(Value::Null));
            }
            let name = str_key(p, "name");
            if !name.is_empty() && name > str_key(row, "name") {
                row.insert("name".into(), p.get("name").cloned().unwrap_or(Value::Null));
            }
        }
        let mut agent_rows = agents.into_values().collect::<Vec<_>>();
        order(&mut agent_rows, &["-last"]);
        agent_rows.truncate(12);
        let mut evo = crate::evolution::evolution_summary(self.evolution.clone());
        if let Some(v) = evo.get("missing_files").cloned() {
            evo.insert(
                "missing_files".into(),
                json!(pulse_store::gojson::to_string(&v).unwrap_or_else(|_| "null".into())),
            );
        }
        object(
            "board_present posts_without_kind quorum claims_active claims_expired threads_open threads_stale threads_closed posts posts_without_provenance suggestions_eligible suggestions_plannable suggestions_needing_votes suggestions_rejected suggestions_expired amendments_open amendments_passed proposals_awaiting proposals_decided active_claims stale_claims open_threads recent_posts evolution kinds areas agents",
            vec![
                json!(str_key(&self.meta, "board_present") == "1"),
                json!(num_value(
                    self.meta.get("posts_without_kind").unwrap_or(&Value::Null)
                )),
                self.params.get("quorum").cloned().unwrap_or(Value::Null),
                json!(active.len()),
                json!(stale.len()),
                json!(open.len()),
                json!(count_status(&self.threads, &["stale"])),
                json!(count_status(&self.threads, &["closed"])),
                json!(self.posts.len()),
                json!(without),
                json!(count_status(&self.suggestions, &["eligible"])),
                json!(count_plannable(&self.suggestions)),
                json!(count_status(&self.suggestions, &["needs votes"])),
                json!(count_status(&self.suggestions, &["rejected"])),
                json!(count_status(&self.suggestions, &["expired"])),
                json!(
                    self.amendments
                        .iter()
                        .filter(|a| !str_key(a, "status").starts_with("closed:"))
                        .count()
                ),
                json!(
                    self.amendments
                        .iter()
                        .filter(|a| str_key(a, "status").starts_with("passed"))
                        .count()
                ),
                json!(count_status(&self.proposals, &["awaiting"])),
                json!(count_status(&self.proposals, &["accepted", "declined"])),
                json!(active),
                json!(stale),
                json!(open),
                json!(recent),
                json!(evo),
                json!(kinds),
                json!(areas.into_iter().collect::<Vec<_>>()),
                json!(agent_rows),
            ],
        )
    }

    pub(crate) fn threads(&self) -> Vec<JsonMap> {
        let mut rows = self.threads.clone();
        order(&mut rows, &["-last"]);
        rows.sort_by_key(|r| match str_key(r, "status").as_str() {
            "open" => 0,
            "stale" => 1,
            "closed" => 2,
            _ => 3,
        });
        rows
    }

    pub(crate) fn notices(&self) -> JsonMap {
        let suggestions = count_status(&self.suggestions, &["needs votes"]);
        let amendments = self
            .amendments
            .iter()
            .filter(|a| !str_key(a, "status").starts_with("closed:"))
            .count() as i64;
        let proposals = count_status(&self.proposals, &["awaiting"]);
        let name = str_key(&self.meta, "repo_name");
        object(
            "ballot votes_open proposals_awaiting brain brains commons_present commons_base commons_questions",
            vec![
                json!([object(
                    "slug base name brain suggestions amendments proposals",
                    vec![
                        json!(name),
                        json!(""),
                        json!(str_key(&self.meta, "repo_name")),
                        json!(false),
                        json!(suggestions),
                        json!(amendments),
                        json!(proposals)
                    ]
                )]),
                json!(suggestions + amendments),
                json!(proposals),
                json!(""),
                json!(Vec::<String>::new()),
                Value::Null,
                json!(""),
                Value::Null,
            ],
        )
    }
}

pub(crate) fn str_value(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

pub(crate) fn num_value(v: &Value) -> i64 {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}

pub(crate) fn object(keys: &str, values: Vec<Value>) -> JsonMap {
    let mut result = JsonMap::new();
    for (key, value) in keys.split_whitespace().zip(values) {
        result.insert(key.to_string(), value);
    }
    result
}

pub(crate) fn pick(row: &JsonMap, keys: &str) -> JsonMap {
    let mut out = JsonMap::new();
    for key in keys.split_whitespace() {
        out.insert(
            key.to_string(),
            row.get(key).cloned().unwrap_or(Value::Null),
        );
    }
    out
}

pub(crate) fn order<R: std::borrow::Borrow<JsonMap>>(rows: &mut [R], fields: &[&str]) {
    // str() of a field without allocating when it already is a string.
    fn key<'a>(row: &'a JsonMap, field: &str) -> std::borrow::Cow<'a, str> {
        match row.get(field) {
            Some(Value::String(s)) => std::borrow::Cow::Borrowed(s.as_str()),
            Some(v) => std::borrow::Cow::Owned(str_value(v)),
            None => std::borrow::Cow::Borrowed(""),
        }
    }
    rows.sort_by(|a, b| {
        let (a, b) = (a.borrow(), b.borrow());
        for field in fields {
            let desc = field.starts_with('-');
            let field = field.trim_start_matches('-');
            let av = key(a, field);
            let bv = key(b, field);
            if av != bv {
                return if desc { bv.cmp(&av) } else { av.cmp(&bv) };
            }
        }
        std::cmp::Ordering::Equal
    });
}

/// Go: exec.CommandContext(5s) + Output(): stdout is read while the command
/// runs, so a long listing never fills the pipe and stalls git, and a command
/// still running at the deadline is killed. Any failure reads as "" - an
/// unborn checkout has no branch, a missing remote is normal.
pub(crate) fn git_value(repo: &str, args: &[&str]) -> String {
    let mut command = Command::new(pulse_gitenv::git_program());
    command.arg("-C").arg(repo).args(args);
    command.env_clear();
    for kv in pulse_gitenv::env() {
        if let Some((k, v)) = kv.split_once('=') {
            command.env(k, v);
        }
    }
    // Output() with a nil Stderr captures the child's stderr into the error;
    // nothing here reads it, so it must not reach the hub's own log either.
    let Ok(mut child) = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return String::new();
    };
    let Some(mut stdout) = child.stdout.take() else {
        return String::new();
    };
    let reader = std::thread::Builder::new()
        .name("pulse-hub-git".into())
        .spawn(move || {
            let mut out = Vec::new();
            std::io::Read::read_to_end(&mut stdout, &mut out).map(|_| out)
        });
    let start = std::time::Instant::now();
    let mut nap = std::time::Duration::from_micros(250);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if start.elapsed() >= std::time::Duration::from_secs(5) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => {
                std::thread::sleep(nap);
                nap = (nap * 2).min(std::time::Duration::from_millis(10));
            }
            Err(_) => break None,
        }
    };
    let out = match reader.map(|r| r.join()) {
        Ok(Ok(Ok(out))) => out,
        _ => return String::new(),
    };
    match status {
        Some(status) if status.success() => String::from_utf8_lossy(&out).trim().to_string(),
        _ => String::new(),
    }
}

pub(crate) fn present(p: &pulse_store::Project) -> bool {
    Path::new(&p.path).is_dir()
        && git_value(&p.path, &["rev-parse", "--is-inside-work-tree"]) == "true"
}

pub(crate) fn truncate(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

pub(crate) fn post_frontmatter(p: &pulse_tally::Post) -> JsonMap {
    let mut fm = object(
        "kind agent runner model session date branch summary",
        vec![
            json!(p.kind),
            json!(p.agent),
            json!(p.runner),
            json!(p.model),
            json!(p.session),
            json!(p.date),
            json!(p.branch),
            json!(p.summary),
        ],
    );
    for (key, value) in [
        ("expires", &p.expires),
        ("on", &p.on_post),
        ("vote", &p.vote),
        ("resolution", &p.resolution),
        ("text_hash", &p.text_hash),
        ("links", &p.links),
        ("files", &p.files),
        ("name", &p.name),
        ("project", &p.origin_project),
    ] {
        if !value.is_empty() {
            fm.insert(key.into(), json!(value));
        }
    }
    if p.for_owner {
        fm.insert("for_owner".into(), json!("true"));
    }
    fm.insert("id".into(), json!(p.id));
    fm.insert("area".into(), json!(p.area));
    fm.insert("thread".into(), json!(p.thread));
    fm.insert("path".into(), json!(format!("store:{}", p.id)));
    fm
}

static PROPOSAL_HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\W*proposal for the human").unwrap());
static PROPOSAL_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\W*proposal for the human[^:]*:\W*").unwrap());

pub(crate) fn proposal_text(body: &str) -> String {
    let mut lines = body.split('\n').collect::<Vec<_>>();
    let mut start = None;
    for (i, line) in lines.iter().enumerate() {
        if PROPOSAL_HEADING.is_match(line.trim()) {
            start = Some(i);
            break;
        }
    }
    let mut para = Vec::new();
    if let Some(i) = start {
        let first = PROPOSAL_PREFIX
            .replace(line_trim(lines[i]), "")
            .trim()
            .to_string();
        if !first.is_empty() {
            para.push(first);
        }
        lines = lines[i + 1..].to_vec();
    }
    for line in lines {
        let line = line.trim();
        if start.is_none() && line.starts_with('#') {
            continue;
        }
        if line.is_empty() {
            if !para.is_empty() {
                break;
            }
            continue;
        }
        para.push(line.to_string());
    }
    let mut text = para.join(" ");
    if start.is_some() {
        text = text.trim().trim_matches(['*', ' ']).trim().to_string();
    }
    truncate(&text, 600)
}

fn line_trim(s: &str) -> &str {
    s.trim()
}

pub(crate) fn later_hub_kind(posts: &[&JsonMap], index: usize, kinds: &[&str]) -> bool {
    let want = kinds.iter().copied().collect::<BTreeSet<_>>();
    posts[index + 1..]
        .iter()
        .any(|post| want.contains(str_key(post, "kind").as_str()))
}

pub(crate) fn later_hub_answer_date(posts: &[&JsonMap], index: usize) -> bool {
    let date = str_key(posts[index], "date");
    posts[index + 1..]
        .iter()
        .any(|post| str_key(post, "kind") == "answer" && str_key(post, "date") > date)
}

/// The posts of a project the owner (or the lead on the owner's behalf) has
/// already decided; a decided question no longer waits for anyone.
pub(crate) fn decided_posts(store: &pulse_store::Store, project: &str) -> Result<BTreeSet<String>> {
    let conn = store.conn()?;
    let mut stmt = conn.prepare("SELECT post_id FROM decisions WHERE project = ?")?;
    let rows = stmt.query_map([project], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<std::result::Result<BTreeSet<_>, _>>()?)
}

pub(crate) fn count_status(rows: &[JsonMap], statuses: &[&str]) -> i64 {
    rows.iter()
        .filter(|row| {
            statuses
                .iter()
                .any(|status| str_key(row, "status") == *status)
        })
        .count() as i64
}

pub(crate) fn count_plannable(rows: &[JsonMap]) -> i64 {
    rows.iter()
        .filter(|row| {
            row.get("plannable")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .count() as i64
}

pub(crate) fn sorted_keys<V>(m: &BTreeMap<String, V>) -> Vec<String> {
    m.keys().cloned().collect()
}

fn vals(values: &[&str]) -> Vec<Value> {
    values.iter().map(|v| json!(*v)).collect()
}

fn num_str(s: &str) -> i64 {
    s.parse().unwrap_or(0)
}

fn str_key(row: &JsonMap, key: &str) -> String {
    row.get(key).map(str_value).unwrap_or_default()
}

fn inc(row: &mut JsonMap, key: &str) {
    let n = num_value(row.get(key).unwrap_or(&Value::Null)) + 1;
    row.insert(key.to_string(), json!(n));
}

fn suggestion_priority(status: &str) -> i32 {
    match status {
        "eligible" => 0,
        "needs votes" => 1,
        "rejected" => 2,
        "expired" => 3,
        _ => 3,
    }
}

fn suggestion_cmp(a: &JsonMap, b: &JsonMap) -> std::cmp::Ordering {
    suggestion_priority(&str_key(a, "status"))
        .cmp(&suggestion_priority(&str_key(b, "status")))
        .then_with(|| {
            num_value(b.get("score").unwrap_or(&Value::Null))
                .cmp(&num_value(a.get("score").unwrap_or(&Value::Null)))
        })
        .then_with(|| str_key(a, "date").cmp(&str_key(b, "date")))
        .then_with(|| str_key(a, "id").cmp(&str_key(b, "id")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers_f2 as f2;
    use serde_json::json;
    use std::process::Command;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .arg("init")
            .arg(dir.path())
            .output()
            .unwrap();
        dir
    }

    // Output() reads stdout while git runs; the earlier poll-then-read loop
    // hung on any listing longer than the pipe buffer until its 5 s kill and
    // then returned "" for a command that had succeeded.
    #[test]
    fn git_value_reads_output_longer_than_a_pipe_buffer() {
        let dir = repo();
        let repo = dir.path().to_str().unwrap();
        for i in 0..3000 {
            std::fs::write(
                dir.path().join(format!("untracked-file-number-{i:05}.txt")),
                "x",
            )
            .unwrap();
        }
        let start = std::time::Instant::now();
        let listing = git_value(repo, &["status", "--porcelain", "--untracked-files=all"]);
        assert!(start.elapsed() < std::time::Duration::from_secs(4));
        assert!(listing.len() > 64 * 1024, "{} bytes", listing.len());
        assert_eq!(listing.lines().count(), 3000);
        assert_eq!(
            git_value(repo, &["rev-parse", "--is-inside-work-tree"]),
            "true"
        );
        // An unborn branch has no HEAD: git fails on stderr, the value is "".
        assert_eq!(git_value(repo, &["rev-parse", "--short", "HEAD"]), "");
    }

    fn join(store: &pulse_store::Store, name: &str) -> pulse_store::Agent {
        pulse_board::join_agent(
            store,
            pulse_board::JoinInput {
                name: name.to_string(),
                runner: "test".to_string(),
                model: "test".to_string(),
                session: format!("s-{name}"),
                ..Default::default()
            },
        )
        .unwrap()
        .0
    }

    #[test]
    fn store_suggestion_vote_reason_uses_summary() {
        let _serial = crate::test_helpers::serial();
        for (vote, summary) in [
            ("up", "up: fewer retries"),
            ("down", "down: no measured improvement"),
            ("up", "up"),
            ("down", "down"),
        ] {
            let s = f2::additions_store(false);
            f2::additions_project(&s, "one");
            join(&s, "proposer");
            join(&s, "voter");
            let suggestion = pulse_board::create_post(
                &s,
                pulse_board::PostInput {
                    kind: "suggestion".into(),
                    project: "one".into(),
                    area: "governance".into(),
                    thread: "vote-reasons".into(),
                    agent: "proposer".into(),
                    session: "s-proposer".into(),
                    summary: "Show why agents voted.".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            let vote_post = pulse_board::create_post(
                &s,
                pulse_board::PostInput {
                    kind: "vote".into(),
                    project: "one".into(),
                    area: "governance".into(),
                    thread: "vote-reasons".into(),
                    agent: "voter".into(),
                    session: "s-voter".into(),
                    on: suggestion.id.clone(),
                    vote: vote.into(),
                    summary: summary.into(),
                    body: "## Vote\n\nBody detail is not the displayed reason.\n".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            let data = f2::handler(&s).snapshot("one", "").unwrap();
            assert_eq!(data.suggestions.len(), 1);
            assert_eq!(str_key(&data.suggestions[0], "id"), suggestion.id);
            let votes = data.suggestions[0]["votes"].as_array().unwrap();
            assert_eq!(votes.len(), 1);
            assert_eq!(votes[0]["post_id"], vote_post.id);
            assert_eq!(votes[0]["vote"], vote);
            assert_eq!(votes[0]["classification"], "counted");
            assert_eq!(votes[0]["reason"], summary);
        }
    }

    #[test]
    fn store_amendment_vote_reason_uses_summary() {
        let _serial = crate::test_helpers::serial();
        let s = f2::additions_store(false);
        f2::additions_project(&s, "one");
        f2::additions_record(
            &s,
            "EVO-constitution",
            "one",
            "evolution",
            "constitution",
            "## Workflow\n\nRun one check.\n",
        );
        join(&s, "proposer");
        join(&s, "voter");
        let amendment = pulse_board::create_post(&s, pulse_board::PostInput {
            kind: "amendment".into(),
            project: "one".into(),
            area: "governance".into(),
            thread: "checks".into(),
            agent: "proposer".into(),
            session: "s-proposer".into(),
            summary: "Run both checks before committing.".into(),
            body: "## Current text\n\n```\nRun one check.\n```\n\n## Proposed text\n\n```\nRun both checks.\n```\n".into(),
            ..Default::default()
        }).unwrap();
        let summary = "Both checks catch more regressions.";
        let restatement = "The workflow runs both checks instead of one.";
        let vote = pulse_board::create_post(
            &s,
            pulse_board::PostInput {
                kind: "vote".into(),
                project: "one".into(),
                area: "governance".into(),
                thread: "checks".into(),
                agent: "voter".into(),
                session: "s-voter".into(),
                on: amendment.id.clone(),
                vote: "up".into(),
                text_hash: amendment.text_hash.clone(),
                summary: summary.into(),
                body: format!(
                    "## Vote\n\nBody detail is not the displayed reason.\n\nRead: {restatement}\n"
                ),
                ..Default::default()
            },
        )
        .unwrap();
        let data = f2::handler(&s).snapshot("one", "").unwrap();
        assert_eq!(data.amendments.len(), 1);
        assert_eq!(str_key(&data.amendments[0], "id"), amendment.id);
        let votes = data.amendments[0]["votes"].as_array().unwrap();
        assert_eq!(votes.len(), 1);
        assert_eq!(votes[0]["post_id"], vote.id);
        assert_eq!(votes[0]["classification"], "counted");
        assert_eq!(votes[0]["reason"], summary);
        assert_eq!(votes[0]["read_line"], restatement);
    }

    #[test]
    fn thread_rows_carry_enriched_status() {
        let _serial = crate::test_helpers::serial();
        let s = f2::additions_store(false);
        f2::additions_project(&s, "one");
        join(&s, "alpha");
        join(&s, "bravo");
        let question = pulse_board::create_post(
            &s,
            pulse_board::PostInput {
                kind: "question".into(),
                project: "one".into(),
                area: "api".into(),
                thread: "rate-limits".into(),
                agent: "alpha".into(),
                summary: "Why 429?".into(),
                date: "2026-09-06T08:00:00Z".into(),
                ..Default::default()
            },
        )
        .unwrap();
        pulse_board::create_post(
            &s,
            pulse_board::PostInput {
                kind: "handoff".into(),
                project: "one".into(),
                area: "api".into(),
                thread: "rate-limits".into(),
                agent: "alpha".into(),
                summary: "Please take it".into(),
                date: "2026-09-06T08:01:00Z".into(),
                ..Default::default()
            },
        )
        .unwrap();
        pulse_board::create_post(
            &s,
            pulse_board::PostInput {
                kind: "claim".into(),
                project: "one".into(),
                area: "api".into(),
                thread: "rate-limits".into(),
                agent: "bravo".into(),
                summary: "On it".into(),
                expires: "2026-09-06T13:00:00Z".into(),
                date: "2026-09-06T08:02:00Z".into(),
                ..Default::default()
            },
        )
        .unwrap();
        pulse_board::create_post(
            &s,
            pulse_board::PostInput {
                kind: "answer".into(),
                project: "one".into(),
                area: "api".into(),
                thread: "answered".into(),
                agent: "bravo".into(),
                summary: "Named answer".into(),
                on: question.id,
                date: "2026-09-06T08:03:00Z".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let data = f2::handler(&s).snapshot("one", "").unwrap();
        let thread = data.thread("api/rate-limits")["thread"]
            .as_object()
            .unwrap()
            .clone();
        let participants = thread["participants"]
            .as_array()
            .unwrap()
            .iter()
            .map(str_value)
            .collect::<Vec<_>>();
        assert_eq!(participants.join(","), "alpha,bravo");
        assert_eq!(thread["last_by"], "bravo");
        assert_eq!(thread["last_kind"], "claim");
        assert_eq!(thread["open_questions"], 0);
        assert_eq!(thread["open_handoffs"], 0);
        assert_eq!(thread["active_claims"].as_array().unwrap()[0], "bravo");
    }

    #[test]
    fn suggestion_rows_carry_jobs_and_plan_gates() {
        let _serial = crate::test_helpers::serial();
        let s = f2::additions_store(false);
        let repo = repo();
        let project = pulse_board::add_project(
            &s,
            repo.path().to_str().unwrap(),
            "One",
            "",
            false,
            "",
            "human",
            "one",
        )
        .unwrap();
        let agents = ["alpha", "bravo", "charlie", "delta"]
            .into_iter()
            .map(|n| (n.to_string(), join(&s, n)))
            .collect::<std::collections::BTreeMap<_, _>>();
        let suggest = |agent: &str, thread: &str, summary: &str, for_owner: bool| {
            pulse_board::create_post(
                &s,
                pulse_board::PostInput {
                    kind: "suggestion".into(),
                    project: project.slug.clone(),
                    area: "product".into(),
                    thread: thread.into(),
                    agent_row: Some(agents[agent].clone()),
                    summary: summary.into(),
                    body: "Suggestion body.".into(),
                    for_owner,
                    ..Default::default()
                },
            )
            .unwrap()
        };
        let vote = |agent: &str, area: &str, thread: &str, on_id: &str, direction: &str| {
            pulse_board::create_post(
                &s,
                pulse_board::PostInput {
                    kind: "vote".into(),
                    project: project.slug.clone(),
                    area: area.into(),
                    thread: thread.into(),
                    agent_row: Some(agents[agent].clone()),
                    on: on_id.into(),
                    vote: direction.into(),
                    summary: format!("Voting {direction}"),
                    body: "Vote body.".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        };
        let net_zero = suggest("alpha", "net-zero", "Use offset scans for exports", false);
        vote("bravo", "product", "net-zero", &net_zero.id, "up");
        vote("charlie", "product", "net-zero", &net_zero.id, "down");
        let plannable = suggest("bravo", "owner-cache", "Release candidate cache fix", false);
        vote("alpha", "product", "owner-cache", &plannable.id, "up");
        vote("charlie", "product", "owner-cache", &plannable.id, "up");
        let blocked = suggest(
            "charlie",
            "blocked-work",
            "Add retry budget to API calls",
            false,
        );
        vote("alpha", "product", "blocked-work", &blocked.id, "up");
        vote("bravo", "product", "blocked-work", &blocked.id, "up");
        let owner = suggest("delta", "support-window", "Choose the support window", true);
        vote("alpha", "product", "support-window", &owner.id, "up");
        vote("bravo", "product", "support-window", &owner.id, "up");
        let job = pulse_board::add_job(
            &s,
            pulse_board::JobInput {
                project: project.slug.clone(),
                kind: "task".into(),
                title: "Carry blocked suggestion".into(),
                source: format!("suggestion:{}", blocked.id),
                created_by: "human".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .0;
        s.conn()
            .unwrap()
            .execute(
                "UPDATE jobs SET status='blocked' WHERE id=?",
                [job.id.as_str()],
            )
            .unwrap();
        let data = f2::handler(&s).snapshot(&project.slug, "").unwrap();
        let by_id = data
            .suggestions
            .iter()
            .map(|r| (str_key(r, "id"), r.clone()))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(by_id[&net_zero.id]["status"], "needs votes");
        assert_eq!(by_id[&plannable.id]["plannable"], true);
        assert_eq!(by_id[&blocked.id]["job"], job.id);
        assert_eq!(by_id[&blocked.id]["job_status"], "blocked");
        assert_eq!(
            by_id[&blocked.id]["gate"],
            format!("blocked as job {}", job.id)
        );
        assert_eq!(by_id[&owner.id]["gate"], pulse_tally::GATE_OWNER);
        assert_eq!(data.overview()["suggestions_plannable"], 1);
    }

    #[test]
    fn owner_questions_wait_until_decided_answered_or_resolved() {
        let _serial = crate::test_helpers::serial();
        let s = f2::additions_store(false);
        let repo = repo();
        pulse_board::add_project(
            &s,
            repo.path().to_str().unwrap(),
            "One",
            "",
            false,
            "",
            "human",
            "one",
        )
        .unwrap();
        join(&s, "alpha");
        join(&s, "daemon");
        let post = |kind: &str,
                    thread: &str,
                    for_owner: bool,
                    summary: &str,
                    body: &str,
                    on: &str,
                    resolution: &str,
                    date: &str| {
            pulse_board::create_post(
                &s,
                pulse_board::PostInput {
                    kind: kind.into(),
                    project: "one".into(),
                    area: if thread == "loop-gate" || thread.starts_with("job-") {
                        "evolution".into()
                    } else {
                        "api".into()
                    },
                    thread: thread.into(),
                    agent: if summary == pulse_store::EVOLUTION_STOP_SUMMARY {
                        "daemon".into()
                    } else {
                        "alpha".into()
                    },
                    for_owner,
                    summary: summary.into(),
                    body: body.into(),
                    on: on.into(),
                    resolution: resolution.into(),
                    date: date.into(),
                    ..Default::default()
                },
            )
            .unwrap()
        };
        let stop = pulse_store::EVOLUTION_STOP_SUMMARY;
        // A worker question is not the owner's.
        post(
            "question",
            "plain",
            false,
            "Why 429?",
            "",
            "",
            "",
            "2026-09-06T08:00:00Z",
        );
        // A marked question waits for the owner.
        let ask = post(
            "question",
            "owner",
            true,
            "Which provider pays?",
            "Pick one.",
            "",
            "",
            "2026-09-06T08:01:00Z",
        );
        // A marked question that got its answer does not.
        let done = post(
            "question",
            "settled",
            true,
            "Keep the old name?",
            "",
            "",
            "",
            "2026-09-06T08:02:00Z",
        );
        post(
            "answer",
            "settled",
            false,
            "Yes, keep it",
            "",
            &done.id,
            "",
            "2026-09-06T08:03:00Z",
        );
        // The loop-gate thread: an old decided stop closed it; the new stop still waits.
        let old = post(
            "question",
            "loop-gate",
            true,
            stop,
            "an instruction-shaped post was detected",
            "",
            "",
            "2026-09-06T08:04:00Z",
        );
        pulse_board::decide(&s, &old.id, "yes", "not a command", "human").unwrap();
        // The decision's result post is dated now; the new stop comes after it.
        let after = (chrono::DateTime::parse_from_rfc3339(&s.now()).unwrap()
            + chrono::Duration::seconds(60))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let gate = post(
            "question",
            "loop-gate",
            true,
            stop,
            "experiment job-1 failed with a hard-stop marker\nThe workflow was not bypassed.",
            "",
            "",
            &after,
        );
        // A stop on a suggestion whose thread closed afterwards is moot.
        post(
            "question",
            "job-closed",
            true,
            stop,
            "evolution constraint violation: immutable path",
            "",
            "",
            "2026-09-06T08:07:00Z",
        );
        post(
            "result",
            "job-closed",
            false,
            "Closed",
            "",
            "",
            "dropped",
            "2026-09-06T08:08:00Z",
        );
        // Card-needed asks are the planner's own work since ADR-044.
        post(
            "question",
            "job-card",
            true,
            stop,
            &format!("{}; name exactly one File:", pulse_evolve::CARD_NEEDED),
            "",
            "",
            "2026-09-06T08:09:00Z",
        );

        let h = f2::handler(&s);
        let data = h.snapshot("one", "").unwrap();
        let ids = data
            .questions
            .iter()
            .map(|q| str_key(q, "id"))
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec![gate.id.clone(), ask.id.clone()],
            "{:#?}",
            data.questions
        );
        assert_eq!(data.questions[0]["stop"], true);
        assert_eq!(data.questions[0]["thread_key"], "evolution/loop-gate");
        assert_eq!(data.questions[1]["stop"], false);
        assert_eq!(data.questions[1]["body"], "Pick one.");

        let view = crate::read::ProjectView {
            project: s.project("one").unwrap().unwrap(),
            data: Some(data),
            error: String::new(),
            present: true,
        };
        let summary = h.summary(&view);
        assert_eq!(summary["owner_questions"], 2);
        let org = h.org(std::slice::from_ref(&view));
        assert_eq!(org["totals"]["owner_questions"], 2);
        let rows = org["questions"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["slug"], "one");
        assert_eq!(rows[0]["base"], "/p/one");
        assert!(org["projects"][0].get("questions").is_none());
        let flat = crate::read::org_questions(std::slice::from_ref(&view));
        assert_eq!(flat.len(), 2);
        assert_eq!(str_key(&flat[0], "id"), gate.id);

        let (status, _, raw) = crate::test_helpers_f3::request(
            h.clone_for_test(),
            "GET",
            "/api/v1/projects/one/questions",
            "",
            &[],
        );
        assert_eq!(status, 200, "{raw}");
        let listed: Vec<JsonMap> = serde_json::from_str(&raw).unwrap();
        assert_eq!(listed.len(), 2);

        pulse_board::decide(&s, &ask.id, "yes", "the first one", "human").unwrap();
        pulse_board::decide(&s, &gate.id, "yes", "reviewed", "human").unwrap();
        let data = h.snapshot("one", "").unwrap();
        assert!(data.questions.is_empty(), "{:#?}", data.questions);
    }

    #[test]
    fn org_owner_presence_and_plannable_totals() {
        let _serial = crate::test_helpers::serial();
        let s = f2::additions_store(false);
        let h = f2::handler(&s);
        let evolution =
            json!({"loop":{"loop":"","stop":"","limits":""},"archive":{"rows":[],"record":null}});
        let data = BoardData {
            meta: object(
                "repo_name branch head dirty built_at board_present",
                vec![
                    json!("one"),
                    json!("main"),
                    json!("abc123"),
                    json!("0"),
                    json!(s.now()),
                    json!("1"),
                ],
            ),
            params: JsonMap::new(),
            evolution: serde_json::from_value(evolution).unwrap(),
            suggestions: vec![
                object("status plannable", vec![json!("eligible"), json!(true)]),
                object("status plannable", vec![json!("eligible"), json!(false)]),
                object("status plannable", vec![json!("needs votes"), json!(false)]),
            ],
            ..Default::default()
        };
        let org = h.org(json!([{"project":{"slug":"one"},"data":data.overview()}]));
        assert_eq!(org["owner_presence"], pulse_store::OWNER_PRESENT);
        assert_eq!(org["totals"]["votes_plannable"], 1);
    }
}
