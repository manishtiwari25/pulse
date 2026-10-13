use crate::board::{object, str_value};
use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, write_error, write_json};
use anyhow::{Result, anyhow};
use rusqlite::types::Value as SqlValue;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::sync::Mutex;
use std::time::Duration;

pub(crate) type JsonMap = BTreeMap<String, Value>;
pub(crate) static ADDITION_WRITES: Mutex<()> = Mutex::new(());
pub(crate) const ADDITION_TASK_COLUMNS: &str = "t.id,t.project,t.title,t.status,t.priority,t.lead,t.job_id,t.created_by,t.created_at,t.updated_at,t.current_rev,rv.brief";
pub(crate) const ADDITION_TASK_FROM: &str =
    " FROM tasks t JOIN task_revisions rv ON rv.task_id=t.id AND rv.rev=t.current_rev";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AdditionEvent {
    pub(crate) id: i64,
    pub(crate) at: String,
    pub(crate) actor: String,
    pub(crate) action: String,
    pub(crate) subject: String,
    pub(crate) detail: Value,
}

impl Handler {
    pub(crate) fn additions(&self, w: &mut ResponseWriter, r: &Request, route: String) -> bool {
        let parts: Vec<String> = route
            .trim_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if parts.is_empty() {
            return false;
        }
        match parts[0].as_str() {
            "records" => self.addition_records(w, r, parts),
            "constitution" => {
                if parts.len() != 1 {
                    addition_not_found(w);
                } else {
                    self.addition_constitution(w, r);
                }
            }
            "events" => self.addition_events(w, r, parts),
            "tasks" => self.addition_tasks(w, r, parts),
            "jobs" => self.addition_jobs(w, r, parts),
            "code-deliveries" => self.addition_code_deliveries(w, r, parts),
            "messages" => self.addition_messages(w, r, parts),
            "compute" => self.addition_compute(w, r, parts),
            "projects" => return self.addition_projects(w, r, parts),
            "folders" => self.addition_folders(w, r, parts),
            "heldout" => self.addition_heldout(w, r, parts),
            "cred" => self.addition_cred(w, r, parts),
            "harness" => self.addition_harness(w, r, parts),
            "ask" => self.addition_ask(w, r, parts),
            "hygiene" => self.addition_hygiene(w, r, parts),
            "mcp" => self.addition_mcp(w, r, parts),
            "skills" => self.addition_skills(w, r, parts),
            "service" => self.addition_service(w, r, parts),
            _ => return false,
        }
        true
    }

    pub(crate) fn addition_project(&self, r: &Request) -> String {
        if !self.project.is_empty() {
            self.project.clone()
        } else {
            r.query("project")
        }
    }

    pub(crate) fn addition_available(
        &self,
        w: &mut ResponseWriter,
        r: &Request,
        tables: Vec<String>,
    ) -> bool {
        let Some(store) = self.store.as_deref() else {
            let code = if r.method == "POST" { 409 } else { 503 };
            write_error(
                w,
                code,
                "The store is unavailable".to_string(),
                "The hub has no open PULSE store.".to_string(),
                "Run pulse init, then restart pulse hub.".to_string(),
            );
            return false;
        };
        let conn = match store.conn() {
            Ok(c) => c,
            Err(e) => {
                addition_failure(w, e.to_string());
                return false;
            }
        };
        for table in tables {
            let exists = conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?",
                [&table],
                |row| row.get::<_, i64>(0),
            );
            match exists {
                Ok(0) => {
                    let what = if table == "task_revisions" {
                        "tasks are not available yet".to_string()
                    } else {
                        format!("{} are not available yet", table.replace('_', " "))
                    };
                    write_error(
                        w,
                        503,
                        what,
                        "This store does not have the required additive tables.".to_string(),
                        "Update PULSE and restart pulse hub.".to_string(),
                    );
                    return false;
                }
                Ok(_) => {}
                Err(e) => {
                    addition_failure(w, e.to_string());
                    return false;
                }
            }
        }
        true
    }

    pub(crate) fn addition_rows(
        &self,
        r: &Request,
        query: &str,
        args: Vec<SqlValue>,
    ) -> Result<Vec<JsonMap>> {
        self.query_rows(&r.ctx, query, args)
    }

    pub(crate) fn query_rows(
        &self,
        _ctx: &pulse_bus::CancellationToken,
        query: &str,
        args: Vec<SqlValue>,
    ) -> Result<Vec<JsonMap>> {
        let store = self
            .store
            .as_deref()
            .ok_or_else(|| anyhow!("the store is unavailable"))?;
        let conn = store.conn()?;
        let mut stmt = conn.prepare(query)?;
        let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let refs: Vec<&dyn rusqlite::ToSql> =
            args.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(refs.as_slice(), |row| {
            let mut out = JsonMap::new();
            for (i, key) in columns.iter().enumerate() {
                let v: SqlValue = row.get(i)?;
                out.insert(key.clone(), sql_json(v));
            }
            Ok(out)
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub(crate) fn addition_task(&self, r: &Request, id: &str) -> Result<JsonMap> {
        let rows = self.addition_rows(
            r,
            &format!("SELECT {ADDITION_TASK_COLUMNS}{ADDITION_TASK_FROM} WHERE t.id=?"),
            vec![SqlValue::Text(id.to_string())],
        )?;
        let project = self.addition_project(r);
        if rows.is_empty()
            || (!project.is_empty()
                && rows[0].get("project").and_then(Value::as_str).unwrap_or("") != project)
        {
            return Err(pulse_cli::errorf(
                2,
                &format!("No task {id:?}"),
                "No task matches this ID in the selected project.",
                "Select an existing task from the task list.",
            )
            .into());
        }
        let mut rows = rows;
        self.addition_task_work(&mut rows)?;
        Ok(rows.remove(0))
    }

    pub(crate) fn addition_task_work(&self, rows: &mut [JsonMap]) -> Result<()> {
        let Some(store) = self.store.as_deref() else {
            return Ok(());
        };
        let tasks = rows
            .iter()
            .map(|row| pulse_tasks::Task {
                id: addition_row_string(row, "id"),
                project: addition_row_string(row, "project"),
                job_id: addition_row_string(row, "job_id"),
                ..pulse_tasks::Task::default()
            })
            .collect::<Vec<_>>();
        let work = pulse_tasks::work_for(store, &tasks)?;
        for row in rows {
            let w = work
                .get(&addition_row_string(&*row, "id"))
                .cloned()
                .unwrap_or_default();
            row.insert("jobs_total".to_string(), json!(w.total));
            row.insert("jobs_queued".to_string(), json!(w.queued));
            row.insert("jobs_blocked".to_string(), json!(w.blocked));
            row.insert("jobs_running".to_string(), json!(w.running));
            row.insert("jobs_done".to_string(), json!(w.done));
            row.insert("jobs_failed".to_string(), json!(w.failed));
            row.insert("working".to_string(), value_from(&w.working));
        }
        Ok(())
    }

    pub(crate) fn addition_tasks(&self, w: &mut ResponseWriter, r: &Request, parts: Vec<String>) {
        if parts.len() > 3 || (parts.len() == 3 && parts[2] != "amend" && parts[2] != "cancel") {
            addition_not_found(w);
            return;
        }
        let methods = if parts.len() == 1 {
            vec!["GET".to_string(), "POST".to_string()]
        } else if parts.len() == 3 {
            vec!["POST".to_string()]
        } else {
            vec!["GET".to_string()]
        };
        if !addition_method(w, r, methods)
            || !self.addition_available(
                w,
                r,
                vec![
                    "tasks".to_string(),
                    "task_revisions".to_string(),
                    "messages".to_string(),
                ],
            )
        {
            return;
        }
        if r.method == "POST" {
            self.addition_task_write(w, r, parts);
            return;
        }
        if parts.len() == 1 {
            let mut query = format!("SELECT {ADDITION_TASK_COLUMNS}{ADDITION_TASK_FROM} WHERE 1=1");
            let mut args = Vec::new();
            let project = self.addition_project(r);
            if !project.is_empty() {
                query.push_str(" AND t.project=?");
                args.push(SqlValue::Text(project));
            }
            let status = r.query("status");
            if !status.is_empty() {
                if !pulse_tasks::STATUSES.contains(&status.as_str()) {
                    write_error(
                        w,
                        400,
                        "Invalid task status".to_string(),
                        "The requested status is not a task state.".to_string(),
                        "Use open, planned, in_progress, review, done, or cancelled.".to_string(),
                    );
                    return;
                }
                query.push_str(" AND t.status=?");
                args.push(SqlValue::Text(status));
            }
            query.push_str(" ORDER BY t.priority DESC,t.created_at,t.id");
            let mut tasks = match self.addition_rows(r, &query, args) {
                Ok(v) => v,
                Err(e) => {
                    addition_failure(w, e.to_string());
                    return;
                }
            };
            if let Err(e) = self.addition_task_work(&mut tasks) {
                addition_failure(w, e.to_string());
                return;
            }
            write_json(w, 200, json!({"tasks": tasks}));
            return;
        }
        let task = match self.addition_task(r, &parts[1]) {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        let revisions = match self.addition_rows(
            r,
            "SELECT rev,title,brief,note,author,at FROM task_revisions WHERE task_id=? ORDER BY rev",
            vec![SqlValue::Text(parts[1].clone())],
        ) {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        let messages = match self.addition_rows(
            r,
            "SELECT id,project,task_id,author,body,at FROM messages WHERE task_id=? ORDER BY rowid",
            vec![SqlValue::Text(parts[1].clone())],
        ) {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        let project = addition_row_string(&task, "project");
        let job_id = addition_row_string(&task, "job_id");
        // A task's jobs carry `task:ID` or `task:ID:label` (one task, several jobs).
        let (exact, like) = pulse_store::task_source_params(&parts[1]);
        let jobs = match self.addition_rows(
            r,
            &format!(
                "SELECT id,project,kind,tier,title,brief,source,priority,status,created_at,created_by,\
             lease_agent AS agent,lease_until,generation,attempts,finished_at,result,branch,\
             (SELECT hs.id FROM harness_sessions hs WHERE hs.job_id = jobs.id ORDER BY hs.started_at DESC LIMIT 1) AS harness_session \
             FROM jobs WHERE project=? AND ({} OR (? != '' AND id=?)) ORDER BY created_at,id",
                pulse_store::task_source_clause("source")
            ),
            vec![
                SqlValue::Text(project),
                SqlValue::Text(exact),
                SqlValue::Text(like),
                SqlValue::Text(job_id.clone()),
                SqlValue::Text(job_id),
            ],
        ) {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        write_json(
            w,
            200,
            json!({"task": task, "revisions": revisions, "messages": messages, "jobs": jobs}),
        );
    }

    pub(crate) fn addition_messages(
        &self,
        w: &mut ResponseWriter,
        r: &Request,
        parts: Vec<String>,
    ) {
        if parts.len() != 1 {
            addition_not_found(w);
            return;
        }
        if !addition_method(w, r, vec!["GET".to_string(), "POST".to_string()])
            || !self.addition_available(w, r, vec!["messages".to_string(), "tasks".to_string()])
        {
            return;
        }
        if r.method == "POST" {
            self.addition_message_write(w, r);
            return;
        }
        let mut query =
            "SELECT id,project,task_id,author,body,at FROM messages WHERE 1=1".to_string();
        let mut args = Vec::new();
        let project = self.addition_project(r);
        if !project.is_empty() {
            query.push_str(" AND project=?");
            args.push(SqlValue::Text(project));
        }
        let task = r.query("task");
        if !task.is_empty() {
            query.push_str(" AND task_id=?");
            args.push(SqlValue::Text(task));
        }
        let since = r.query("since");
        if !since.is_empty() {
            query.push_str(" AND rowid>(SELECT rowid FROM messages WHERE id=?)");
            args.push(SqlValue::Text(since));
        }
        let messages = match self.addition_rows(r, &(query + " ORDER BY rowid"), args) {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        write_json(w, 200, json!({"messages": messages}));
    }

    pub(crate) fn addition_compute(&self, w: &mut ResponseWriter, r: &Request, parts: Vec<String>) {
        if parts.len() == 4 && parts[1] == "executors" && parts[3] == "remove" {
            self.addition_executor_remove(w, r, parts[2].clone());
            return;
        }
        if parts.len() != 1 && (parts.len() != 3 || (parts[2] != "approve" && parts[2] != "deny")) {
            addition_not_found(w);
            return;
        }
        let method = if parts.len() == 3 { "POST" } else { "GET" };
        if !addition_method(w, r, vec![method.to_string()])
            || !self.addition_available(w, r, vec!["compute_requests".to_string()])
        {
            return;
        }
        if r.method == "POST" {
            self.addition_compute_write(w, r, &parts[1], &parts[2]);
            return;
        }
        let mut query = "SELECT id,project,requested_by,size,hours,reason,status,decided_by,executor,at,updated_at FROM compute_requests".to_string();
        let mut args = Vec::new();
        let project = self.addition_project(r);
        if !project.is_empty() {
            query.push_str(" WHERE project=?");
            args.push(SqlValue::Text(project));
        }
        let requests = match self.addition_rows(r, &(query + " ORDER BY at,id"), args) {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        let executors = match self.addition_rows(r, "SELECT * FROM executors ORDER BY name", vec![])
        {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let caps = match pulse_tasks::get_caps(store) {
            Ok(v) => value_from(&v),
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        write_json(
            w,
            200,
            json!({"requests": requests, "executors": executors, "caps": caps}),
        );
    }

    pub(crate) fn addition_task_write(
        &self,
        w: &mut ResponseWriter,
        r: &Request,
        parts: Vec<String>,
    ) {
        let (payload, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let _lock = ADDITION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
        let Some(store) = self.store.as_deref() else {
            return;
        };
        if parts.len() == 1 {
            let (mut project, ok) =
                addition_string(w, &payload, "project", self.project.is_empty());
            if !ok {
                return;
            }
            if project.is_empty() {
                project = self.project.clone();
            }
            if !self.project.is_empty() && project != self.project {
                write_error(
                    w,
                    400,
                    "Project mismatch".to_string(),
                    "This hub serves a different project.".to_string(),
                    "Send the task to the selected project.".to_string(),
                );
                return;
            }
            let (title, ok) = addition_string(w, &payload, "title", true);
            if !ok {
                return;
            }
            let (brief, ok) = addition_string(w, &payload, "brief", true);
            if !ok {
                return;
            }
            let (priority, ok) = addition_priority(w, &payload);
            if !ok {
                return;
            }
            let created = match pulse_tasks::new(
                store,
                &project,
                &title,
                &brief,
                priority as i32,
                pulse_store::HUMAN_AGENT,
            ) {
                Ok(v) => v,
                Err(e) => {
                    addition_failure(w, e.to_string());
                    return;
                }
            };
            let task = match self.addition_task(r, &created.id) {
                Ok(v) => v,
                Err(e) => {
                    addition_failure(w, e.to_string());
                    return;
                }
            };
            write_json(w, 201, json!({"task": task}));
            return;
        }
        let (title, brief, note, reason, ok) = if parts[2] == "amend" {
            let has_title = payload.contains_key("title");
            let (title, ok1) = addition_string(w, &payload, "title", has_title);
            let (brief, ok2) = addition_string(w, &payload, "brief", true);
            let (note, ok3) = addition_string(w, &payload, "note", false);
            (title, brief, note, String::new(), ok1 && ok2 && ok3)
        } else {
            let (reason, ok1) = addition_string(w, &payload, "reason", false);
            (String::new(), String::new(), String::new(), reason, ok1)
        };
        if !ok {
            return;
        }
        let task = match self.addition_task(r, &parts[1]) {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        let status = addition_row_string(&task, "status");
        if status == "done" || status == "cancelled" {
            write_error(
                w,
                409,
                "Task is closed".to_string(),
                "Completed or cancelled work cannot be amended.".to_string(),
                "Create a new task instead.".to_string(),
            );
            return;
        }
        let result = if parts[2] == "amend" {
            pulse_tasks::amend(
                store,
                &parts[1],
                &title,
                &brief,
                &note,
                pulse_store::HUMAN_AGENT,
            )
        } else {
            pulse_tasks::cancel(store, &parts[1], &reason, pulse_store::HUMAN_AGENT)
        };
        if let Err(e) = result {
            addition_failure(w, e.to_string());
            return;
        }
        let task = match self.addition_task(r, &parts[1]) {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        write_json(w, 200, json!({"task": task}));
    }

    pub(crate) fn addition_message_write(&self, w: &mut ResponseWriter, r: &Request) {
        let (payload, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let _lock = ADDITION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let (mut project, ok) = addition_string(w, &payload, "project", self.project.is_empty());
        if !ok {
            return;
        }
        if project.is_empty() {
            project = self.project.clone();
        }
        if !self.project.is_empty() && project != self.project {
            write_error(
                w,
                400,
                "Project mismatch".to_string(),
                "This hub serves a different project.".to_string(),
                "Send the message to the selected project.".to_string(),
            );
            return;
        }
        let (task_id, ok) = addition_string(w, &payload, "task_id", false);
        if !ok {
            return;
        }
        let (body, ok) = addition_string(w, &payload, "body", true);
        if !ok {
            return;
        }
        let message = match pulse_tasks::send_message(
            store,
            &project,
            &task_id,
            pulse_store::HUMAN_AGENT,
            &body,
        ) {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        let mut messages = match self.addition_rows(
            r,
            "SELECT id,project,task_id,author,body,at FROM messages WHERE id=?",
            vec![SqlValue::Text(message.id)],
        ) {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        if messages.is_empty() {
            addition_failure(w, "not found".to_string());
            return;
        }
        write_json(w, 201, json!({"message": messages.remove(0)}));
    }

    pub(crate) fn addition_compute_write(
        &self,
        w: &mut ResponseWriter,
        r: &Request,
        id: &str,
        action: &str,
    ) {
        let (payload, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let _lock = ADDITION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let (words, ok) = addition_string(w, &payload, "words", false);
        if !ok {
            return;
        }
        let query = "SELECT id,project,requested_by,size,hours,reason,status,decided_by,executor,at,updated_at FROM compute_requests WHERE id=?";
        let mut requests = match self.addition_rows(r, query, vec![SqlValue::Text(id.to_string())])
        {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        if requests.is_empty()
            || (!self.project.is_empty()
                && addition_row_string(&requests[0], "project") != self.project)
        {
            addition_failure(w, "not found".to_string());
            return;
        }
        if addition_row_string(&requests[0], "status") != "pending" {
            write_error(
                w,
                409,
                "Compute request is already decided".to_string(),
                "Only pending requests may be approved or denied.".to_string(),
                "Refresh the compute requests.".to_string(),
            );
            return;
        }
        let result = if action == "approve" {
            pulse_tasks::approve(store, id, &words, pulse_store::HUMAN_AGENT)
        } else {
            pulse_tasks::deny(store, id, &words, pulse_store::HUMAN_AGENT)
        };
        if let Err(e) = result {
            addition_failure(w, e.to_string());
            return;
        }
        requests = match self.addition_rows(r, query, vec![SqlValue::Text(id.to_string())]) {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        if requests.is_empty() {
            addition_failure(w, "not found".to_string());
            return;
        }
        write_json(w, 200, json!({"request": requests.remove(0)}));
    }

    pub(crate) fn addition_records(&self, w: &mut ResponseWriter, r: &Request, parts: Vec<String>) {
        if parts.len() == 3 {
            self.addition_record_decision(w, r, &parts[1], &parts[2]);
            return;
        }
        if parts.len() > 2 {
            addition_not_found(w);
            return;
        }
        if !addition_method(w, r, vec!["GET".to_string()])
            || !self.addition_available(
                w,
                r,
                vec!["records".to_string(), "record_revisions".to_string()],
            )
        {
            return;
        }
        let Some(store) = self.store.as_deref() else {
            return;
        };
        if parts.len() == 1 {
            let records = match store.list_records(pulse_store::RecordFilter {
                project: self.addition_project(r),
                kind: r.query("kind"),
                status: r.query("status"),
                area: r.query("area"),
                ..Default::default()
            }) {
                Ok(v) => v,
                Err(e) => {
                    addition_failure(w, e.to_string());
                    return;
                }
            };
            let out = records.into_iter().map(addition_record).collect::<Vec<_>>();
            write_json(w, 200, json!({"records": out}));
            return;
        }
        let (rev, ok) = addition_integer(w, &r.query("rev"), "rev", 0, 0);
        if !ok {
            return;
        }
        let (rec, revision) = match store.get_record(&parts[1], rev) {
            Ok((rec, Some(rev))) => (rec, rev),
            Ok((rec, None)) => {
                write_error(
                    w,
                    404,
                    "Revision not found".to_string(),
                    "record revision not found".to_string(),
                    "Select an existing revision.".to_string(),
                );
                let _ = rec;
                return;
            }
            Err(e) => {
                if e.to_string().contains(" has no revision ") {
                    write_error(
                        w,
                        404,
                        "Revision not found".to_string(),
                        "record revision not found".to_string(),
                        "Select an existing revision.".to_string(),
                    );
                    return;
                }
                addition_failure(w, e.to_string());
                return;
            }
        };
        let project = self.addition_project(r);
        if !project.is_empty() && project != rec.project {
            addition_failure(w, "no such record".to_string());
            return;
        }
        let revisions = match store.revisions(&rec.id) {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        let history = revisions
            .into_iter()
            .map(|r| json!({"rev": r.rev, "author": r.author, "at": r.at, "note": r.note}))
            .collect::<Vec<_>>();
        write_json(
            w,
            200,
            json!({
                "record": addition_record(rec.clone()),
                "frontmatter": revision.frontmatter,
                "body": revision.body,
                "revisions": history,
                "draft": crate::evolution::draft_summary(store, &rec.id, true),
            }),
        );
    }

    pub(crate) fn addition_constitution(&self, w: &mut ResponseWriter, r: &Request) {
        if !addition_method(w, r, vec!["GET".to_string()])
            || !self.addition_available(
                w,
                r,
                vec!["records".to_string(), "record_revisions".to_string()],
            )
        {
            return;
        }
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let mut project = self.addition_project(r);
        if project.is_empty() {
            match store.brain() {
                Ok(Some(brain)) => project = brain.slug,
                Ok(None) => {}
                Err(e) => {
                    addition_failure(w, e.to_string());
                    return;
                }
            }
        }
        if project.is_empty() {
            write_error(
                w,
                400,
                "A project is required".to_string(),
                "There is no default framework project.".to_string(),
                "Select a project with ?project=<slug>.".to_string(),
            );
            return;
        }
        if project != pulse_store::COMMONS_SLUG {
            match store.project(&project) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    write_error(
                        w,
                        404,
                        "no such project".to_string(),
                        "The requested project is not registered.".to_string(),
                        "Select a registered project.".to_string(),
                    );
                    return;
                }
                Err(e) => {
                    addition_failure(w, e.to_string());
                    return;
                }
            }
        }
        let records = match store.list_records(pulse_store::RecordFilter {
            project: project.clone(),
            kind: "evolution".to_string(),
            slug: "constitution".to_string(),
            ..Default::default()
        }) {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        if records.is_empty() {
            write_json(
                w,
                200,
                json!({"record": Value::Null, "body": "", "amendments": Vec::<Value>::new()}),
            );
            return;
        }
        let (rec, revision) = match store.get_record(&records[0].id, 0) {
            Ok((rec, Some(rev))) => (rec, rev),
            Ok((rec, None)) => (rec, pulse_store::Revision::default()),
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        let mut amendments = Vec::new();
        if self.source.is_some() {
            match self.snapshot(&project, "") {
                Ok(snapshot) => {
                    for amendment in snapshot.amendments {
                        let status = addition_row_string(&amendment, "status");
                        amendments.push(json!({
                            "id": amendment.get("id").cloned().unwrap_or(Value::Null),
                            "status": status,
                            "votes_up": amendment.get("up").cloned().unwrap_or(Value::Null),
                            "votes_down": amendment.get("down").cloned().unwrap_or(Value::Null),
                            "quorum": amendment.get("quorum").cloned().unwrap_or(Value::Null),
                            "passed": status.starts_with("passed"),
                            "protected": addition_protected(&amendment, &revision.body),
                        }));
                    }
                }
                Err(e) => {
                    addition_failure(w, e.to_string());
                    return;
                }
            }
        }
        write_json(
            w,
            200,
            json!({"record": addition_record(rec), "body": revision.body, "amendments": amendments}),
        );
    }

    pub(crate) fn addition_read_events(
        &self,
        r: &Request,
        since: i64,
        limit: i64,
    ) -> Result<Vec<AdditionEvent>> {
        let store = self
            .store
            .as_deref()
            .ok_or_else(|| anyhow!("the store is unavailable"))?;
        let events =
            pulse_bus::latest(store, since, self.addition_event_filter(r), limit as usize)?;
        let redact = pulse_bus::redactor(Some(store));
        Ok(events
            .into_iter()
            .map(|event| addition_bus_event(redact(event)))
            .collect())
    }

    pub(crate) fn addition_event_filter(&self, r: &Request) -> pulse_bus::Filter {
        let mut actions = Vec::new();
        for key in ["kind", "action", "kinds", "actions"] {
            for value in r.query_all(key) {
                for action in value.split(',') {
                    let action = action.trim();
                    if !action.is_empty() {
                        actions.push(action.to_string());
                    }
                }
            }
        }
        pulse_bus::Filter {
            actions,
            project: self.addition_project(r),
            subject: r.query("subject"),
            ..Default::default()
        }
    }

    pub(crate) fn addition_events(&self, w: &mut ResponseWriter, r: &Request, parts: Vec<String>) {
        let stream = parts.len() == 2 && parts[1] == "stream";
        if parts.len() != 1 && !stream {
            addition_not_found(w);
            return;
        }
        if !addition_method(w, r, vec!["GET".to_string()])
            || !self.addition_available(w, r, vec!["events".to_string()])
        {
            return;
        }
        let mut cursor = r.query("since");
        if cursor.is_empty() && stream {
            cursor = r.header("Last-Event-ID");
        }
        let (since, ok) = addition_integer(w, &cursor, "since", 0, 0);
        if !ok {
            return;
        }
        let (limit, ok) = addition_integer(w, &r.query("limit"), "limit", 100, 1000);
        if !ok {
            return;
        }
        if limit == 0 {
            write_error(
                w,
                400,
                "Invalid limit".to_string(),
                "limit must be greater than zero.".to_string(),
                "Use a limit between 1 and 1000.".to_string(),
            );
            return;
        }
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let mut head = 0;
        let mut events = Vec::new();
        if stream {
            match pulse_bus::last_id(store) {
                Ok(v) => head = v,
                Err(e) => {
                    addition_failure(w, e.to_string());
                    return;
                }
            }
        }
        if !stream || since > 0 {
            match self.addition_read_events(r, since, limit) {
                Ok(v) => events = v,
                Err(e) => {
                    addition_failure(w, e.to_string());
                    return;
                }
            }
        }
        if !stream {
            write_json(w, 200, json!({"events": events}));
            return;
        }
        w.header_mut().insert(
            hyper::header::CONTENT_TYPE,
            hyper::header::HeaderValue::from_static("text/event-stream"),
        );
        w.header_mut().insert(
            hyper::header::CACHE_CONTROL,
            hyper::header::HeaderValue::from_static("no-cache"),
        );
        w.header_mut().insert(
            hyper::header::HeaderName::from_static("x-accel-buffering"),
            hyper::header::HeaderValue::from_static("no"),
        );
        if w.write(b": connected\n\n").is_err() || w.flush().is_err() {
            return;
        }
        for event in events {
            if !send_sse(w, &event) {
                return;
            }
            if event.id > head {
                head = event.id;
            }
        }
        let tail = pulse_bus::tail(
            r.ctx.clone(),
            store.clone(),
            head,
            self.addition_event_filter(r),
            Duration::from_secs(1),
        );
        loop {
            if r.ctx.is_cancelled() {
                return;
            }
            match tail.recv_timeout(Duration::from_secs(15)) {
                Ok(event) => {
                    let event = addition_bus_event(pulse_bus::redactor(Some(store))(event));
                    if !send_sse(w, &event) {
                        return;
                    }
                }
                Err(err) => {
                    if err.is_timeout() {
                        if w.write(b": heartbeat\n\n").is_err() || w.flush().is_err() {
                            return;
                        }
                    } else {
                        return;
                    }
                }
            }
        }
    }
}

pub(crate) fn addition_available(
    h: &Handler,
    w: &mut ResponseWriter,
    r: &Request,
    tables: Vec<String>,
) -> bool {
    h.addition_available(w, r, tables)
}

pub(crate) fn addition_not_found(w: &mut ResponseWriter) {
    write_error(
        w,
        404,
        "Unknown API route".to_string(),
        "That owner-console route does not exist.".to_string(),
        "Use an /api/v1 route.".to_string(),
    );
}

pub(crate) fn addition_method(w: &mut ResponseWriter, r: &Request, methods: Vec<String>) -> bool {
    if methods.iter().any(|m| m == &r.method) {
        return true;
    }
    if let Ok(v) = hyper::header::HeaderValue::from_str(&methods.join(", ")) {
        w.header_mut().insert(hyper::header::ALLOW, v);
    }
    write_error(
        w,
        405,
        "Method not allowed".to_string(),
        format!("This route does not support {}.", r.method),
        format!("Use {}.", methods.join(" or ")),
    );
    false
}

pub(crate) trait AdditionFailureArg {
    fn failure_text(self) -> String;
}

impl AdditionFailureArg for String {
    fn failure_text(self) -> String {
        self
    }
}

impl AdditionFailureArg for &str {
    fn failure_text(self) -> String {
        self.to_string()
    }
}

impl AdditionFailureArg for anyhow::Error {
    fn failure_text(self) -> String {
        self.to_string()
    }
}

impl AdditionFailureArg for rusqlite::Error {
    fn failure_text(self) -> String {
        self.to_string()
    }
}

impl AdditionFailureArg for pulse_cli::Error {
    fn failure_text(self) -> String {
        self.what
    }
}

pub(crate) fn addition_failure<E: AdditionFailureArg>(w: &mut ResponseWriter, err: E) {
    let err = err.failure_text();
    let mut code = 409;
    let mut what = err;
    let mut why = "The store could not complete this request.".to_string();
    let mut fix = "Refresh the page and check the request.".to_string();
    if let Some((head, rest)) = what.split_once("\n  why: ") {
        let parsed_why;
        if let Some((w, f)) = rest.split_once("\n  fix: ") {
            parsed_why = w.to_string();
            fix = f.to_string();
        } else {
            parsed_why = rest.to_string();
        }
        what = head.to_string();
        why = parsed_why;
        code = 400;
    }
    if what == "no such record" {
        code = 404;
        what = "no such record".to_string();
        why = "No record matches the requested ID and project.".to_string();
        fix = "Select an existing record from the records list.".to_string();
    } else if what == "not found" || what == "query returned no rows" {
        code = 404;
        what = "not found".to_string();
        why = "The requested item does not exist in this project.".to_string();
        fix = "Select an existing item from its list.".to_string();
    } else if what.starts_with("Invalid ") {
        code = 400;
        why = "The request contains an invalid value.".to_string();
        fix = "Correct the value and submit again.".to_string();
    } else if what.contains("Not a directory") || what.contains("Not a git checkout") {
        code = 400;
    } else if what.starts_with("No task ")
        || what.starts_with("No message ")
        || what.starts_with("No compute request ")
        || what.starts_with("No project ")
        || what.starts_with("No executor ")
        || what.starts_with("No MCP server ")
        || what.starts_with("No skill ")
    {
        code = 404;
    }
    write_error(w, code, what, why, fix);
}

pub(crate) fn addition_string<P, K>(
    w: &mut ResponseWriter,
    payload: P,
    key: K,
    required: bool,
) -> (String, bool)
where
    P: Borrow<JsonMap>,
    K: AsRef<str>,
{
    let payload = payload.borrow();
    let key = key.as_ref();
    let Some(value) = payload.get(key) else {
        return if required {
            invalid_string(w, key, required)
        } else {
            (String::new(), true)
        };
    };
    let Some(text) = value.as_str() else {
        return invalid_string(w, key, required);
    };
    if (required && text.trim().is_empty()) || text.contains('\0') {
        return invalid_string(w, key, required);
    }
    (text.to_string(), true)
}

fn invalid_string(w: &mut ResponseWriter, key: &str, required: bool) -> (String, bool) {
    let kind = if required {
        "non-empty string"
    } else {
        "string"
    };
    write_error(
        w,
        400,
        format!("Invalid {key}"),
        format!("{key} must be a {kind}."),
        format!("Send a JSON object with a valid {key}."),
    );
    (String::new(), false)
}

pub(crate) fn addition_priority<P: Borrow<JsonMap>>(
    w: &mut ResponseWriter,
    payload: P,
) -> (i64, bool) {
    let payload = payload.borrow();
    let Some(value) = payload.get("priority") else {
        return (0, true);
    };
    let n = if let Some(n) = value.as_i64() {
        n
    } else if let Some(n) = value.as_f64() {
        if n.trunc() != n || n > i32::MAX as f64 || n < i32::MIN as f64 {
            i64::MAX
        } else {
            n as i64
        }
    } else {
        i64::MAX
    };
    if n > i32::MAX as i64 || n < i32::MIN as i64 {
        write_error(
            w,
            400,
            "Invalid priority".to_string(),
            "priority must be a signed 32-bit JSON integer.".to_string(),
            "Send a whole number for priority.".to_string(),
        );
        return (0, false);
    }
    (n, true)
}

pub(crate) fn addition_row_string<R, K>(row: R, key: K) -> String
where
    R: Borrow<JsonMap>,
    K: AsRef<str>,
{
    row.borrow()
        .get(key.as_ref())
        .map(str_value)
        .unwrap_or_default()
}

pub(crate) fn addition_integer(
    w: &mut ResponseWriter,
    value: &str,
    name: &str,
    fallback: i64,
    maximum: i64,
) -> (i64, bool) {
    if value.is_empty() {
        return (fallback, true);
    }
    let Ok(n) = value.parse::<i64>() else {
        return invalid_integer(w, name);
    };
    if n < 0 || (maximum > 0 && n > maximum) {
        return invalid_integer(w, name);
    }
    (n, true)
}

fn invalid_integer(w: &mut ResponseWriter, name: &str) -> (i64, bool) {
    write_error(
        w,
        400,
        format!("Invalid {name}"),
        format!("{name} must be a non-negative integer within the supported range."),
        format!("Use a valid {name} query parameter."),
    );
    (0, false)
}

pub(crate) fn addition_record(rec: pulse_store::Record) -> JsonMap {
    let mut out = JsonMap::new();
    out.insert("id".into(), json!(rec.id));
    out.insert("project".into(), json!(rec.project));
    out.insert("kind".into(), json!(rec.kind));
    out.insert("number".into(), json!(rec.number));
    out.insert("slug".into(), json!(rec.slug));
    out.insert("title".into(), json!(rec.title));
    out.insert("status".into(), json!(rec.status));
    out.insert("tier".into(), json!(rec.tier));
    out.insert("areas".into(), json!(rec.areas));
    out.insert("tags".into(), json!(rec.tags));
    out.insert("delivery".into(), json!(rec.delivery));
    out.insert("delivered".into(), json!(rec.delivered));
    out.insert("date".into(), json!(rec.date));
    out.insert("path".into(), json!(rec.path));
    out.insert("updated_at".into(), json!(rec.updated_at));
    out.insert("current_rev".into(), json!(rec.current_rev));
    out
}

pub(crate) fn addition_protected(amendment: &JsonMap, constitution: &str) -> bool {
    if let Some(protected) = amendment.get("protected").and_then(Value::as_bool) {
        return protected;
    }
    let status = addition_row_string(amendment, "status");
    if status.to_lowercase().contains("protected") {
        return true;
    }
    let mut protected = "Hard stops, Kill switch, Amendments, frontmatter".to_string();
    for line in constitution.split('\n') {
        let cells = line
            .trim_matches([' ', '\t', '|'])
            .split('|')
            .collect::<Vec<_>>();
        if cells.len() > 1
            && cells[0]
                .trim()
                .trim_matches(['*', '`', ' '])
                .eq_ignore_ascii_case("Not amendable by vote")
        {
            protected = cells[1].to_string();
            break;
        }
    }
    let sections = addition_row_string(amendment, "section");
    for section in sections.split('|') {
        for name in protected.split(',') {
            let name = name.trim_matches([' ', '`', '\t']);
            if !name.is_empty()
                && section
                    .trim_matches([' ', '`', '\t'])
                    .eq_ignore_ascii_case(name)
            {
                return true;
            }
        }
    }
    false
}

pub(crate) fn addition_bus_event(event: pulse_bus::Event) -> AdditionEvent {
    let detail = serde_json::from_str(&event.detail)
        .map(go_any_numbers)
        .unwrap_or_else(|_| Value::String(event.detail));
    private_setting_event(AdditionEvent {
        id: event.id,
        at: event.at,
        actor: event.actor,
        action: event.action,
        subject: event.subject,
        detail,
    })
}

fn go_any_numbers(value: Value) -> Value {
    match value {
        Value::Number(n) => n
            .as_f64()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or(Value::Number(n)),
        Value::Array(items) => Value::Array(items.into_iter().map(go_any_numbers).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (key, go_any_numbers(value)))
                .collect(),
        ),
        other => other,
    }
}

fn private_setting_event(event: AdditionEvent) -> AdditionEvent {
    let value = pulse_store::gojson::to_vec(&event)
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .expect("addition event to value");
    let value = crate::privacy::private_setting_event(value);
    let mut event: AdditionEvent =
        serde_json::from_value(value).expect("addition event from value");
    event.detail = go_any_numbers(event.detail);
    event
}

fn send_sse(w: &mut ResponseWriter, event: &AdditionEvent) -> bool {
    let data = match pulse_store::gojson::to_string(event) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let action = event.action.replace(['\r', '\n'], "");
    let text = format!("id: {}\nevent: {}\ndata: {}\n\n", event.id, action, data);
    w.write(text.as_bytes()).is_ok() && w.flush().is_ok()
}

fn sql_json(v: SqlValue) -> Value {
    match v {
        SqlValue::Null => Value::Null,
        SqlValue::Integer(n) => json!(n),
        SqlValue::Real(n) => json!(n),
        SqlValue::Text(s) => Value::String(s),
        SqlValue::Blob(b) => Value::String(String::from_utf8_lossy(&b).to_string()),
    }
}

pub(crate) fn value_from<T: Serialize>(v: &T) -> Value {
    let text = pulse_store::gojson::to_string(v).expect("serialize value");
    serde_json::from_str(&text).expect("serialized JSON")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers_http as http;
    use serde_json::{Value, json};
    use std::io::{BufRead, BufReader};
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn arr<'a>(m: &'a serde_json::Map<String, Value>, key: &str) -> &'a Vec<Value> {
        m[key].as_array().expect("array")
    }

    #[test]
    fn additions_store_with_runner_environment() {
        let _serial = crate::test_helpers::serial();
        let _job = pulse_testutil::setenv("PULSE_JOB", "parent");
        let _agent = pulse_testutil::setenv("PULSE_AGENT", "parent");
        let _session = pulse_testutil::setenv("PULSE_SESSION", "parent");
        let _project = pulse_testutil::setenv("PULSE_PROJECT", "parent");
        {
            let fixture = http::additions_store(false);
            assert_eq!(std::env::var("PULSE_JOB").unwrap_or_default(), "");
            assert_eq!(std::env::var("PULSE_AGENT").unwrap_or_default(), "");
            assert_eq!(std::env::var("PULSE_SESSION").unwrap_or_default(), "");
            assert_eq!(std::env::var("PULSE_PROJECT").unwrap_or_default(), "");
            drop(fixture);
        }
    }

    #[test]
    fn additions_records() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        let (status, result) =
            http::additions_request(http::handler(&s), "GET", "/api/v1/records", "");
        assert_eq!(status, 200);
        assert_eq!(arr(&result, "records").len(), 0);
        let mut rec = http::additions_record(&s, "ADR-001", "one", "adr", "first", "First body");
        http::additions_record(&s, "MEM-other", "two", "memory", "other", "Other body");
        rec.status = "Accepted".to_string();
        s.put_record(
            rec.clone(),
            std::collections::BTreeMap::from([("status".to_string(), json!("Accepted"))]),
            "Second body",
            "human",
            "revised",
        )
        .unwrap();
        let (status, result) = http::additions_request(
            http::handler(&s),
            "GET",
            "/api/v1/records?project=one&kind=adr&status=accepted&area=hub",
            "",
        );
        let rows = arr(&result, "records");
        assert_eq!(status, 200);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], rec.id);
        assert_eq!(rows[0].as_object().unwrap().len(), 16);
        assert!(rows[0].get("body").is_none());
        let (status, result) = http::additions_request(
            http::handler(&s),
            "GET",
            "/api/v1/records/ADR-001?rev=1",
            "",
        );
        assert_eq!(status, 200);
        assert_eq!(result["body"], "First body");
        assert_eq!(arr(&result, "revisions").len(), 2);
        let (status, result) =
            http::additions_request(http::handler(&s), "GET", "/api/v1/records/ADR-001", "");
        assert_eq!(status, 200);
        assert_eq!(result["body"], "Second body");
        for path in [
            "/api/v1/records/unknown",
            "/api/v1/records/ADR-001?rev=9",
            "/api/v1/records/ADR-001?project=two",
        ] {
            let (status, _) = http::additions_request(http::handler(&s), "GET", path, "");
            assert_eq!(status, 404, "{path}");
        }
        for rev in ["-1", "x", "1.5"] {
            let (status, result) = http::additions_request(
                http::handler(&s),
                "GET",
                &format!("/api/v1/records/ADR-001?rev={rev}"),
                "",
            );
            assert_eq!(status, 400);
            assert!(result.get("why").is_some());
            assert!(result.get("fix").is_some());
        }
    }

    #[test]
    fn additions_constitution() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        http::additions_project(&s, "one");
        let (status, result) = http::additions_request(
            http::handler(&s),
            "GET",
            "/api/v1/constitution?project=one",
            "",
        );
        assert_eq!(status, 200);
        assert!(result["record"].is_null());
        assert_eq!(result["body"], "");
        assert_eq!(arr(&result, "amendments").len(), 0);
        http::additions_record(
            &s,
            "EVO-constitution",
            "one",
            "evolution",
            "constitution",
            "## Purpose\nImprove carefully.",
        );
        let (status, result) = http::additions_request(
            http::handler(&s),
            "GET",
            "/api/v1/constitution?project=one",
            "",
        );
        assert_eq!(status, 200);
        assert_eq!(result["body"], "## Purpose\nImprove carefully.");
        let (status, _) = http::additions_request(
            http::handler(&s),
            "GET",
            "/api/v1/constitution?project=two",
            "",
        );
        assert_eq!(status, 404);
    }

    #[test]
    fn additions_specific_missing_resource_errors() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        for (path, message) in [
            ("/api/v1/records/missing", "no such record"),
            ("/api/v1/tasks/TASK-999", "No task \"TASK-999\""),
            ("/api/v1/constitution?project=missing", "no such project"),
        ] {
            let (status, result) = http::additions_request(http::handler(&s), "GET", path, "");
            assert_eq!(status, 404, "{path}");
            assert_eq!(result["error"], message);
            assert_ne!(result["why"], "");
            assert_ne!(result["fix"], "");
        }
    }

    #[test]
    fn additions_protected_sections() {
        let _serial = crate::test_helpers::serial();
        for (section, status, body, want) in [
            ("Hard stops", "needs votes", "", true),
            ("Purpose|Kill switch", "closed: accepted", "", true),
            ("Purpose", "passed: ready", "", false),
            (
                "Purpose",
                "needs votes",
                "| Not amendable by vote | `Purpose`, Limits |",
                true,
            ),
            ("", "protected section \"Limits\"", "", true),
        ] {
            let got = addition_protected(
                &JsonMap::from([
                    ("section".to_string(), json!(section)),
                    ("status".to_string(), json!(status)),
                ]),
                body,
            );
            assert_eq!(got, want, "{section} {status}");
        }
    }

    #[test]
    fn additions_unavailable_and_methods() {
        let _serial = crate::test_helpers::serial();
        for path in [
            "tasks",
            "messages",
            "compute",
            "records",
            "constitution",
            "events",
            "events/stream",
        ] {
            let (status, result) = http::additions_request(
                crate::new_handler(None, None),
                "GET",
                &format!("/api/v1/{path}"),
                "",
            );
            assert_eq!(status, 503, "{path}");
            assert!(result.get("error").is_some());
            assert!(result.get("why").is_some());
            assert!(result.get("fix").is_some());
        }
        let s = http::additions_store(true);
        let (status, result) =
            http::additions_request(http::handler(&s), "GET", "/api/v1/tasks", "");
        assert_eq!(status, 503);
        assert_eq!(result["error"], "tasks are not available yet");
        let (status, _) =
            http::additions_request(http::handler(&s), "POST", "/api/v1/events", "{}");
        assert_eq!(status, 405);
    }

    #[test]
    fn additions_events_polling() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        s.event(
            "human",
            "message.sent",
            "one-msg",
            json!({"project":"one","body":"hello"}),
        )
        .unwrap();
        s.event("human", "message.sent", "two-msg", json!({"project":"two"}))
            .unwrap();
        let (status, result) = http::additions_request(
            http::handler(&s),
            "GET",
            "/api/v1/events?project=one&limit=1",
            "",
        );
        assert_eq!(status, 200);
        let events = arr(&result, "events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["subject"], "one-msg");
        assert_eq!(events[0]["detail"]["body"], "hello");
        for subject in ["one-3", "one-4", "one-5"] {
            s.event("human", "message.sent", subject, json!({"project":"one"}))
                .unwrap();
        }
        let (status, result) = http::additions_request(
            http::handler(&s),
            "GET",
            "/api/v1/events?project=one&limit=2",
            "",
        );
        assert_eq!(status, 200);
        assert_eq!(
            http::event_subjects(arr(&result, "events")).join(","),
            "one-4,one-5"
        );
        for query in ["since=-1", "since=no", "limit=0", "limit=1001", "limit=2.5"] {
            let (status, _) = http::additions_request(
                http::handler(&s),
                "GET",
                &format!("/api/v1/events?{query}"),
                "",
            );
            assert_eq!(status, 400, "{query}");
        }
    }

    #[test]
    fn additions_events_stream_starts_at_the_head_without_a_cursor() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        let h = Arc::new(http::handler(&s));
        let server =
            crate::test_helpers::TestServer::start(Arc::new(move |w, r| h.serve_http(w, r)));
        for i in 0..5 {
            s.event(
                "human",
                "message.sent",
                &format!("history-{i}"),
                json!({"project":"one"}),
            )
            .unwrap();
        }
        let response = crate::test_helpers::client()
            .get(format!("{}/api/v1/events/stream?project=one", server.url))
            .send()
            .unwrap();
        assert_eq!(response.status(), 200);
        s.event(
            "human",
            "message.sent",
            "after-connection",
            json!({"project":"one"}),
        )
        .unwrap();
        let mut reader = BufReader::new(response);
        let mut line = String::new();
        loop {
            line.clear();
            reader.read_line(&mut line).unwrap();
            if line.starts_with("data: ") {
                assert!(line.contains("\"subject\":\"after-connection\""), "{line}");
                break;
            }
        }
    }

    #[test]
    fn additions_events_stream_resume_replays_the_newest_window() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        let cursor: i64 = s
            .conn()
            .unwrap()
            .query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |r| r.get(0))
            .unwrap();
        for subject in ["missed-1", "missed-2", "missed-3", "missed-4"] {
            s.event("human", "message.sent", subject, json!({"project":"one"}))
                .unwrap();
        }
        let (status, result) = http::additions_request(
            http::handler(&s),
            "GET",
            &format!("/api/v1/events?project=one&since={cursor}&limit=2"),
            "",
        );
        assert_eq!(status, 200);
        assert_eq!(
            http::event_subjects(arr(&result, "events")).join(","),
            "missed-3,missed-4"
        );
    }

    #[test]
    fn additions_events_stream_after_connection() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        let h = std::sync::Arc::new(http::handler(&s));
        let server = crate::test_helpers::TestServer::start(std::sync::Arc::new(move |w, r| {
            h.serve_http(w, r)
        }));
        s.event(
            "human",
            "message.sent",
            "before-connection",
            json!({"project":"one"}),
        )
        .unwrap();
        let last: i64 = s
            .conn()
            .unwrap()
            .query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |r| r.get(0))
            .unwrap();
        let response = crate::test_helpers::client()
            .get(format!(
                "{}/api/v1/events/stream?project=one&kind=message.sent&subject=included",
                server.url
            ))
            .header("Last-Event-ID", last.to_string())
            .send()
            .unwrap();
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(
            response.headers().get("Content-Type").unwrap(),
            "text/event-stream"
        );
        s.event(
            "human",
            "message.sent",
            "excluded",
            json!({"project":"two"}),
        )
        .unwrap();
        s.event(
            "human",
            "task.amended",
            "included",
            json!({"project":"one"}),
        )
        .unwrap();
        s.event(
            "human",
            "message.sent",
            "other-subject",
            json!({"project":"one"}),
        )
        .unwrap();
        s.event(
            "human",
            "message.sent",
            "included",
            json!({"project":"one"}),
        )
        .unwrap();
        let mut reader = std::io::BufReader::new(response);
        let mut frame = String::new();
        loop {
            let mut line = String::new();
            std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
            frame.push_str(&line);
            if line == "\n" {
                let text = frame.clone();
                if text.contains("data: ") {
                    assert!(text.contains("event: message.sent\n"));
                    assert!(text.contains("\"subject\":\"included\""));
                    assert!(!text.contains("excluded"));
                    assert!(text.contains("id: "));
                    break;
                }
                frame.clear();
            }
        }
    }

    #[test]
    fn additions_stream_heartbeat() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        let h = std::sync::Arc::new(http::handler(&s));
        let server = crate::test_helpers::TestServer::start(std::sync::Arc::new(move |w, r| {
            h.serve_http(w, r)
        }));
        let cursor: i64 = s
            .conn()
            .unwrap()
            .query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |r| r.get(0))
            .unwrap();
        let response = crate::test_helpers::client()
            .get(format!(
                "{}/api/v1/events/stream?since={}",
                server.url, cursor
            ))
            .header("Last-Event-ID", "0")
            .timeout(std::time::Duration::from_secs(18))
            .send()
            .unwrap();
        let mut reader = std::io::BufReader::new(response);
        loop {
            let mut line = String::new();
            std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
            if line == ": heartbeat\n" {
                break;
            }
            assert!(
                !line.starts_with("data:"),
                "stream replayed an event preceding its query cursor: {line}"
            );
        }
    }

    #[test]
    fn additions_tasks_and_messages() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        http::additions_project(&s, "one");
        http::additions_project(&s, "two");
        let (status, result) =
            http::additions_request(http::handler(&s), "GET", "/api/v1/tasks", "");
        assert_eq!(status, 200);
        assert_eq!(arr(&result, "tasks").len(), 0);
        let (status, result) = http::additions_request(
            http::handler(&s),
            "POST",
            "/api/v1/tasks",
            r#"{"project":"one","title":"Owner task","brief":"Build the thing","priority":3}"#,
        );
        assert!(status == 200 || status == 201);
        let task = &result["task"];
        let id = task["id"].as_str().unwrap().to_string();
        assert_eq!(task["created_by"], "human");
        assert_eq!(task["brief"], "Build the thing");
        assert_eq!(task["current_rev"], 1);
        assert_eq!(task["priority"], 3);
        assert_eq!(task["jobs_total"], 0);
        assert_eq!(task["working"].as_array().unwrap().len(), 0);
        let (status, result) = http::additions_request(
            http::handler(&s),
            "POST",
            &format!("/api/v1/tasks/{id}/amend"),
            r#"{"brief":"Build the amended thing","note":"Owner clarification"}"#,
        );
        assert_eq!(status, 200);
        assert_eq!(result["task"]["title"], "Owner task");
        assert_eq!(result["task"]["current_rev"], 2);
        let (status, result) = http::additions_request(
            http::handler(&s),
            "POST",
            "/api/v1/messages",
            &format!(r#"{{"project":"one","task_id":"{id}","body":"How is it going?"}}"#),
        );
        assert!(status == 200 || status == 201);
        assert_eq!(result["message"]["author"], "human");
        assert_eq!(result["message"]["body"], "How is it going?");
        // The task's jobs: its plain source, its labelled sources, not a
        // longer task id that shares the prefix.
        for (job, source) in [
            ("job-plain", format!("task:{id}")),
            ("job-ui", format!("task:{id}:hub-ui")),
            ("job-near", format!("task:{id}0")),
        ] {
            s.store
                .conn()
                .unwrap()
                .execute(
                    "INSERT INTO jobs (id, project, kind, title, source, status, created_at) VALUES (?, 'one', 'task', 'T', ?, 'queued', '2026-01-01T00:00:00Z')",
                    rusqlite::params![job, source],
                )
                .unwrap();
        }
        let (status, result) =
            http::additions_request(http::handler(&s), "GET", &format!("/api/v1/tasks/{id}"), "");
        assert_eq!(status, 200);
        assert_eq!(arr(&result, "revisions").len(), 2);
        assert_eq!(arr(&result, "messages").len(), 1);
        let jobs = arr(&result, "jobs")
            .iter()
            .map(|j| j["id"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>();
        assert_eq!(jobs, vec!["job-plain", "job-ui"], "{result:?}");
        let (status, result) = http::additions_request(
            http::handler(&s),
            "POST",
            &format!("/api/v1/tasks/{id}/cancel"),
            r#"{"reason":"Priorities changed"}"#,
        );
        assert_eq!(status, 200);
        assert_eq!(result["task"]["status"], "cancelled");
    }

    #[test]
    fn additions_write_validation() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        http::additions_project(&s, "one");
        http::additions_project(&s, "two");
        for (path, body) in [
            ("tasks", r#"{"project":"one","title":1,"brief":"text"}"#),
            ("tasks", r#"{"project":"one","title":"text","brief":false}"#),
            (
                "tasks",
                r#"{"project":"one","title":"text","brief":"text","priority":1.5}"#,
            ),
            (
                "tasks",
                r#"{"project":"one","title":"text","brief":"text","priority":"3"}"#,
            ),
            ("tasks", r#"{"project":"one","title":"","brief":"text"}"#),
            ("messages", r#"{"project":"one","body":null}"#),
            (
                "messages",
                r#"{"project":"one","task_id":123,"body":"text"}"#,
            ),
            ("tasks", "[]"),
            ("tasks", r#"{"project":"one""#),
        ] {
            let (status, result) = http::additions_request(
                http::handler(&s),
                "POST",
                &format!("/api/v1/{path}"),
                body,
            );
            assert_eq!(status, 400, "{path} {body}");
            assert!(result.get("error").is_some());
            assert!(result.get("why").is_some());
            assert!(result.get("fix").is_some());
        }
    }

    #[test]
    fn additions_message_cursor_same_second() {
        let _serial = crate::test_helpers::serial();
        let s = http::additions_store(false);
        http::additions_project(&s, "one");
        let mut ninth_id = String::new();
        for i in 1..=12 {
            let (status, result) = http::additions_request(
                http::handler(&s),
                "POST",
                "/api/v1/messages",
                &format!(r#"{{"project":"one","body":"Message {i}"}}"#),
            );
            assert!(status == 200 || status == 201);
            if i == 9 {
                ninth_id = result["message"]["id"].as_str().unwrap().to_string();
            }
        }
        let (status, result) = http::additions_request(
            http::handler(&s),
            "GET",
            &format!("/api/v1/messages?project=one&since={ninth_id}"),
            "",
        );
        assert_eq!(status, 200);
        let messages = arr(&result, "messages");
        assert_eq!(messages.len(), 3);
        for (i, message) in messages.iter().enumerate() {
            assert_eq!(message["body"], format!("Message {}", i + 10));
        }
    }

    #[test]
    fn additions_concurrent_tasks() {
        let _serial = crate::test_helpers::serial();
        let s = Arc::new(http::additions_store(false));
        http::additions_project(&s, "one");
        let barrier = Arc::new(Barrier::new(6));
        let mut joins = Vec::new();
        for i in 0..6 {
            let s = s.clone();
            let barrier = barrier.clone();
            joins.push(thread::spawn(move || {
                barrier.wait();
                let body = format!(r#"{{"project":"one","title":"Task {i}","brief":"Build it"}}"#);
                let (status, result) =
                    http::additions_request(http::handler(&s), "POST", "/api/v1/tasks", &body);
                assert!(status == 200 || status == 201, "{status} {result:?}");
            }));
        }
        for join in joins {
            join.join().unwrap();
        }
        let count: i64 = s
            .conn()
            .unwrap()
            .query_row("SELECT count(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 6);
    }

    #[test]
    fn additions_compute() {
        let _serial = crate::test_helpers::serial();
        crate::test_helpers::test_main_setup();
        let s = http::additions_store(false);
        http::additions_project(&s, "one");
        let (status, result) =
            http::additions_request(http::handler(&s), "GET", "/api/v1/compute", "");
        assert_eq!(status, 200);
        assert_eq!(arr(&result, "requests").len(), 0);
        let conn = s.conn().unwrap();
        for id in ["CMP-001", "CMP-002"] {
            conn.execute("INSERT INTO compute_requests(id,project,requested_by,size,hours,reason,status,decided_by,executor,at,updated_at) VALUES(?,'one','one-lead','Standard_B2ms',1,'More capacity','pending','','',?,?)",
                rusqlite::params![id, s.now(), s.now()]).unwrap();
        }
        drop(conn);
        for (key, value) in [
            ("compute.max_vms", "3"),
            ("compute.max_hours_per_day", "10"),
            ("compute.budget_credits", "100"),
        ] {
            s.set_setting("human", key, value).unwrap();
        }
        for (i, action) in ["approve", "deny"].iter().enumerate() {
            let (status, result) = http::additions_request(
                http::handler(&s),
                "POST",
                &format!("/api/v1/compute/CMP-00{}/{}", i + 1, action),
                r#"{"words":"Owner decision"}"#,
            );
            assert_eq!(status, 200);
            let want = if *action == "deny" {
                "denied"
            } else {
                "approved"
            };
            assert_eq!(result["request"]["status"], want);
            assert_eq!(result["request"]["decided_by"], "human");
        }
        let (status, _) = http::additions_request(
            http::handler(&s),
            "POST",
            "/api/v1/compute/CMP-001/deny",
            r#"{"words":"Already decided"}"#,
        );
        assert_eq!(status, 409);
    }

    #[test]
    fn additions_concurrent_events() {
        let _serial = crate::test_helpers::serial();
        let s = Arc::new(http::additions_store(false));
        let mut joins = Vec::new();
        for i in 0..6 {
            let s1 = s.clone();
            joins.push(thread::spawn(move || {
                s1.event(
                    "human",
                    "message.sent",
                    &i.to_string(),
                    json!({"project":"one"}),
                )
                .unwrap();
            }));
            let s2 = s.clone();
            joins.push(thread::spawn(move || {
                let (status, _) = http::additions_request(
                    http::handler(&s2),
                    "GET",
                    "/api/v1/events?project=one",
                    "",
                );
                assert_eq!(status, 200);
            }));
        }
        for join in joins {
            join.join().unwrap();
        }
    }
}
