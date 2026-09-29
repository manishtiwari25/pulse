use anyhow::{Result, anyhow};
use regex::Regex;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

pub(crate) type JsonMap = BTreeMap<String, Value>;

pub trait TallySource: Send + Sync {
    fn records(&self, project: &str, area: &str) -> Result<String>;
}

#[allow(dead_code)]
pub(crate) struct NativeTally {
    pub(crate) store: pulse_store::Store,
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
    #[allow(dead_code)]
    pub(crate) project: Option<pulse_store::Project>,
    pub(crate) meta: JsonMap,
    pub(crate) params: JsonMap,
    pub(crate) posts: Vec<JsonMap>,
    pub(crate) by_id: BTreeMap<String, JsonMap>,
    pub(crate) claims: Vec<JsonMap>,
    pub(crate) threads: Vec<JsonMap>,
    pub(crate) proposals: Vec<JsonMap>,
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
            .iter()
            .cloned()
            .map(|p| (p.id.clone(), p))
            .collect::<BTreeMap<_, _>>();
        let mut d = BoardData {
            project: Some(p.clone()),
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
                            json!(post.files), json!(fm), json!(post.body.trim_end_matches('\n')), json!(if missing.is_empty() { 1 } else { 0 }),
                            json!(missing), json!(if area.is_empty() || area == f[1] { 1 } else { 0 }),
                            json!(truncate(&post.name, 40)), json!(truncate(&post.origin_project, 60)),
                        ],
                    );
                    d.by_id.insert(f[3].to_string(), row.clone());
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
        for row in &mut d.proposals {
            let post = d
                .by_id
                .get(&str_key(row, "id"))
                .cloned()
                .unwrap_or_default();
            let decided = d
                .by_id
                .get(&str_key(row, "decided_id"))
                .cloned()
                .unwrap_or_default();
            for key in ["name", "session", "rel_path", "body", "kind"] {
                row.insert(key.to_string(), json!(str_key(&post, key)));
            }
            for key in ["summary", "agent", "name"] {
                row.insert(format!("decided_{key}"), json!(str_key(&decided, key)));
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
            let post = d.by_id.get(&id).cloned().unwrap_or_default();
            row.insert("name".into(), json!(str_key(&post, "name")));
            row.insert("session".into(), json!(str_key(&post, "session")));
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
                let post = d
                    .by_id
                    .get(&str_key(vote, "post_id"))
                    .cloned()
                    .unwrap_or_default();
                vote.insert("reason".into(), json!(str_key(&post, "summary")));
                vote.insert("name".into(), json!(str_key(&post, "name")));
                vote.insert("session".into(), json!(str_key(&post, "session")));
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
            let post = d.by_id.get(&id).cloned().unwrap_or_default();
            row.insert("name".into(), json!(str_key(&post, "name")));
            row.insert("session".into(), json!(str_key(&post, "session")));
            let mut votes = avotes.remove(&id).unwrap_or_default();
            for vote in &mut votes {
                let post = d
                    .by_id
                    .get(&str_key(vote, "post_id"))
                    .cloned()
                    .unwrap_or_default();
                vote.insert("reason".into(), json!(str_key(&post, "summary")));
                vote.insert("name".into(), json!(str_key(&post, "name")));
                let mut read_line = String::new();
                for line in str_key(&post, "body").split('\n') {
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
        let mut posts = self.posts.clone();
        order(&mut posts, &["date", "id"]);
        let mut seen = BTreeMap::<String, BTreeSet<String>>::new();
        let mut by_thread = BTreeMap::<String, Vec<JsonMap>>::new();
        for post in &posts {
            let key = str_key(post, "thread_key");
            let Some(&idx) = threads.get(&key) else {
                continue;
            };
            by_thread.entry(key.clone()).or_default().push(post.clone());
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
            .filter(|p| str_key(p, "kind") == "answer" && !str_key(p, "on_post").is_empty())
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

    pub(crate) fn post(&self, id: &str) -> JsonMap {
        let Some(p) = self.by_id.get(id) else {
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
        let active = self
            .claims
            .iter()
            .filter(|c| str_key(c, "status") == "active")
            .cloned()
            .collect::<Vec<_>>();
        let stale = self
            .claims
            .iter()
            .filter(|c| matches!(str_key(c, "status").as_str(), "expired" | "no expiry"))
            .cloned()
            .collect::<Vec<_>>();
        let mut open = self
            .threads
            .iter()
            .filter(|t| matches!(str_key(t, "status").as_str(), "open" | "stale"))
            .cloned()
            .collect::<Vec<_>>();
        order(&mut open, &["-last"]);
        let mut recent = self
            .posts
            .iter()
            .map(|p| {
                pick(
                    p,
                    "id thread_key kind agent name session date summary provenance_ok",
                )
            })
            .collect::<Vec<_>>();
        order(&mut recent, &["-date"]);
        recent.truncate(12);
        let mut kinds = BTreeMap::<String, i64>::new();
        let mut areas = BTreeSet::<String>::new();
        let mut without = 0;
        for p in &self.posts {
            let mut kind = str_key(p, "kind");
            if kind.is_empty() {
                kind = "(none)".into();
            }
            *kinds.entry(kind).or_default() += 1;
            areas.insert(str_key(p, "area"));
            without += 1 - num_value(p.get("provenance_ok").unwrap_or(&Value::Null));
        }
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
                json!(Vec::<Value>::new()),
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

pub(crate) fn order(rows: &mut [JsonMap], fields: &[&str]) {
    rows.sort_by(|a, b| {
        for field in fields {
            let desc = field.starts_with('-');
            let field = field.trim_start_matches('-');
            let av = str_key(a, field);
            let bv = str_key(b, field);
            if av != bv {
                return if desc { bv.cmp(&av) } else { av.cmp(&bv) };
            }
        }
        std::cmp::Ordering::Equal
    });
}

pub(crate) fn git_value(repo: &str, args: &[&str]) -> String {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo).args(args);
    command.env_clear();
    for kv in pulse_gitenv::env() {
        if let Some((k, v)) = kv.split_once('=') {
            command.env(k, v);
        }
    }
    let Ok(mut child) = command.stdout(std::process::Stdio::piped()).spawn() else {
        return String::new();
    };
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let Ok(out) = child.wait_with_output() else {
                    return String::new();
                };
                if !status.success() {
                    return String::new();
                }
                return String::from_utf8_lossy(&out.stdout).trim().to_string();
            }
            Ok(None) => {
                if start.elapsed() >= std::time::Duration::from_secs(5) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return String::new();
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(_) => return String::new(),
        }
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

pub(crate) fn later_hub_kind(posts: &[JsonMap], index: usize, kinds: &[&str]) -> bool {
    let want = kinds.iter().copied().collect::<BTreeSet<_>>();
    posts[index + 1..]
        .iter()
        .any(|post| want.contains(str_key(post, "kind").as_str()))
}

pub(crate) fn later_hub_answer_date(posts: &[JsonMap], index: usize) -> bool {
    let date = str_key(&posts[index], "date");
    posts[index + 1..]
        .iter()
        .any(|post| str_key(post, "kind") == "answer" && str_key(post, "date") > date)
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
