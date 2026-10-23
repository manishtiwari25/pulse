use crate::board::{BoardData, Fields, PostRow, object, order, pick, str_value};
use crate::httpx::{Request, ResponseWriter};
use crate::server::Handler;
use crate::view_cache::org_mark;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub(crate) type JsonMap = BTreeMap<String, Value>;

#[derive(Debug, Clone, Default)]
pub(crate) struct ProjectView {
    pub(crate) project: pulse_store::Project,
    /// Shared with the snapshot cache and every other request reading it.
    pub(crate) data: Option<Arc<BoardData>>,
    pub(crate) error: String,
    pub(crate) present: bool,
    /// The store watermark the snapshot was built from, so organization
    /// responses assembled from these views can be reused while it holds.
    pub(crate) mark: String,
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
                let data = v
                    .get("data")
                    .cloned()
                    .and_then(|data| {
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
                                        object(
                                            "status plannable",
                                            vec![json!("eligible"), json!(true)],
                                        )
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
                    })
                    .map(Arc::new);
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
                    mark: String::new(),
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
        if !self.present_cached(&p) {
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
        let d =
            match self.snapshot_shared(&slug, &r.query("area"), route == "refresh", Some(&r.ctx)) {
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
                let row = match self.post_detail(&d, r.trim_start_matches("posts/")) {
                    Ok(row) => row,
                    Err(err) => {
                        self.respond(w, Value::Null, err.to_string());
                        return;
                    }
                };
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
            "timeline" => {
                let now = str_map(&d.params, "now");
                if r.query("provenance") == "missing" {
                    let posts = provenance_missing_posts(&d.posts);
                    json!(timeline(&posts, now))
                } else {
                    json!(timeline(&d.posts, now))
                }
            }
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
                        let present = self.present_cached(&p);
                        let mut view = ProjectView {
                            project: p.clone(),
                            present,
                            ..Default::default()
                        };
                        if present {
                            match self.snapshot_marked(&p.slug, "", false, None) {
                                Ok((data, mark)) => {
                                    view.data = Some(data);
                                    view.mark = mark;
                                }
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
        let views = match self.each() {
            Ok(v) => v,
            Err(err) => {
                self.respond(w, Value::Null, err.to_string());
                return;
            }
        };
        // The response for a route is reused while every project snapshot
        // behind it is the one it was built from; only the aggregation over
        // the shared snapshots is skipped, the snapshots themselves are
        // already shared.
        let ttl = if route == "org" {
            self.org_cache_ttl
        } else {
            self.org_cache_ttl
                .min(crate::view_cache::ORG_ROUTE_BYTES_TTL)
        };
        let mark = org_mark(&views);
        let json_headers = |w: &mut ResponseWriter| {
            w.header_mut().insert(
                hyper::header::CONTENT_TYPE,
                "application/json; charset=utf-8".parse().unwrap(),
            );
            w.header_mut()
                .insert(hyper::header::CACHE_CONTROL, "no-store".parse().unwrap());
        };
        if let Some(buf) = self.views.org_bytes(&route, &mark, ttl) {
            drop(views);
            w.header_mut().insert("x-org-cache", "hit".parse().unwrap());
            json_headers(w);
            w.write_header(200);
            let _ = w.write(&buf);
            return;
        }
        let data = match route.as_str() {
            "org" => {
                let mut org = self.org(&views);
                match self.metadata() {
                    Ok(metadata) => org.extend(metadata),
                    Err(err) => {
                        self.respond(w, Value::Null, err.to_string());
                        return;
                    }
                }
                json!(org)
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
        drop(views);
        let mut buf = match pulse_store::gojson::to_vec(&data) {
            Ok(v) => v,
            Err(err) => {
                self.respond(w, Value::Null, err.to_string());
                return;
            }
        };
        if route != "org" {
            buf.push(b'\n');
        }
        if !ttl.is_zero() {
            self.views.store_org_bytes(&route, &mark, &buf);
        }
        w.header_mut()
            .insert("x-org-cache", "miss".parse().unwrap());
        json_headers(w);
        w.write_header(200);
        let _ = w.write(&buf);
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
        // The newest post: the first in row order to carry the greatest date
        // (`min_by` keeps the first of equal elements; the comparison is
        // reversed).
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
                    &m.into_iter().collect::<JsonMap>(),
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
        let mut row = object(
            "slug kind brain base path remote name branch head dirty built_at board_present claims_active threads_open questions_open owner_questions suggestions_eligible suggestions_plannable suggestions_needing_votes suggestions_rejected amendments_open proposals_awaiting posts posts_24h can_decide agents identities evolution_present loop stop_present baseline last_post active_claims proposals questions present missing error",
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
        );
        // The shared board has no goal; a project row says whether its lead
        // works toward a goal the owner chose or the generated default.
        if kind == "project"
            && let Some(store) = self.store.as_deref()
        {
            let goal = crate::lifecycle::goal_summary(store, &p.slug);
            row.insert("goal_source".into(), json!(goal.source));
            row.insert("goal_preview".into(), json!(goal.preview));
        }
        row
    }

    pub(crate) fn org<'a, V: OrgViews<'a>>(&self, views: V) -> JsonMap {
        let views = views.into_views();
        let views = views.as_ref();
        // `projects` and `commons` are taken from `all` after the loop below
        // has stripped `identities`, `proposals` and `questions`, so the
        // emitted rows do not carry them.
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
            "generated_at registry can_decide commons_dir projects commons stamp notices totals active_claims proposals questions owner_presence",
            vec![
                json!(store_now(self)),
                json!(
                    self.store
                        .as_ref()
                        .map(|s| s.path.clone())
                        .unwrap_or_default()
                ),
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

/// Posts whose `provenance_ok` is not 1, matching `posts_without_provenance`
/// in the overview stats so the KPI tile can link to the same set.
pub(crate) fn provenance_missing_posts(posts: &[PostRow]) -> Vec<&PostRow> {
    posts
        .iter()
        .filter(|p| crate::board::num_value(p.get("provenance_ok").unwrap_or(&Value::Null)) != 1)
        .collect()
}

pub(crate) fn timeline<R: Fields>(posts: &[R], now: String) -> JsonMap {
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
    // Every project's posts, borrowed rather than cloned.
    let mut posts = Vec::<&PostRow>::new();
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

pub(crate) fn parse_dt(text: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let text = text.trim().trim_end_matches('Z').replace(' ', "T");
    // Cut on a char boundary: a byte slice panics on multi-byte input such as
    // a date typed into frontmatter with a non-ASCII suffix.
    let text = match text.char_indices().nth(19) {
        Some((cut, _)) => &text[..cut],
        None => text.as_str(),
    };
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

fn str_map<R: Fields + ?Sized>(row: &R, key: &str) -> String {
    row.field(key).map(str_value).unwrap_or_default()
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

    #[test]
    fn parse_dt_does_not_panic_on_multibyte_input() {
        // Trailing junk past the 19-char prefix is ignored, even when multi-byte.
        assert!(parse_dt("2026-10-04T01:04:47Zé").is_some());
        assert_eq!(
            parse_dt("2026-10-04T01:04:47Z").map(|d| d.timestamp()),
            parse_dt("2026-10-04 01:04:47").map(|d| d.timestamp())
        );
        assert!(parse_dt("2026-10-04T01:04:4é").is_none());
        assert!(parse_dt("ééééééééééééééééééééé").is_none());
    }
    use crate::test_helpers_http as http;
    use anyhow::{Result, anyhow};
    use serde_json::{Map, Value, json};
    use std::io::Write;
    use std::process::Command;
    use std::sync::Arc;
    use std::time::Instant;

    #[test]
    fn timeline_provenance_missing_filter_matches_overview_count() {
        let posts: Vec<PostRow> = [
            json!({"id": "a", "date": "2026-01-01T00:00:00Z", "kind": "finding", "provenance_ok": 1}),
            json!({"id": "b", "date": "2026-01-01T01:00:00Z", "kind": "finding", "provenance_ok": 0}),
            json!({"id": "c", "date": "2026-01-01T02:00:00Z", "kind": "finding"}),
        ]
        .into_iter()
        .map(|v| PostRow::from_map(&v.as_object().unwrap().clone().into_iter().collect()))
        .collect();
        let missing = provenance_missing_posts(&posts);
        let ids: Vec<String> = missing.iter().map(|p| str_map(p, "id")).collect();
        assert_eq!(ids, vec!["b", "c"]);
        let tl = timeline(&missing, "2026-01-01T03:00:00Z".into());
        assert_eq!(tl["total"], 2);
        assert_eq!(timeline(&posts, "2026-01-01T03:00:00Z".into())["total"], 3);
    }

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

    fn project_reader_fixture() -> (http::TestStore, Handler) {
        let s = http::additions_store(false);
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
            http::additions_project(&s, slug);
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
        let mut h = http::handler(&s);
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

    #[test]
    fn extra_settings_effective_defaults() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
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
        let h = http::handler(&s);
        let (status, data) = http::additions_request(h.clone(), "GET", "/api/v1/settings", "");
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
        let (status, data) = http::additions_request(h, "GET", "/api/v1/settings", "");
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
                        http::additions_request(h.clone(), "GET", &format!("/api/v1/{route}"), "");
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
