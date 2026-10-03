use crate::board::{BoardData, object, order, pick, str_value};
use crate::httpx::{Request, ResponseWriter};
use crate::server::Handler;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) type JsonMap = BTreeMap<String, Value>;

#[derive(Debug, Clone, Default)]
pub(crate) struct ProjectView {
    pub(crate) project: pulse_store::Project,
    pub(crate) data: Option<BoardData>,
    pub(crate) error: String,
    pub(crate) present: bool,
}

/// Go returns the response and lets the garbage collector reclaim the snapshot
/// on its own time; freeing a multi-gigabyte view set inline would hold the
/// response until the deallocation finished, so it leaves the request thread.
/// A failed spawn drops the value right here instead.
fn release<T: Send + 'static>(value: T) {
    let _ = std::thread::Builder::new()
        .name("pulse-hub-release".into())
        .spawn(move || drop(value));
}

pub(crate) trait OrgViews<'a> {
    fn into_views(self) -> Cow<'a, [ProjectView]>;
}

impl<'a> OrgViews<'a> for &'a [ProjectView] {
    fn into_views(self) -> Cow<'a, [ProjectView]> {
        Cow::Borrowed(self)
    }
}

impl<'a> OrgViews<'a> for &'a Vec<ProjectView> {
    fn into_views(self) -> Cow<'a, [ProjectView]> {
        Cow::Borrowed(self.as_slice())
    }
}

impl OrgViews<'static> for Value {
    fn into_views(self) -> Cow<'static, [ProjectView]> {
        self.as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|v| {
                let project = v.get("project").cloned().unwrap_or(Value::Null);
                let data = v.get("data").cloned().and_then(|data| {
                    data.as_object().map(|obj| {
                        let mut summary = obj.clone().into_iter().collect::<JsonMap>();
                        let suggestions_plannable = summary
                            .get("suggestions_plannable")
                            .and_then(Value::as_i64)
                            .unwrap_or(0);
                        let suggestions_needing_votes = summary
                            .get("suggestions_needing_votes")
                            .and_then(Value::as_i64)
                            .unwrap_or(0);
                        let amendments_open = summary
                            .get("amendments_open")
                            .and_then(Value::as_i64)
                            .unwrap_or(0);
                        let proposals_awaiting = summary
                            .get("proposals_awaiting")
                            .and_then(Value::as_i64)
                            .unwrap_or(0);
                        BoardData {
                            suggestions: (0..suggestions_plannable)
                                .map(|_| {
                                    object("status plannable", vec![json!("eligible"), json!(true)])
                                })
                                .chain((0..suggestions_needing_votes).map(|_| {
                                    object(
                                        "status plannable",
                                        vec![json!("needs votes"), json!(false)],
                                    )
                                }))
                                .collect(),
                            amendments: (0..amendments_open)
                                .map(|_| object("status", vec![json!("open")]))
                                .collect(),
                            proposals: (0..proposals_awaiting)
                                .map(|_| object("status", vec![json!("awaiting")]))
                                .collect(),
                            evolution: serde_json::from_value(
                                summary.remove("evolution").unwrap_or(Value::Null),
                            )
                            .unwrap_or_default(),
                            meta: summary,
                            ..Default::default()
                        }
                    })
                });
                ProjectView {
                    project: pulse_store::Project {
                        slug: project
                            .get("slug")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        name: project
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        path: project
                            .get("path")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        brain: project
                            .get("brain")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        ..Default::default()
                    },
                    present: v.get("present").and_then(Value::as_bool).unwrap_or(true),
                    error: v
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    data,
                }
            })
            .collect::<Vec<_>>()
            .into()
    }
}

impl Handler {
    pub(crate) fn project_api(
        &self,
        w: &mut ResponseWriter,
        r: &Request,
        slug: String,
        route: String,
    ) {
        if !crate::server::read_method(w, r) {
            return;
        }
        let p = match self.entry(&slug) {
            Ok(p) => p,
            Err(err) => {
                let code = if err.downcast_ref::<pulse_tally::NoProjectError>().is_some() {
                    404
                } else {
                    503
                };
                crate::server::write_error(
                    w,
                    code,
                    "no such project".into(),
                    err.to_string(),
                    "Run pulse projects.".into(),
                );
                return;
            }
        };
        if !crate::board::present(&p) {
            crate::server::write_error(
                w,
                503,
                "registered but missing on disk".into(),
                "The project path must be a directory in a Git checkout.".into(),
                "Restore the registered checkout or update the project path.".into(),
            );
            return;
        }
        if route == "health" {
            crate::server::write_json(
                w,
                200,
                json!(object(
                    "ok time last_error",
                    vec![json!(true), json!(store_now(self)), json!("")]
                )),
            );
            return;
        }
        if route == "settings" {
            let data = self.settings();
            self.respond_result(w, data);
            return;
        }
        if route != "project"
            && route != "refresh"
            && route != "overview"
            && route != "threads"
            && route != "suggestions"
            && route != "amendments"
            && route != "evolution"
            && route != "timeline"
            && route != "notices"
            && route != "proposals"
            && route != "questions"
            && !route.starts_with("threads/")
            && !route.starts_with("posts/")
        {
            crate::server::write_error(
                w,
                404,
                "not found".into(),
                "No such project API route.".into(),
                "Use the API v1 project routes.".into(),
            );
            return;
        }
        let d = match self.snapshot(&slug, &r.query("area")) {
            Ok(d) => d,
            Err(err) => {
                self.respond(w, Value::Null, err.to_string());
                return;
            }
        };
        let value = match route.as_str() {
            "project" | "refresh" => json!(d.meta),
            "overview" => json!(d.overview()),
            "threads" => json!(d.threads()),
            r if r.starts_with("threads/") => {
                let row = d.thread(r.trim_start_matches("threads/"));
                if row.is_empty() {
                    crate::server::write_error(
                        w,
                        404,
                        "no such thread".into(),
                        "That thread is not in this project.".into(),
                        "Refresh the thread list.".into(),
                    );
                    return;
                }
                json!(row)
            }
            r if r.starts_with("posts/") => {
                let row = d.post(r.trim_start_matches("posts/"));
                if row.is_empty() {
                    crate::server::write_error(
                        w,
                        404,
                        "no such post".into(),
                        "That post is not in this project.".into(),
                        "Refresh the thread.".into(),
                    );
                    return;
                }
                json!(row)
            }
            "suggestions" => json!(d.suggestions),
            "amendments" => json!(d.amendments),
            "proposals" => json!(d.proposals),
            "questions" => json!(d.questions),
            "notices" => json!(d.notices()),
            "timeline" => json!(timeline(&d.posts, str_map(&d.params, "now"))),
            "evolution" => json!(d.evolution),
            _ => Value::Null,
        };
        crate::server::write_json(w, 200, value);
    }

    pub(crate) fn respond(&self, w: &mut ResponseWriter, data: Value, err: String) {
        if !err.is_empty() {
            crate::server::write_error(
                w,
                503,
                "The store view is unavailable".into(),
                err,
                "Check pulse status and retry.".into(),
            );
            return;
        }
        crate::server::write_json(w, 200, data);
    }

    pub(crate) fn respond_result<T: serde::Serialize>(
        &self,
        w: &mut ResponseWriter,
        data: Result<T>,
    ) {
        match data {
            Ok(data) => crate::server::write_json(w, 200, go_value(&data)),
            Err(err) => self.respond(w, Value::Null, err.to_string()),
        }
    }

    pub(crate) fn each(&self) -> Result<Vec<ProjectView>> {
        let store = self
            .store
            .as_deref()
            .ok_or_else(|| anyhow!("the store is unavailable"))?;
        let mut projects = store.projects(true)?;
        projects.push(self.entry(pulse_store::COMMONS_SLUG)?);
        let out = std::thread::scope(|scope| {
            let handles = projects
                .into_iter()
                .enumerate()
                .map(|(i, p)| {
                    scope.spawn(move || {
                        let present = crate::board::present(&p);
                        let mut view = ProjectView {
                            project: p.clone(),
                            present,
                            ..Default::default()
                        };
                        if present {
                            match self.snapshot(&p.slug, "") {
                                Ok(data) => view.data = Some(data),
                                Err(err) => view.error = err.to_string(),
                            }
                        } else if p.slug == pulse_store::COMMONS_SLUG {
                            view.error = "not created yet".into();
                        } else {
                            view.error = "missing on disk".into();
                        }
                        (i, view)
                    })
                })
                .collect::<Vec<_>>();
            let mut out = handles
                .into_iter()
                .map(|h| h.join().expect("project view probe panicked"))
                .collect::<Vec<_>>();
            out.sort_by_key(|(i, _)| *i);
            out.into_iter().map(|(_, view)| view).collect::<Vec<_>>()
        });
        Ok(out)
    }

    pub(crate) fn org_api(&self, w: &mut ResponseWriter, r: &Request, route: String) {
        if route == "settings" {
            self.respond_result(w, self.settings());
            return;
        }
        if route == "executors" {
            self.respond_result(w, self.executors(r));
            return;
        }
        if route == "runner" {
            self.respond_result(w, self.runner(r));
            return;
        }
        match route.as_str() {
            "health" | "org" | "notices" | "proposals" | "questions" | "roster" | "votes"
            | "activity" | "timeline" => {}
            _ => {
                crate::server::write_error(
                    w,
                    404,
                    "not found".into(),
                    "No such organization API route.".into(),
                    "Use /api/v1/org.".into(),
                );
                return;
            }
        }
        if route == "health" {
            let Some(store) = self.store.as_deref() else {
                crate::server::write_error(
                    w,
                    503,
                    "The store is unavailable".into(),
                    "No PULSE store is open.".into(),
                    "Run pulse init.".into(),
                );
                return;
            };
            match store.projects(true) {
                Ok(projects) => {
                    let mut slugs = vec![pulse_store::COMMONS_SLUG.to_string()];
                    slugs.extend(projects.into_iter().map(|p| p.slug));
                    slugs.sort();
                    crate::server::write_json(
                        w,
                        200,
                        json!(object(
                            "ok time projects errors",
                            vec![json!(true), json!(store.now()), json!(slugs), json!({})]
                        )),
                    );
                }
                Err(err) => self.respond(w, Value::Null, err.to_string()),
            }
            return;
        }
        if route == "org" {
            let cache = self.org_cache.lock().expect("org cache lock poisoned");
            let cache_at = self
                .org_cache_at
                .lock()
                .expect("org cache time lock poisoned");
            if !cache.is_empty()
                && cache_at
                    .as_ref()
                    .and_then(|at| at.elapsed().ok())
                    .is_some_and(|d| d < self.org_cache_ttl)
            {
                w.header_mut().insert("x-org-cache", "hit".parse().unwrap());
                w.header_mut().insert(
                    hyper::header::CONTENT_TYPE,
                    "application/json; charset=utf-8".parse().unwrap(),
                );
                w.header_mut()
                    .insert(hyper::header::CACHE_CONTROL, "no-store".parse().unwrap());
                let _ = w.write(&cache);
                return;
            }
        }
        let views = match self.each() {
            Ok(v) => v,
            Err(err) => {
                self.respond(w, Value::Null, err.to_string());
                return;
            }
        };
        let data = match route.as_str() {
            "org" => {
                let mut org = self.org(&views);
                release(views);
                match self.metadata() {
                    Ok(metadata) => org.extend(metadata),
                    Err(err) => {
                        self.respond(w, Value::Null, err.to_string());
                        return;
                    }
                }
                w.header_mut()
                    .insert("x-org-cache", "miss".parse().unwrap());
                let buf = match pulse_store::gojson::to_vec(&org) {
                    Ok(v) => v,
                    Err(err) => {
                        self.respond(w, Value::Null, err.to_string());
                        return;
                    }
                };
                {
                    let mut cache = self.org_cache.lock().expect("org cache lock poisoned");
                    let mut cache_at = self
                        .org_cache_at
                        .lock()
                        .expect("org cache time lock poisoned");
                    *cache = buf.clone();
                    *cache_at = Some(std::time::SystemTime::now());
                }
                w.header_mut().insert(
                    hyper::header::CONTENT_TYPE,
                    "application/json; charset=utf-8".parse().unwrap(),
                );
                w.header_mut()
                    .insert(hyper::header::CACHE_CONTROL, "no-store".parse().unwrap());
                w.write_header(200);
                let _ = w.write(&buf);
                return;
            }
            "notices" => self
                .org(&views)
                .get("notices")
                .cloned()
                .unwrap_or(Value::Null),
            "proposals" => json!(org_proposals(&views)),
            "questions" => json!(org_questions(&views)),
            "votes" => json!(org_votes(&views)),
            "activity" => json!(org_activity(&views, 150)),
            "timeline" => json!(org_timeline(&views, store_now(self))),
            "roster" => match self.roster(&views) {
                Ok(v) => json!(v),
                Err(err) => {
                    self.respond(w, Value::Null, err.to_string());
                    return;
                }
            },
            _ => Value::Null,
        };
        release(views);
        self.respond(w, data, String::new());
    }

    pub(crate) fn summary(&self, view: &ProjectView) -> JsonMap {
        let p = &view.project;
        let kind = if p.slug == pulse_store::COMMONS_SLUG {
            "commons"
        } else {
            "project"
        };
        let Some(d) = &view.data else {
            return object(
                "slug kind brain base path present missing error",
                vec![
                    json!(p.slug),
                    json!(kind),
                    json!(p.brain),
                    json!(base_for(&p.slug)),
                    json!(p.path),
                    json!(false),
                    json!(!view.present),
                    json!(view.error),
                ],
            );
        };
        let overview = d.overview();
        let evo = crate::evolution::evolution_summary(d.evolution.clone());
        // Go copies the post slice and stable-sorts it by -date; the head of
        // that order is the first post carrying the greatest date.
        let last = d
            .posts
            .iter()
            .min_by(|a, b| str_map(b, "date").cmp(&str_map(a, "date")))
            .map(|p| {
                json!(pick(
                    p,
                    "id thread_key kind agent name session date summary"
                ))
            })
            .unwrap_or(Value::Null);
        let day_ago = parse_dt(&store_now(self))
            .map(|t| {
                (t - chrono::Duration::hours(24)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            })
            .unwrap_or_default();
        let mut posts24 = 0;
        let mut identities = BTreeMap::new();
        let mut questions = BTreeSet::new();
        for post in &d.posts {
            identities.insert(str_map(post, "agent"), true);
            if str_map(post, "date") >= day_ago {
                posts24 += 1;
            }
            if post.get("kind").and_then(Value::as_str) == Some("question") {
                questions.insert(str_map(post, "thread_key"));
            }
        }
        let questions_open = d
            .threads
            .iter()
            .filter(|t| {
                matches!(str_map(t, "status").as_str(), "open" | "stale")
                    && questions.contains(&str_map(t, "key"))
            })
            .count();
        let claims = overview
            .get("active_claims")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| v.as_object().cloned())
            .map(|m| {
                pick(
                    &m.into_iter().collect(),
                    "id thread_key agent expires summary name session",
                )
            })
            .collect::<Vec<_>>();
        let proposals = d
            .proposals
            .iter()
            .filter(|row| row.get("status").and_then(Value::as_str) == Some("awaiting"))
            .map(|row| {
                pick(
                    row,
                    "id thread_key agent name session date status summary proposal_text rel_path",
                )
            })
            .collect::<Vec<_>>();
        object(
            "slug kind brain base path remote name branch head dirty built_at board_present claims_active threads_open questions_open owner_questions suggestions_eligible suggestions_plannable suggestions_needing_votes suggestions_rejected amendments_open proposals_awaiting posts posts_24h tally_mode can_decide agents identities evolution_present loop stop_present baseline last_post active_claims proposals questions present missing error",
            vec![
                json!(p.slug),
                json!(kind),
                json!(p.brain),
                json!(base_for(&p.slug)),
                json!(p.path),
                json!(p.remote),
                d.meta.get("repo_name").cloned().unwrap_or(Value::Null),
                d.meta.get("branch").cloned().unwrap_or(Value::Null),
                d.meta.get("head").cloned().unwrap_or(Value::Null),
                json!(str_map(&d.meta, "dirty") == "1"),
                d.meta.get("built_at").cloned().unwrap_or(Value::Null),
                json!(str_map(&d.meta, "board_present") == "1"),
                overview
                    .get("claims_active")
                    .cloned()
                    .unwrap_or(Value::Null),
                overview.get("threads_open").cloned().unwrap_or(Value::Null),
                json!(questions_open),
                json!(d.questions.len()),
                overview
                    .get("suggestions_eligible")
                    .cloned()
                    .unwrap_or(Value::Null),
                overview
                    .get("suggestions_plannable")
                    .cloned()
                    .unwrap_or(Value::Null),
                overview
                    .get("suggestions_needing_votes")
                    .cloned()
                    .unwrap_or(Value::Null),
                overview
                    .get("suggestions_rejected")
                    .cloned()
                    .unwrap_or(Value::Null),
                overview
                    .get("amendments_open")
                    .cloned()
                    .unwrap_or(Value::Null),
                overview
                    .get("proposals_awaiting")
                    .cloned()
                    .unwrap_or(Value::Null),
                json!(d.posts.len()),
                json!(posts24),
                json!("go"),
                json!(true),
                json!(identities.len()),
                json!(crate::board::sorted_keys(&identities)),
                json!(str_map(&evo, "evolution_present") == "1"),
                evo.get("loop").cloned().unwrap_or(Value::Null),
                json!(str_map(&evo, "stop_present") == "1"),
                evo.get("baseline").cloned().unwrap_or(Value::Null),
                last,
                json!(claims),
                json!(proposals),
                json!(d.questions),
                json!(true),
                json!(false),
                json!(view.error),
            ],
        )
    }

    pub(crate) fn org<'a, V: OrgViews<'a>>(&self, views: V) -> JsonMap {
        let views = views.into_views();
        let views = views.as_ref();
        // Go's projects, commons and all alias the same maps, so the
        // identities/proposals deletes below show in the emitted rows too.
        let mut project_idx = Vec::new();
        let mut commons_idx = None;
        let mut all = Vec::new();
        let mut brains = Vec::<String>::new();
        let mut stamps = Vec::<String>::new();
        for view in views {
            let summary = self.summary(view);
            stamps.push(str_map(&summary, "built_at"));
            if view.project.slug == pulse_store::COMMONS_SLUG {
                commons_idx = Some(all.len());
            } else {
                if view.project.brain {
                    brains.push(view.project.slug.clone());
                }
                project_idx.push(all.len());
            }
            all.push(summary);
        }
        let brain = if brains.len() == 1 {
            brains[0].clone()
        } else {
            String::new()
        };
        let mut identities = BTreeMap::new();
        let mut active_agents = BTreeMap::new();
        let mut claims = Vec::new();
        let mut proposals = Vec::new();
        let mut questions = Vec::new();
        let mut ballot = Vec::new();
        let (mut votes_open, mut votes_plannable, mut posts24, mut posts, mut present_count) =
            (0, 0, 0, 0, 0);
        for p in &mut all {
            if p.get("present").and_then(Value::as_bool) != Some(true) {
                continue;
            }
            if p.get("kind").and_then(Value::as_str) == Some("project") {
                present_count += 1;
                ballot.push(object(
                    "slug base name brain suggestions amendments proposals",
                    vec![
                        p.get("slug").cloned().unwrap_or(Value::Null),
                        p.get("base").cloned().unwrap_or(Value::Null),
                        p.get("name").cloned().unwrap_or(Value::Null),
                        p.get("brain").cloned().unwrap_or(Value::Null),
                        json!(num(p, "suggestions_needing_votes")),
                        p.get("amendments_open").cloned().unwrap_or(Value::Null),
                        p.get("proposals_awaiting").cloned().unwrap_or(Value::Null),
                    ],
                ));
            }
            votes_open += num(p, "suggestions_needing_votes") + num(p, "amendments_open");
            votes_plannable += num(p, "suggestions_plannable");
            posts24 += num(p, "posts_24h");
            posts += num(p, "posts");
            for id in p
                .get("identities")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                identities.insert(str_value(&id), true);
            }
            p.remove("identities");
            for row in p
                .get("active_claims")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let mut claim = as_map(row);
                claim.insert("slug".into(), p.get("slug").cloned().unwrap_or(Value::Null));
                claim.insert("base".into(), p.get("base").cloned().unwrap_or(Value::Null));
                active_agents.insert(
                    format!(
                        "{}\0{}",
                        str_map(&claim, "agent"),
                        str_map(&claim, "session")
                    ),
                    true,
                );
                claims.push(claim);
            }
            for row in p
                .get("proposals")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let mut proposal = as_map(row);
                proposal.insert("slug".into(), p.get("slug").cloned().unwrap_or(Value::Null));
                proposal.insert("base".into(), p.get("base").cloned().unwrap_or(Value::Null));
                proposal.insert("can_decide".into(), json!(true));
                proposals.push(proposal);
            }
            p.remove("proposals");
            for row in p
                .get("questions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let mut question = as_map(row);
                question.insert("slug".into(), p.get("slug").cloned().unwrap_or(Value::Null));
                question.insert("base".into(), p.get("base").cloned().unwrap_or(Value::Null));
                questions.push(question);
            }
            p.remove("questions");
        }
        order(&mut claims, &["expires"]);
        order(&mut proposals, &["-date", "-slug"]);
        order(&mut questions, &["-date", "-slug"]);
        questions.sort_by_key(|r| !r.get("stop").and_then(Value::as_bool).unwrap_or(false));
        let projects: Vec<Value> = project_idx.iter().map(|&i| json!(all[i])).collect();
        let commons = commons_idx.map_or(Value::Null, |i| json!(all[i]));
        let commons_ok = commons.get("present").and_then(Value::as_bool) == Some(true);
        let commons_base = commons
            .get("base")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let commons_dir = commons
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let commons_questions = if commons_ok {
            json!(
                commons
                    .get("questions_open")
                    .cloned()
                    .unwrap_or(Value::Null)
            )
        } else {
            Value::Null
        };
        let questions_open = commons
            .get("questions_open")
            .cloned()
            .unwrap_or(Value::Null);
        let ballot_votes: i64 = ballot
            .iter()
            .map(|r| num(r, "suggestions") + num(r, "amendments"))
            .sum();
        let ballot_proposals: i64 = ballot.iter().map(|r| num(r, "proposals")).sum();
        object(
            "generated_at registry tally_mode can_decide commons_dir projects commons stamp notices totals active_claims proposals questions owner_presence",
            vec![
                json!(store_now(self)),
                json!(
                    self.store
                        .as_ref()
                        .map(|s| s.path.clone())
                        .unwrap_or_default()
                ),
                json!("go"),
                json!(true),
                json!(commons_dir),
                json!(projects),
                commons,
                json!(stamps.join("|")),
                json!(object(
                    "ballot votes_open proposals_awaiting brain brains commons_present commons_base commons_questions",
                    vec![
                        json!(ballot),
                        json!(ballot_votes),
                        json!(ballot_proposals),
                        json!(brain),
                        json!(brains),
                        json!(commons_ok),
                        json!(commons_base),
                        commons_questions
                    ]
                )),
                json!(object(
                    "projects projects_present agents agents_active votes_open votes_plannable proposals_awaiting owner_questions questions_open posts_24h posts brain brains",
                    vec![
                        json!(projects.len()),
                        json!(present_count),
                        json!(identities.len()),
                        json!(active_agents.len()),
                        json!(votes_open),
                        json!(votes_plannable),
                        json!(proposals.len()),
                        json!(questions.len()),
                        questions_open,
                        json!(posts24),
                        json!(posts),
                        json!(brain),
                        json!(brains)
                    ]
                )),
                json!(claims),
                json!(proposals),
                json!(questions),
                json!(
                    self.store
                        .as_ref()
                        .map(|s| s.setting("owner.presence", pulse_store::OWNER_PRESENT))
                        .unwrap_or_else(|| pulse_store::OWNER_PRESENT.to_string())
                ),
            ],
        )
    }
}

pub(crate) fn base_for(slug: &str) -> String {
    if slug == pulse_store::COMMONS_SLUG {
        "/commons".into()
    } else {
        format!("/p/{slug}")
    }
}

pub(crate) fn org_proposals(views: &[ProjectView]) -> Vec<JsonMap> {
    let mut out = Vec::new();
    for view in views {
        let Some(d) = &view.data else { continue };
        if view.project.slug == pulse_store::COMMONS_SLUG {
            continue;
        }
        for p in &d.proposals {
            let mut row = p.clone();
            row.remove("body");
            row.insert("slug".into(), json!(view.project.slug));
            row.insert("base".into(), json!(base_for(&view.project.slug)));
            out.push(row);
        }
    }
    order(&mut out, &["-date", "-slug"]);
    out.sort_by(|a, b| {
        let aa = str_map(a, "status") == "awaiting";
        let bb = str_map(b, "status") == "awaiting";
        bb.cmp(&aa)
    });
    out
}

/// Open owner questions across the org, newest first with evolution stops on
/// top; the body stays so the console can show the ask without a second fetch.
pub(crate) fn org_questions(views: &[ProjectView]) -> Vec<JsonMap> {
    let mut out = Vec::new();
    for view in views {
        let Some(d) = &view.data else { continue };
        if view.project.slug == pulse_store::COMMONS_SLUG {
            continue;
        }
        for q in &d.questions {
            let mut row = q.clone();
            row.insert("slug".into(), json!(view.project.slug));
            row.insert("base".into(), json!(base_for(&view.project.slug)));
            out.push(row);
        }
    }
    order(&mut out, &["-date", "-slug"]);
    out.sort_by_key(|r| !r.get("stop").and_then(Value::as_bool).unwrap_or(false));
    out
}

pub(crate) fn org_votes(views: &[ProjectView]) -> JsonMap {
    let mut suggestions = Vec::new();
    let mut amendments = Vec::new();
    for view in views {
        let Some(d) = &view.data else { continue };
        for s in &d.suggestions {
            if str_map(s, "status") == "needs votes" {
                let mut row = s.clone();
                row.insert("slug".into(), json!(view.project.slug));
                row.insert("base".into(), json!(base_for(&view.project.slug)));
                row.insert(
                    "quorum".into(),
                    d.params.get("quorum").cloned().unwrap_or(Value::Null),
                );
                row.insert(
                    "expiry_days".into(),
                    d.params.get("expiry_days").cloned().unwrap_or(Value::Null),
                );
                suggestions.push(row);
            }
        }
        for a in &d.amendments {
            if !str_map(a, "status").starts_with("closed:") {
                let mut row = a.clone();
                row.insert("slug".into(), json!(view.project.slug));
                row.insert("base".into(), json!(base_for(&view.project.slug)));
                amendments.push(row);
            }
        }
    }
    order(&mut suggestions, &["date", "slug"]);
    suggestions.sort_by(|a, b| {
        num(b, "voters")
            .cmp(&num(a, "voters"))
            .then_with(|| str_map(a, "date").cmp(&str_map(b, "date")))
    });
    order(&mut amendments, &["date", "slug"]);
    amendments.sort_by(|a, b| {
        let aa = str_map(a, "status").starts_with("passed");
        let bb = str_map(b, "status").starts_with("passed");
        bb.cmp(&aa)
    });
    object(
        "suggestions amendments",
        vec![json!(suggestions), json!(amendments)],
    )
}

pub(crate) fn org_activity(views: &[ProjectView], limit: usize) -> Vec<JsonMap> {
    let mut out = Vec::new();
    for view in views {
        let Some(d) = &view.data else { continue };
        let mut project_posts = Vec::new();
        for p in &d.posts {
            let mut row = pick(
                p,
                "id thread_key kind agent name session project date summary vote resolution provenance_ok",
            );
            row.insert("slug".into(), json!(view.project.slug));
            row.insert("base".into(), json!(base_for(&view.project.slug)));
            project_posts.push(row);
        }
        order(&mut project_posts, &["-date"]);
        project_posts.truncate(limit);
        out.extend(project_posts);
    }
    order(&mut out, &["-date", "-id"]);
    out.truncate(limit);
    out
}

pub(crate) fn timeline<R: std::borrow::Borrow<JsonMap>>(posts: &[R], now: String) -> JsonMap {
    let end = parse_dt(&now).unwrap_or_else(chrono::Utc::now);
    let mut hours = Vec::new();
    let mut days = Vec::new();
    let mut by_hour = BTreeMap::new();
    let mut by_day = BTreeMap::new();
    for i in (0..24).rev() {
        let key = (end - chrono::Duration::hours(i))
            .format("%Y-%m-%dT%H")
            .to_string();
        by_hour.insert(key.clone(), hours.len());
        hours.push(object(
            "t n kinds",
            vec![json!(key), json!(0), json!(BTreeMap::<String, i64>::new())],
        ));
    }
    for i in (0..30).rev() {
        let key = (end - chrono::Duration::days(i))
            .format("%Y-%m-%d")
            .to_string();
        by_day.insert(key.clone(), days.len());
        days.push(object(
            "t n kinds",
            vec![json!(key), json!(0), json!(BTreeMap::<String, i64>::new())],
        ));
    }
    let mut before = 0;
    let mut unparsed = 0;
    for p in posts {
        let p = p.borrow();
        let Some(mut dt) = parse_dt(&str_map(p, "date")) else {
            unparsed += 1;
            continue;
        };
        if dt > end {
            dt = end;
        }
        let day_key = dt.format("%Y-%m-%d").to_string();
        let Some(&day_idx) = by_day.get(&day_key) else {
            before += 1;
            continue;
        };
        let mut kind = str_map(p, "kind");
        if kind.is_empty() {
            kind = "(none)".into();
        }
        bump(&mut days[day_idx], &kind);
        let hour_key = dt.format("%Y-%m-%dT%H").to_string();
        if let Some(&hour_idx) = by_hour.get(&hour_key) {
            bump(&mut hours[hour_idx], &kind);
        }
    }
    object(
        "now hours days total before_window unparsed",
        vec![
            json!(end.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            json!(hours),
            json!(days),
            json!(posts.len()),
            json!(before),
            json!(unparsed),
        ],
    )
}

pub(crate) fn org_timeline(views: &[ProjectView], mut now: String) -> JsonMap {
    // Go appends every project's post maps to one slice; the maps are shared.
    let mut posts = Vec::<&JsonMap>::new();
    for view in views {
        if let Some(d) = &view.data {
            posts.extend(d.posts.iter());
            let n = str_map(&d.params, "now");
            if n > now {
                now = n;
            }
        }
    }
    let mut total = timeline(&posts, now.clone());
    let mut series = Vec::new();
    for view in views {
        if let Some(d) = &view.data {
            let mut row = timeline(&d.posts, now.clone());
            let kind = if view.project.slug == pulse_store::COMMONS_SLUG {
                "commons"
            } else {
                "project"
            };
            row.insert("slug".into(), json!(view.project.slug));
            row.insert("kind".into(), json!(kind));
            row.insert("base".into(), json!(base_for(&view.project.slug)));
            series.push(row);
        }
    }
    total.insert("series".into(), json!(series));
    total
}

pub(crate) fn json_map(value: Value) -> Result<JsonMap> {
    let data = pulse_store::gojson::to_vec(&value)?;
    let row = serde_json::from_slice::<Value>(&data)?;
    row.as_object()
        .cloned()
        .map(|m| m.into_iter().collect())
        .ok_or_else(|| anyhow!("JSON value is not an object"))
}

/// Go: func parseTime(text string) time.Time
pub(crate) fn parse_dt(text: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let text = text.trim().trim_end_matches('Z').replace(' ', "T");
    let text = if text.len() > 19 { &text[..19] } else { &text };
    for layout in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(t) = chrono::NaiveDateTime::parse_from_str(text, layout) {
            return Some(t.and_utc());
        }
    }
    chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|t| t.and_utc())
}

fn bump(row: &mut JsonMap, kind: &str) {
    let n = num(row, "n") + 1;
    row.insert("n".into(), json!(n));
    let mut kinds = row
        .get("kinds")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect::<JsonMap>();
    let current = kinds.get(kind).and_then(Value::as_i64).unwrap_or(0) + 1;
    kinds.insert(kind.to_string(), json!(current));
    row.insert("kinds".into(), json!(kinds));
}

fn str_map(row: &JsonMap, key: &str) -> String {
    row.get(key).map(str_value).unwrap_or_default()
}

fn num(row: &JsonMap, key: &str) -> i64 {
    row.get(key)
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0)
}

fn as_map(v: Value) -> JsonMap {
    v.as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect()
}

fn store_now(h: &Handler) -> String {
    h.store.as_ref().map(|s| s.now()).unwrap_or_default()
}

fn go_value<T: serde::Serialize>(v: &T) -> Value {
    let text = pulse_store::gojson::to_string(v).expect("serialize hub payload");
    serde_json::from_str(&text).expect("reparse hub payload")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers_f2 as f2;
    use anyhow::{Result, anyhow};
    use serde_json::{Map, Value, json};
    use std::io::Write;
    use std::process::Command;
    use std::sync::Arc;
    use std::time::Instant;

    fn lifecycle_repo() -> String {
        let dir = crate::test_helpers::hold_tempdir(tempfile::tempdir().expect("temp repo"));
        let dir = std::fs::canonicalize(dir).expect("canonical temp repo");
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .envs(pulse_gitenv::env().into_iter().filter_map(|kv| {
                    kv.split_once('=')
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                }))
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@x")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@x")
                .output()
                .expect("run git");
            assert!(
                out.status.success(),
                "git {args:?}: {}\n{}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("README.md"), "# t\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        dir.display().to_string()
    }

    fn project_reader_fixture() -> (f2::TestStore, Handler) {
        let s = f2::additions_store(false);
        let (agent, _) = pulse_board::join_agent(
            &s,
            pulse_board::JoinInput {
                name: "reader".into(),
                runner: "test".into(),
                model: "test".into(),
                session: "reader".into(),
                ..Default::default()
            },
        )
        .unwrap();
        for slug in ["delta", "charlie", "bravo", "alpha"] {
            f2::additions_project(&s, slug);
            s.conn()
                .unwrap()
                .execute(
                    "UPDATE projects SET path=?,brain=? WHERE slug=?",
                    rusqlite::params![lifecycle_repo(), slug == "alpha", slug],
                )
                .unwrap();
            pulse_board::create_post(
                &s,
                pulse_board::PostInput {
                    kind: "finding".into(),
                    project: slug.into(),
                    area: "console".into(),
                    thread: "readers".into(),
                    agent_row: Some(agent.clone()),
                    summary: format!("{slug} finding"),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let mut h = f2::handler(&s);
        h.org_cache_ttl = std::time::Duration::ZERO;
        (s, h)
    }

    #[derive(Clone)]
    struct ProjectReaderSource(ProjectReaderFn);
    type ProjectReaderFn = Arc<dyn Fn(&str, &str) -> Result<String> + Send + Sync>;

    impl crate::board::TallySource for ProjectReaderSource {
        fn records(&self, project: &str, area: &str) -> Result<String> {
            (self.0)(project, area)
        }
    }

    const GOLDEN_DIR: &str = "/tmp/pulse-parity/golden";

    fn read_golden(name: &str) -> Option<Value> {
        let path = std::path::Path::new(GOLDEN_DIR).join(name);
        let data = match std::fs::read(&path) {
            Ok(data) => data,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("Python parity goldens are not installed at {GOLDEN_DIR}");
                return None;
            }
            Err(err) => panic!("read {path:?}: {err}"),
        };
        Some(serde_json::from_slice(&data).expect("golden json"))
    }

    fn copy_snapshot(now: &str) -> Option<pulse_store::Store> {
        let mut src = match std::fs::File::open("/tmp/pulse-parity/pulse.sqlite") {
            Ok(src) => src,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("Python parity snapshot is not installed at /tmp/pulse-parity");
                return None;
            }
            Err(err) => panic!("open parity snapshot: {err}"),
        };
        let home = crate::test_helpers::hold_tempdir(tempfile::tempdir().expect("snapshot home"));
        let db = home.join("pulse.sqlite");
        let mut dst = std::fs::File::create(&db).expect("create snapshot copy");
        std::io::copy(&mut src, &mut dst).expect("copy snapshot");
        dst.flush().expect("flush snapshot");
        Some(
            pulse_store::Store::open(pulse_store::Options {
                home: home.display().to_string(),
                now: now.to_string(),
                ..Default::default()
            })
            .expect("open snapshot"),
        )
    }

    fn normalize_golden(v: Value, parent: &str) -> Value {
        match v {
            Value::Object(map) => {
                let mut out = Map::new();
                for (key, value) in map {
                    if matches!(
                        key.as_str(),
                        "generated_at"
                            | "time"
                            | "last_check"
                            | "built_at"
                            | "now"
                            | "stamp"
                            | "tally_script"
                            | "tally_mode"
                    ) || (parent == "tally" && matches!(key.as_str(), "script" | "mode"))
                    {
                        continue;
                    }
                    out.insert(key.clone(), normalize_golden(value, &key));
                }
                Value::Object(out)
            }
            Value::Array(rows) => Value::Array(
                rows.into_iter()
                    .map(|value| normalize_golden(value, parent))
                    .collect(),
            ),
            other => other,
        }
    }

    fn json_differences(path: &str, want: &Value, got: &Value) -> Vec<String> {
        if want == got {
            return Vec::new();
        }
        match (want, got) {
            (Value::Object(w), Value::Object(g)) => {
                let keys = w
                    .keys()
                    .chain(g.keys())
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>();
                keys.into_iter()
                    .flat_map(|key| {
                        json_differences(
                            &format!("{path}.{key}"),
                            w.get(&key).unwrap_or(&Value::Null),
                            g.get(&key).unwrap_or(&Value::Null),
                        )
                    })
                    .collect()
            }
            (Value::Array(w), Value::Array(g)) if w.len() == g.len() => w
                .iter()
                .zip(g.iter())
                .enumerate()
                .flat_map(|(i, (w, g))| json_differences(&format!("{path}[{i}]"), w, g))
                .collect(),
            (Value::Array(_), Value::Array(_)) => vec![format!("{path}.length")],
            _ => vec![path.to_string()],
        }
    }

    fn roster_golden(value: Value, legacy: bool) -> Value {
        let mut rows = value.as_array().cloned().expect("roster array");
        for row in &mut rows {
            if legacy
                && row.get("status").and_then(Value::as_str) == Some("working")
                && row.get("agent_status").is_some()
            {
                let status = row["agent_status"].clone();
                row.as_object_mut().unwrap().insert("status".into(), status);
            }
        }
        rows.sort_by(|a, b| {
            a.get("agent")
                .and_then(Value::as_str)
                .unwrap_or("")
                .cmp(b.get("agent").and_then(Value::as_str).unwrap_or(""))
        });
        Value::Array(rows)
    }

    fn thread_enrichment_golden_field(path: &str) -> bool {
        [
            ".participants",
            ".last_by",
            ".last_kind",
            ".open_questions",
            ".open_handoffs",
            ".active_claims",
        ]
        .iter()
        .any(|suffix| path.ends_with(suffix))
    }

    fn documented_difference(slug: &str, route: &str, path: &str) -> &'static str {
        if route == "roster" && path.starts_with("$[") {
            let field = path.split_once("].").map(|(_, f)| f).unwrap_or("");
            match field {
                "presence"
                | "presence_reason"
                | "last_heartbeat_at"
                | "presence_session"
                | "presence_checked_at"
                | "presence_expires_at" => {
                    return "WU-024 adds supervised presence separately from unchanged identity, score and history fields";
                }
                "current_job" | "current_job_kind" | "current_job_title" | "current_thread"
                | "executor" | "focus_since" | "current_task" | "current_task_title"
                | "current_project" | "current_source" => {
                    return "agents-focus-roster adds current work/task fields separately from identity, score and history fields";
                }
                _ => {}
            }
        }
        if path.ends_with(".head") || path.ends_with(".dirty") || path.ends_with(".branch") {
            return "checkout state: the goldens were captured at another commit with uncommitted changes";
        }
        if (route == "project" || (slug == "org" && route == "org"))
            && matches!(
                path,
                "$.store"
                    | "$.records_version"
                    | "$.schema_version"
                    | "$.binary_version"
                    | "$.registry"
                    | "$.fingerprint"
                    | "$.tool"
                    | "$.note"
            )
        {
            return "approved native metadata: actual store path and schema/records/binary versions";
        }
        if route == "project"
            && (slug == "amistio" || slug == "project-light")
            && (path == "$.evolution_present" || path == "$.board_params.constitution_present")
        {
            return "D-H/D-L: filesystem-only evolution has not been imported into this SQL snapshot";
        }
        if route == "settings" && path == "$.settings.length" {
            return "API v1 adds compute.* and evolve.* settings; legacy rows checked separately";
        }
        if route == "executors" && (path.ends_with(".admin") || path.ends_with(".healed_at")) {
            return "ADR-034 adds the admin route and the heal marker to every executor row";
        }
        if route == "runner" && path == "$.landing" {
            return "ADR-024: the runner view carries the daemon's landing holds so the console can say why verified branches wait";
        }
        if route == "overview" && path.starts_with("$.evolution.") {
            return "SQL-native evolution fields follow the revised contract, not the Python filesystem report";
        }
        if route == "overview" && path == "$.suggestions_plannable" {
            return "hub adds planner gate visibility for eligible suggestions";
        }
        if ((route == "overview" && path.contains(".open_threads["))
            || (route == "threads" && path.starts_with("$["))
            || (route == "thread" && path.starts_with("$.thread.")))
            && thread_enrichment_golden_field(path)
        {
            return "hub adds thread status enrichment for participants, last activity, open questions, handoffs, and claims";
        }
        if route == "suggestions" && path.starts_with("$[") {
            let field = path.split_once("].").map(|(_, f)| f).unwrap_or("");
            if matches!(field, "job" | "job_status" | "gate" | "plannable") {
                return "hub adds planner gate visibility for suggestion rows";
            }
        }
        if slug == "org" && route == "org" {
            if path == "$.owner_presence" || path == "$.totals.votes_plannable" {
                return "hub adds owner-away and planner gate visibility";
            }
            if path.ends_with(".suggestions_plannable") {
                return "hub adds planner gate visibility to project summaries";
            }
            for index in ["0", "1", "2"] {
                for key in ["evolution_present", "loop", "stop_present", "baseline"] {
                    if path == format!("$.projects[{index}].{key}") {
                        return "SQL-native evolution fields follow the revised contract";
                    }
                }
            }
            for key in ["evolution_present", "loop", "stop_present", "baseline"] {
                if path == format!("$.commons.{key}") {
                    return "SQL-native evolution fields follow the revised contract";
                }
            }
        }
        if slug == "org" && route == "votes" && path.starts_with("$.suggestions[") {
            let field = path.split_once("].").map(|(_, f)| f).unwrap_or("");
            if matches!(field, "job" | "job_status" | "gate" | "plannable") {
                return "hub adds planner gate visibility for organization vote rows";
            }
        }
        ""
    }

    fn path_escape(value: &str) -> String {
        let mut out = String::new();
        for &b in value.as_bytes() {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
        out
    }

    #[test]
    fn golden_parity() {
        let _serial = crate::test_helpers::serial();
        let Some(health) = read_golden("org__health.json") else {
            return;
        };
        let Some(s) = copy_snapshot(health["time"].as_str().unwrap_or("")) else {
            return;
        };
        let h = f2::handler(&s);
        for entry in std::fs::read_dir(GOLDEN_DIR).expect("golden dir") {
            let path = entry.expect("golden entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let stem = name.trim_end_matches(".json");
            let (slug, route) = stem.split_once("__").expect("golden name");
            let want = read_golden(&name).expect("golden file");
            let mut api_route = route.to_string();
            if route == "thread" {
                api_route = format!(
                    "threads/{}",
                    path_escape(want["thread"]["key"].as_str().unwrap_or(""))
                );
            }
            if route == "post" {
                api_route = format!("posts/{}", path_escape(want["id"].as_str().unwrap_or("")));
            }
            let api_path = if slug == "org" {
                format!("/api/v1/{api_route}")
            } else {
                format!("/api/v1/projects/{slug}/{api_route}")
            };
            let (status, got_map) = f2::additions_request(h.clone(), "GET", &api_path, "");
            assert_eq!(status, 200, "{api_path}");
            let mut got = Value::Object(got_map);
            if slug != "org" {
                for alias in [format!("/{slug}/api/v1/"), format!("/p/{slug}/api/v1/")] {
                    let (alias_status, alias_json) =
                        f2::additions_request(h.clone(), "GET", &(alias + &api_route), "");
                    assert_eq!(alias_status, 200, "alias {api_path}");
                    assert_eq!(
                        normalize_golden(got.clone(), ""),
                        normalize_golden(Value::Object(alias_json), ""),
                        "alias differs from {api_path}"
                    );
                }
            }
            if route == "evolution" {
                continue;
            }
            let mut want = want;
            if route == "roster" {
                want = roster_golden(want, true);
                got = roster_golden(got, false);
                eprintln!(
                    "WU-024: compare roster history by identity, without legacy claim-derived working labels"
                );
            }
            let diffs =
                json_differences("$", &normalize_golden(want, ""), &normalize_golden(got, ""));
            let unexpected = diffs
                .into_iter()
                .filter(|diff| documented_difference(slug, route, diff).is_empty())
                .collect::<Vec<_>>();
            assert!(
                unexpected.is_empty(),
                "{api_path} unexpected differences: {}",
                unexpected.join(", ")
            );
        }
    }

    #[test]
    fn legacy_settings_rows() {
        let _serial = crate::test_helpers::serial();
        let Some(want) = read_golden("org__settings.json") else {
            return;
        };
        let Some(s) = copy_snapshot("") else {
            return;
        };
        let got = f2::handler(&s).settings().unwrap();
        let rows = got["settings"].as_array().expect("settings rows");
        let want_rows = want["settings"].as_array().expect("golden settings rows");
        assert!(
            rows.len() > want_rows.len(),
            "new compute settings not present"
        );
        for (i, row) in want_rows.iter().enumerate() {
            let mut legacy = rows[i].clone();
            assert_eq!(
                legacy["secret"], false,
                "legacy setting missing explicit secret:false: {legacy:?}"
            );
            legacy.as_object_mut().unwrap().remove("secret");
            assert_eq!(row, &legacy, "legacy setting differs");
        }
        let mut names = rows
            .iter()
            .filter_map(|row| row["name"].as_str())
            .filter(|name| name.starts_with("compute."))
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, pulse_store::compute_setting_names());
    }

    #[test]
    fn extra_settings_effective_defaults() {
        let _serial = crate::test_helpers::serial();
        let s = f2::additions_store(false);
        let stored: i64 = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM settings WHERE name LIKE 'compute.%' OR name LIKE 'evolve.%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, 0, "fresh stores must not persist extra defaults");
        let h = f2::handler(&s);
        let (status, data) = f2::additions_request(h.clone(), "GET", "/api/v1/settings", "");
        assert_eq!(status, 200, "settings: {data:?}");
        let mut names = Vec::new();
        for value in data["settings"].as_array().unwrap() {
            let row = value.as_object().unwrap();
            let name = row["name"].as_str().unwrap();
            names.push(name.to_string());
            let secret = name == crate::privacy::HELDOUT_SETTING;
            assert_eq!(row["secret"], secret, "{name}: {row:?}");
            let def = pulse_store::COMPUTE_SETTINGS
                .get(name)
                .or_else(|| pulse_store::EVOLVE_SETTINGS.get(name));
            let Some(def) = def else {
                continue;
            };
            assert_eq!(row["value"], *def, "{name}: effective default");
            assert_eq!(row["default"], *def, "{name}: default");
            assert_eq!(row["secret"], secret, "{name}: secret");
        }
        let mut want_names = pulse_store::setting_names();
        want_names.extend(pulse_store::extra_setting_names());
        assert_eq!(names, want_names, "incorrect legacy/extra setting order");
        s.set_setting("human:dashboard", "compute.auto_approve", "1")
            .unwrap();
        let (status, data) = f2::additions_request(h, "GET", "/api/v1/settings", "");
        assert_eq!(status, 200, "updated settings: {data:?}");
        for value in data["settings"].as_array().unwrap() {
            let row = value.as_object().unwrap();
            if row["name"] == "compute.auto_approve" {
                assert_eq!(row["value"], "1", "stored compute override not applied");
                assert_eq!(row["secret"], false, "stored compute override not applied");
            }
        }
    }

    #[test]
    fn project_readers_git_timeout() {
        let _serial = crate::test_helpers::serial();
        let (s, h) = project_reader_fixture();
        let git = Command::new("sh")
            .arg("-c")
            .arg("command -v git")
            .output()
            .expect("find git");
        assert!(git.status.success(), "git not found");
        let git = String::from_utf8(git.stdout).unwrap().trim().to_string();
        let alpha = s.project("alpha").unwrap().unwrap();
        let bin = tempfile::tempdir().unwrap();
        std::fs::write(bin.path().join("git"), "#!/bin/sh\nif [ \"$PULSE_TEST_BLOCK_GIT\" = all ] || [ \"$2\" = \"$PULSE_TEST_BLOCK_GIT\" ]; then\n  if [ \"$PULSE_TEST_SLEEP_GIT\" = yes ]; then exec sleep 6; fi\n  exit 1\nfi\nexec \"$PULSE_TEST_REAL_GIT\" \"$@\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                bin.path().join("git"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        let _real = pulse_testutil::setenv("PULSE_TEST_REAL_GIT", git);
        let old_path = std::env::var("PATH").unwrap_or_default();
        let _path =
            pulse_testutil::setenv("PATH", format!("{}:{}", bin.path().display(), old_path));
        for (_name, blocked, posts) in [
            ("all_blocked", "all".to_string(), 0),
            ("one_blocked", alpha.path, 3),
        ] {
            for route in ["org", "projects", "timeline"] {
                let _blocked = pulse_testutil::setenv("PULSE_TEST_BLOCK_GIT", &blocked);
                let sleep = pulse_testutil::setenv("PULSE_TEST_SLEEP_GIT", "");
                let request = || {
                    let (status, data) =
                        f2::additions_request(h.clone(), "GET", &format!("/api/v1/{route}"), "");
                    assert_eq!(status, 200, "{route}: status {status}: {data:?}");
                    pulse_store::gojson::to_vec(&Value::Object(data)).unwrap()
                };
                let want = request();
                drop(sleep);
                let _sleep = pulse_testutil::setenv("PULSE_TEST_SLEEP_GIT", "yes");
                let start = Instant::now();
                let got = request();
                let elapsed = start.elapsed();
                assert!(
                    elapsed < std::time::Duration::from_secs(6),
                    "project probes took {elapsed:?}, want under 6s with a 5s git timeout"
                );
                assert_eq!(got, want, "timed-out git changed the JSON");
                let data: Value = serde_json::from_slice(&got).unwrap();
                if route == "timeline" {
                    assert_eq!(data["total"], posts);
                    continue;
                }
                let rows = data["projects"].as_array().unwrap();
                assert_eq!(rows.len(), 4);
                for (i, slug) in ["alpha", "bravo", "charlie", "delta"].iter().enumerate() {
                    let row = rows[i].as_object().unwrap();
                    assert_eq!(row["slug"], *slug, "project {i} lost order");
                    assert_eq!(
                        row["present"],
                        blocked != "all" && *slug != "alpha",
                        "project {i} presence"
                    );
                }
            }
        }
    }

    #[test]
    fn each_preserves_order_and_errors() {
        let _serial = crate::test_helpers::serial();
        let (s, mut h) = project_reader_fixture();
        let missing = tempfile::tempdir()
            .unwrap()
            .path()
            .join("missing")
            .display()
            .to_string();
        let charlie = crate::test_helpers::hold_tempdir(tempfile::tempdir().unwrap())
            .display()
            .to_string();
        for (slug, path) in [("bravo", missing), ("charlie", charlie)] {
            s.conn()
                .unwrap()
                .execute(
                    "UPDATE projects SET path=? WHERE slug=?",
                    rusqlite::params![path, slug],
                )
                .unwrap();
        }
        let store = s.store.clone();
        h.source = Some(Arc::new(ProjectReaderSource(Arc::new(
            move |project, area| {
                if project == "alpha" {
                    return Err(anyhow!("tally unavailable for alpha"));
                }
                pulse_tally::records(&store, project, area, pulse_tally::Thresholds::default())
            },
        ))));
        let views = h.each().unwrap();
        let want = [
            ("alpha", "tally unavailable for alpha", true),
            ("bravo", "missing on disk", false),
            ("charlie", "missing on disk", false),
            ("delta", "", true),
            (pulse_store::COMMONS_SLUG, "not created yet", false),
        ];
        assert_eq!(views.len(), want.len());
        for (i, (slug, err, present)) in want.iter().enumerate() {
            let view = &views[i];
            assert_eq!(view.project.slug, *slug, "view {i}");
            assert_eq!(view.present, *present, "view {i}");
            assert_eq!(view.error, *err, "view {i}");
            assert_eq!(view.data.is_none(), !err.is_empty(), "{slug} data");
            let summary = h.summary(view);
            assert_eq!(summary["present"], err.is_empty(), "{slug} present");
            assert_eq!(summary["missing"], !present, "{slug} missing");
            assert_eq!(summary["error"], *err, "{slug} error");
        }
    }
}
