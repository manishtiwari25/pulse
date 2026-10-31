//! `/api/v1/chat`: the hub's conversations - ask an agent to work, watch
//! it, answer its questions, see what it made.
//!
//! The agents run inside the hub process (one worker thread per
//! conversation with a turn in flight) through
//! [`pulse_harness::conversation::Manager`]; this module is the HTTP skin:
//! rows and messages from the store, live state from the manager, steps
//! from the harness session the conversation currently runs, the
//! artifacts folder as files, the branch's changes from git.
use crate::additions::{
    ADDITION_WRITES, addition_failure, addition_method, addition_not_found, addition_string,
};
use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, JsonMap, write_error, write_json};
use hyper::header::{self, HeaderValue};
use pulse_harness::conversation::{ImageAttachment, Manager};
use pulse_store::Conversation;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

/// The manager, built on first use from the handler's store; tests seed it
/// with a pinned provider.
pub(crate) type Chats = Arc<Mutex<Option<Arc<Manager>>>>;

/// The most bytes one artifact may be served at.
const ARTIFACT_LIMIT: u64 = 20 * 1024 * 1024;
/// How many conversations a list returns.
const LIST_LIMIT: i64 = 200;
/// How large a `/message` body may be: every inline image a message may
/// carry at its largest, base64 (4/3), plus the text.
const MESSAGE_BODY_LIMIT: usize = pulse_harness::conversation::ATTACHMENTS_PER_MESSAGE
    * pulse_harness::conversation::IMAGE_LIMIT
    / 3
    * 4
    + crate::server::JSON_BODY_LIMIT;

impl Handler {
    pub(crate) fn chats(&self) -> Option<Arc<Manager>> {
        self.open_chats().map(|(m, _, _)| m)
    }

    /// The manager, built now when it was not yet, with the ids of the
    /// conversations whose turn the last hub left cut and of those whose
    /// turn it stopped to restart and that carry on now (both empty when
    /// the manager already existed). A new manager holds no runtime, so
    /// rows that still say "running" are the last hub's: they go idle with
    /// a note before anything is served.
    pub(crate) fn open_chats(&self) -> Option<(Arc<Manager>, Vec<String>, Vec<String>)> {
        let store = self.store.as_deref()?;
        let mut slot = self.chats.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(m) = slot.as_ref() {
            return Some((m.clone(), Vec::new(), Vec::new()));
        }
        let manager = Manager::new(&store.home, store.clone());
        let cut = manager.recover();
        let carried = manager.carry_on_restarted();
        let manager = Arc::new(manager);
        *slot = Some(manager.clone());
        Some((manager, cut, carried))
    }

    pub(crate) fn addition_chat(&self, w: &mut ResponseWriter, r: &Request, parts: Vec<String>) {
        if !self.addition_available(w, r, vec!["conversations".to_string()]) {
            return;
        }
        let Some(chats) = self.chats() else {
            addition_not_found(w);
            return;
        };
        match parts.as_slice() {
            [_] => {
                if !addition_method(w, r, vec!["GET".to_string(), "POST".to_string()]) {
                    return;
                }
                if r.method == "POST" {
                    self.chat_create(w, r, &chats);
                } else {
                    self.chat_list(w, r, &chats);
                }
            }
            [_, id] => {
                if addition_method(w, r, vec!["GET".to_string()]) {
                    self.chat_show(w, r, &chats, id);
                }
            }
            [_, id, action] if action == "diff" => {
                if addition_method(w, r, vec!["GET".to_string()]) {
                    self.chat_diff(w, &chats, id);
                }
            }
            [_, id, action] if action == "upload" => {
                if addition_method(w, r, vec!["POST".to_string()]) {
                    self.chat_upload(w, r, &chats, id);
                }
            }
            [_, id, action] => {
                if addition_method(w, r, vec!["POST".to_string()]) {
                    self.chat_action(w, r, &chats, id, action);
                }
            }
            [_, id, kind, name] if kind == "artifacts" || kind == "attachments" => {
                if addition_method(w, r, vec!["GET".to_string(), "HEAD".to_string()]) {
                    self.chat_file(w, r, &chats, id, kind, name);
                }
            }
            [_, id, kind, message_n, n] if kind == "image" => {
                if addition_method(w, r, vec!["GET".to_string(), "HEAD".to_string()]) {
                    self.chat_image(w, r, &chats, id, message_n, n);
                }
            }
            _ => addition_not_found(w),
        }
    }

    fn chat_list(&self, w: &mut ResponseWriter, r: &Request, chats: &Manager) {
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let all = matches!(r.query("all").as_str(), "1" | "true" | "yes");
        let rows = match store.chat_list(all, LIST_LIMIT) {
            Ok(rows) => rows,
            Err(e) => {
                addition_failure(w, e);
                return;
            }
        };
        let items: Vec<Value> = rows
            .iter()
            .map(|row| conversation_json(row, chats))
            .collect();
        // The tasks the page groups conversations under (chat_tasks); an
        // older store without the task tables lists none.
        let tasks = match crate::chat_tasks::task_groups(store, all) {
            Ok(tasks) => tasks,
            Err(e) => {
                addition_failure(w, e);
                return;
            }
        };
        write_json(w, 200, json!({"conversations": items, "tasks": tasks}));
    }

    fn chat_create(&self, w: &mut ResponseWriter, r: &Request, chats: &Manager) {
        let (payload, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let (text, ok) = addition_string(w, &payload, "text", true);
        if !ok {
            return;
        }
        let (title, ok) = addition_string(w, &payload, "title", false);
        if !ok {
            return;
        }
        let (mut project, ok) = addition_string(w, &payload, "project", false);
        if !ok {
            return;
        }
        if project.is_empty() {
            project = self.project.clone();
        }
        let (model, ok) = addition_string(w, &payload, "model", false);
        if !ok {
            return;
        }
        let (reasoning, ok) = addition_string(w, &payload, "reasoning", false);
        if !ok {
            return;
        }
        let Some(auto) = payload_bool(w, &payload, "auto_approve", true) else {
            return;
        };
        let (parent, ok) = addition_string(w, &payload, "parent", false);
        if !ok {
            return;
        }
        let _lock = ADDITION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
        // A sub-conversation of `parent` (the owner starting one from the
        // parent's page) takes the parent's workspace and settings; the
        // other fields are the parent's.
        let created = if parent.is_empty() {
            chats.create_with(
                &title,
                &project,
                &model,
                &reasoning,
                auto,
                &text,
                pulse_store::HUMAN_AGENT,
            )
        } else {
            chats.create_child(&parent, &title, &text)
        };
        match created {
            Ok(row) => write_json(
                w,
                201,
                json!({"conversation": conversation_json(&row, chats)}),
            ),
            Err(e) => addition_failure(w, e),
        }
    }

    fn chat_show(&self, w: &mut ResponseWriter, r: &Request, chats: &Manager, id: &str) {
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let Some(row) = self.chat_row(w, id) else {
            return;
        };
        // Messages count from 1 and steps from 0, so "everything" is
        // `since=0` for messages and `after=-1` for steps.
        let (since, ok) = query_int(w, r.query("since"), "since", 0, 0);
        if !ok {
            return;
        }
        let (after, ok) = query_int(w, r.query("after"), "after", -1, -1);
        if !ok {
            return;
        }
        let live = chats.live(&row);
        let session_id = if live.session_id.is_empty() {
            row.session_id.clone()
        } else {
            live.session_id.clone()
        };
        let messages = match store.chat_messages(id, since) {
            Ok(m) => m,
            Err(e) => {
                addition_failure(w, e);
                return;
            }
        };
        let steps = if session_id.is_empty() {
            Vec::new()
        } else {
            match store.harness_steps(&session_id, after) {
                Ok(s) => s,
                Err(e) => {
                    addition_failure(w, e);
                    return;
                }
            }
        };
        let artifacts = chats.artifacts(id);
        let changes = chats.changes(&row);
        // The family: the conversation this one is a sub-conversation of,
        // and its own sub-conversations (every one, put-away ones too,
        // each with its live state).
        let parent_title = if row.parent.is_empty() {
            String::new()
        } else {
            store
                .chat_get(&row.parent)
                .ok()
                .flatten()
                .map(|p| p.title)
                .unwrap_or_default()
        };
        let children: Vec<Value> = match store.chat_children(id, true) {
            Ok(rows) => rows.iter().map(|c| conversation_json(c, chats)).collect(),
            Err(e) => {
                addition_failure(w, e);
                return;
            }
        };
        write_json(
            w,
            200,
            json!({
                "conversation": row,
                "live": live,
                "session_id": session_id,
                "messages": messages,
                "since": since,
                "steps": steps,
                "after": after,
                "artifacts": artifacts,
                "changes": changes,
                "parent_title": parent_title,
                "children": children,
            }),
        );
    }

    fn chat_diff(&self, w: &mut ResponseWriter, chats: &Manager, id: &str) {
        let Some(row) = self.chat_row(w, id) else {
            return;
        };
        let diff = chats.diff(&row).unwrap_or_default();
        write_json(
            w,
            200,
            json!({"id": id, "branch": row.branch, "diff": diff}),
        );
    }

    /// `POST /api/v1/chat/ID/upload?name=FILE` with the file's bytes as the
    /// body: keeps it for a message to carry (`attachments` on `message`).
    fn chat_upload(&self, w: &mut ResponseWriter, r: &Request, chats: &Manager, id: &str) {
        if !crate::server::same_origin_write(w, r) {
            return;
        }
        if self.chat_row(w, id).is_none() {
            return;
        }
        let name = r.query("name");
        if name.trim().is_empty() {
            write_error(
                w,
                400,
                "Missing name".to_string(),
                "The upload needs the file's name in the query string.".to_string(),
                "Send POST /api/v1/chat/ID/upload?name=FILE with the bytes as the body."
                    .to_string(),
            );
            return;
        }
        if r.body.is_empty() || r.body.len() > pulse_harness::conversation::ATTACHMENT_LIMIT {
            write_error(
                w,
                413,
                "File too large or empty".to_string(),
                format!(
                    "The body is {} bytes; attachments go from 1 to {} bytes.",
                    r.body.len(),
                    pulse_harness::conversation::ATTACHMENT_LIMIT
                ),
                "Attach a smaller file, or put it in the project and name its path.".to_string(),
            );
            return;
        }
        let _lock = ADDITION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
        match chats.save_attachment(id, &name, &r.body) {
            Ok(a) => write_json(w, 201, json!({"attachment": a})),
            Err(e) => addition_failure(w, e),
        }
    }

    fn chat_action(
        &self,
        w: &mut ResponseWriter,
        r: &Request,
        chats: &Manager,
        id: &str,
        action: &str,
    ) {
        let limit = if action == "message" {
            MESSAGE_BODY_LIMIT
        } else {
            crate::server::JSON_BODY_LIMIT
        };
        let (payload, ok) = crate::server::read_payload_up_to(w, r, limit);
        if !ok {
            return;
        }
        let Some(row) = self.chat_row(w, id) else {
            return;
        };
        let _lock = ADDITION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
        let result = match action {
            "message" => {
                let (text, ok) = addition_string(w, &payload, "text", false);
                if !ok {
                    return;
                }
                let Some(attachments) = payload_names(w, &payload, "attachments") else {
                    return;
                };
                let Some(images) = payload_images(w, &payload, "images") else {
                    return;
                };
                if images.is_empty() {
                    chats.send_with(id, &text, &attachments)
                } else {
                    chats.send_with_images(id, &text, &attachments, &images)
                }
            }
            "answer" => {
                let (text, ok) = addition_string(w, &payload, "text", true);
                if !ok {
                    return;
                }
                chats.answer(id, &text)
            }
            "approve" => {
                let (decision, ok) = addition_string(w, &payload, "decision", true);
                if !ok {
                    return;
                }
                let (reason, ok) = addition_string(w, &payload, "reason", false);
                if !ok {
                    return;
                }
                chats.approve(id, decision.trim(), &reason)
            }
            "cancel" => chats.cancel(id),
            "archive" => chats.archive(id),
            "close" => chats.close(id),
            "auto" => {
                let Some(on) = payload_bool(w, &payload, "on", true) else {
                    return;
                };
                chats.set_auto_approve(id, on)
            }
            "attach" => {
                let (project, ok) = addition_string(w, &payload, "project", true);
                if !ok {
                    return;
                }
                match chats.attach(id, project.trim()) {
                    Ok(attached) => {
                        if let Some(row) = self.chat_row(w, id) {
                            write_json(
                                w,
                                200,
                                json!({"conversation": conversation_json(&row, chats), "attached": attached}),
                            );
                        }
                        return;
                    }
                    Err(e) => Err(e),
                }
            }
            "title" => {
                let (title, ok) = addition_string(w, &payload, "title", true);
                if !ok {
                    return;
                }
                chats.set_title(id, title.trim())
            }
            "model" => {
                let (model, ok) = addition_string(w, &payload, "model", false);
                if !ok {
                    return;
                }
                chats.set_model(id, &model)
            }
            "reasoning" => {
                let (reasoning, ok) = addition_string(w, &payload, "reasoning", false);
                if !ok {
                    return;
                }
                chats.set_reasoning(id, &reasoning)
            }
            _ => {
                addition_not_found(w);
                return;
            }
        };
        let _ = row;
        match result {
            Ok(()) => {
                if let Some(row) = self.chat_row(w, id) {
                    write_json(
                        w,
                        200,
                        json!({"conversation": conversation_json(&row, chats)}),
                    );
                }
            }
            Err(e) => addition_failure(w, e),
        }
    }

    fn chat_file(
        &self,
        w: &mut ResponseWriter,
        r: &Request,
        chats: &Manager,
        id: &str,
        kind: &str,
        name: &str,
    ) {
        if self.chat_row(w, id).is_none() {
            return;
        }
        let path = if kind == "attachments" {
            chats.attachment_path(id, name)
        } else {
            chats.artifact_path(id, name)
        };
        let Some(path) = path else {
            let what = if kind == "attachments" {
                "attachment"
            } else {
                "artifact"
            };
            write_error(
                w,
                404,
                format!("No such {what}"),
                format!("{name} is not a file in this conversation's {kind} folder."),
                "Open the conversation and pick a file from its list.".to_string(),
            );
            return;
        };
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size > ARTIFACT_LIMIT {
            write_error(
                w,
                413,
                "Artifact too large".to_string(),
                format!(
                    "{name} is {size} bytes; the hub serves files up to {ARTIFACT_LIMIT} bytes."
                ),
                format!("Open it from disk: {}", path.display()),
            );
            return;
        }
        let Ok(data) = std::fs::read(&path) else {
            addition_not_found(w);
            return;
        };
        let ext = path
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        let mut mime = crate::httpx::mime_type_by_extension(&ext);
        if mime.is_empty() {
            mime = if looks_like_text(&data) {
                "text/plain; charset=utf-8".to_string()
            } else {
                "application/octet-stream".to_string()
            };
        }
        if let Ok(v) = HeaderValue::from_str(&mime) {
            w.header_mut().insert(header::CONTENT_TYPE, v);
        }
        w.header_mut().insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        w.header_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        // The chat page frames artifacts (an HTML page in a sandboxed
        // iframe, a PDF in the browser's viewer), so the hub's blanket
        // "never frame" headers give way to same-origin framing here. A page
        // an agent wrote still runs with no access to the hub's origin; a
        // PDF keeps its viewer (CSP `sandbox` would switch plugins off).
        w.header_mut().insert(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("SAMEORIGIN"),
        );
        let csp = if mime.starts_with("text/html") || mime.contains("svg") {
            "sandbox allow-scripts"
        } else {
            "default-src 'none'; style-src 'unsafe-inline'"
        };
        w.header_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(csp),
        );
        crate::httpx::serve_content(w, r, name, &data);
    }

    /// One image of one message: `GET /api/v1/chat/{id}/image/{n}/{i}` is
    /// the `i`-th image of message `n`, served with the mime it arrived
    /// with. Nothing to delete: images go with the conversation.
    fn chat_image(
        &self,
        w: &mut ResponseWriter,
        r: &Request,
        chats: &Arc<Manager>,
        id: &str,
        message_n: &str,
        n: &str,
    ) {
        let Ok(message_n) = message_n.parse::<i64>() else {
            addition_not_found(w);
            return;
        };
        let Ok(n) = n.parse::<usize>() else {
            addition_not_found(w);
            return;
        };
        match chats.image(id, message_n, n) {
            Ok(Some((mime, data))) => {
                if let Ok(v) = HeaderValue::from_str(&mime) {
                    w.header_mut().insert(header::CONTENT_TYPE, v);
                }
                w.header_mut().insert(
                    header::X_CONTENT_TYPE_OPTIONS,
                    HeaderValue::from_static("nosniff"),
                );
                w.header_mut()
                    .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                crate::httpx::serve_content(w, r, "image", &data);
            }
            Ok(None) => addition_not_found(w),
            Err(e) => addition_failure(w, e.to_string()),
        }
    }

    fn chat_row(&self, w: &mut ResponseWriter, id: &str) -> Option<Conversation> {
        let store = self.store.as_deref()?;
        match store.chat_get(id) {
            Ok(Some(row)) => Some(row),
            Ok(None) => {
                write_error(
                    w,
                    404,
                    "No such conversation".to_string(),
                    format!("There is no conversation {id}."),
                    "Pick one from the list.".to_string(),
                );
                None
            }
            Err(e) => {
                addition_failure(w, e);
                None
            }
        }
    }
}

/// A row with its live state folded in.
fn conversation_json(row: &Conversation, chats: &Manager) -> Value {
    let live = chats.live(row);
    let mut v = serde_json::to_value(row).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut v {
        map.insert(
            "live".to_string(),
            serde_json::to_value(&live).unwrap_or(Value::Null),
        );
    }
    v
}

fn payload_bool(
    w: &mut ResponseWriter,
    payload: &JsonMap,
    key: &str,
    fallback: bool,
) -> Option<bool> {
    match payload.get(key) {
        None | Some(Value::Null) => Some(fallback),
        Some(Value::Bool(b)) => Some(*b),
        Some(Value::String(s)) => match s.trim() {
            "true" | "1" | "yes" | "on" => Some(true),
            "false" | "0" | "no" | "off" => Some(false),
            _ => {
                invalid_bool(w, key);
                None
            }
        },
        Some(Value::Number(n)) => Some(n.as_i64().unwrap_or(0) != 0),
        _ => {
            invalid_bool(w, key);
            None
        }
    }
}

fn invalid_bool(w: &mut ResponseWriter, key: &str) {
    write_error(
        w,
        400,
        format!("Invalid {key}"),
        format!("{key} must be true or false."),
        format!("Send {key} as a JSON boolean."),
    );
}

/// An optional list of file names under `key` (a JSON array of strings).
fn payload_names(w: &mut ResponseWriter, payload: &JsonMap, key: &str) -> Option<Vec<String>> {
    match payload.get(key) {
        None | Some(Value::Null) => Some(Vec::new()),
        Some(Value::Array(items)) => {
            let mut out = Vec::new();
            for item in items {
                match item.as_str() {
                    Some(s) if !s.trim().is_empty() => out.push(s.trim().to_string()),
                    _ => {
                        invalid_names(w, key);
                        return None;
                    }
                }
            }
            Some(out)
        }
        _ => {
            invalid_names(w, key);
            None
        }
    }
}

fn invalid_names(w: &mut ResponseWriter, key: &str) {
    write_error(
        w,
        400,
        format!("Invalid {key}"),
        format!("{key} must be a list of file names."),
        format!("Send {key} as a JSON array of the names the upload returned."),
    );
}

/// The `images` of a message body: a list of `{mime, data}` with the bytes
/// base64. Missing means none. `None` back (after an error is written) when
/// the shape is wrong.
fn payload_images(
    w: &mut ResponseWriter,
    payload: &JsonMap,
    key: &str,
) -> Option<Vec<ImageAttachment>> {
    match payload.get(key) {
        None | Some(Value::Null) => Some(Vec::new()),
        Some(v) => match serde_json::from_value::<Vec<ImageAttachment>>(v.clone()) {
            Ok(images) => Some(images),
            Err(e) => {
                write_error(
                    w,
                    400,
                    format!("Invalid {key}"),
                    format!(
                        "{key} must be a list of images, each a {{mime, data}} with the bytes base64: {e}"
                    ),
                    format!(
                        "Send {key} as a JSON array like [{{\"mime\": \"image/png\", \"data\": \"<base64>\"}}]."
                    ),
                );
                None
            }
        },
    }
}

fn query_int(
    w: &mut ResponseWriter,
    value: String,
    name: &str,
    default: i64,
    min: i64,
) -> (i64, bool) {
    if value.is_empty() {
        return (default, true);
    }
    match value.parse::<i64>() {
        Ok(n) if n >= min => (n, true),
        _ => {
            write_error(
                w,
                400,
                format!("Invalid {name}"),
                format!("{name} must be a whole number of at least {min}."),
                format!("Use a valid {name} query parameter."),
            );
            (default, false)
        }
    }
}

fn looks_like_text(data: &[u8]) -> bool {
    let head = &data[..data.len().min(4096)];
    !head.contains(&0) && std::str::from_utf8(head).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers_http::{additions_project, additions_store, handler, json_body};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use std::time::Duration;

    fn fake_handler(
        store: &pulse_store::Store,
        script: Value,
    ) -> (Handler, pulse_testutil::SetenvGuard) {
        pulse_provider_fake::register();
        let path = std::path::Path::new(&store.home).join("script.json");
        std::fs::write(&path, json!({"replies": script}).to_string()).expect("script");
        let guard = pulse_testutil::setenv("PULSE_HARNESS_FAKE_SCRIPT", path.display().to_string());
        let h = handler(store);
        let manager = Manager::new(&store.home, store.clone()).with_provider("fake", "fake-1");
        *h.chats.lock().unwrap() = Some(Arc::new(manager));
        (h, guard)
    }

    fn call(h: &Handler, method: &str, path: &str, body: Value) -> (u16, JsonMap, String) {
        let body = if body.is_null() { json!({}) } else { body };
        crate::test_helpers_http::request(h.clone(), method, path, &json_body(body), &[])
    }

    fn wait_status(h: &Handler, id: &str, want: &[&str]) -> JsonMap {
        for _ in 0..400 {
            let (code, body, _) = call(h, "GET", &format!("/api/v1/chat/{id}"), Value::Null);
            assert_eq!(code, 200);
            let status = body["live"]["status"].as_str().unwrap_or("").to_string();
            let queued = body["live"]["queued"]
                .as_array()
                .map(|a| a.len())
                .unwrap_or(0);
            if want.contains(&status.as_str()) && queued == 0 {
                return body;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("conversation {id} never reached {want:?}");
    }

    #[test]
    fn create_show_message_and_archive_over_http() {
        let _env = pulse_testutil::env_lock();
        let ts = additions_store(false);
        let (h, _script) =
            fake_handler(&ts, json!([{"content": "Hi there."}, {"content": "Done."}]));
        let (code, body, raw) = call(
            &h,
            "POST",
            "/api/v1/chat",
            json!({"text": "hello agent", "title": ""}),
        );
        assert_eq!(code, 201, "{raw}");
        let id = body["conversation"]["id"].as_str().unwrap().to_string();
        assert_eq!(body["conversation"]["title"], "hello agent");
        assert!(body["conversation"]["live"]["status"].is_string());
        let shown = wait_status(&h, &id, &["idle"]);
        let roles: Vec<&str> = shown["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["user", "assistant"]);
        assert_eq!(shown["messages"][1]["content"], "Hi there.");
        assert!(!shown["session_id"].as_str().unwrap().is_empty());
        assert!(
            !shown["steps"].as_array().unwrap().is_empty(),
            "live steps come from the harness session"
        );
        assert_eq!(shown["after"], -1, "no after means every step, from n = 0");
        assert_eq!(shown["steps"][0]["n"], 0, "the first step is included");
        let (code, _, _) = call(
            &h,
            "GET",
            &format!("/api/v1/chat/{id}?after=-2"),
            Value::Null,
        );
        assert_eq!(code, 400);
        assert!(shown["changes"].is_null(), "no project, no branch");
        // Incremental fetch: only what is new.
        let (_, inc, _) = call(
            &h,
            "GET",
            &format!("/api/v1/chat/{id}?since=2&after=100"),
            Value::Null,
        );
        assert_eq!(inc["messages"].as_array().unwrap().len(), 0);
        assert_eq!(inc["steps"].as_array().unwrap().len(), 0);
        assert_eq!(inc["since"], 2);
        // A second message, then the list shows the conversation idle.
        let (code, _, raw) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/message"),
            json!({"text": "more"}),
        );
        assert_eq!(code, 200, "{raw}");
        let shown = wait_status(&h, &id, &["idle"]);
        assert_eq!(shown["messages"].as_array().unwrap().len(), 4);
        assert_eq!(shown["conversation"]["turns"], 2);
        let (code, list, _) = call(&h, "GET", "/api/v1/chat", Value::Null);
        assert_eq!(code, 200);
        let items = list["conversations"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["live"]["status"], "idle");
        // Title, then archive; archived rows leave the default list.
        let (code, _, _) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/title"),
            json!({"title": "Greetings"}),
        );
        assert_eq!(code, 200);
        let (code, body, _) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/archive"),
            Value::Null,
        );
        assert_eq!(code, 200);
        assert_eq!(body["conversation"]["status"], "archived");
        assert_eq!(body["conversation"]["title"], "Greetings");
        let (_, list, _) = call(&h, "GET", "/api/v1/chat", Value::Null);
        assert!(list["conversations"].as_array().unwrap().is_empty());
        let (_, list, _) = call(&h, "GET", "/api/v1/chat?all=1", Value::Null);
        assert_eq!(list["conversations"].as_array().unwrap().len(), 1);
        let (code, _, _) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/message"),
            json!({"text": "x"}),
        );
        assert_eq!(code, 409, "an archived conversation takes no messages");
        std::thread::sleep(Duration::from_millis(100));
    }

    #[test]
    fn questions_and_approvals_round_trip_over_http() {
        let _env = pulse_testutil::env_lock();
        let ts = additions_store(false);
        let (h, _script) = fake_handler(
            &ts,
            json!([
                {"tool": "question", "args": {"question": "Which colour?", "choices": ["red", "blue"]}},
                {"tool": "write", "args": {"path": "colour.txt", "content": "blue\n"}},
                {"content": "Saved blue."}
            ]),
        );
        let (code, body, _) = call(
            &h,
            "POST",
            "/api/v1/chat",
            json!({"text": "pick a colour", "auto_approve": false}),
        );
        assert_eq!(code, 201);
        let id = body["conversation"]["id"].as_str().unwrap().to_string();
        let shown = wait_status(&h, &id, &["waiting"]);
        assert_eq!(shown["live"]["pending"]["kind"], "question");
        assert_eq!(shown["live"]["pending"]["question"], "Which colour?");
        assert_eq!(shown["live"]["pending"]["choices"][1], "blue");
        let (code, _, raw) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/approve"),
            json!({"decision": "allow"}),
        );
        assert_eq!(code, 409, "no approval waits yet: {raw}");
        let (code, _, raw) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/answer"),
            json!({"text": "blue"}),
        );
        assert_eq!(code, 200, "{raw}");
        // Next stop: the write wants an approval.
        let mut shown = wait_status(&h, &id, &["waiting"]);
        for _ in 0..200 {
            if shown["live"]["pending"]["kind"] == "approval" {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
            shown = wait_status(&h, &id, &["waiting"]);
        }
        assert_eq!(shown["live"]["pending"]["kind"], "approval");
        assert_eq!(shown["live"]["pending"]["tool"], "write");
        let (code, _, _) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/approve"),
            json!({"decision": "allow"}),
        );
        assert_eq!(code, 200);
        let shown = wait_status(&h, &id, &["idle"]);
        let roles: Vec<&str> = shown["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(
            roles,
            ["user", "question", "answer", "approval", "assistant"]
        );
        assert_eq!(shown["messages"][3]["extra"]["decision"], "allow");
        let cwd = shown["conversation"]["cwd"].as_str().unwrap();
        assert_eq!(
            std::fs::read_to_string(std::path::Path::new(cwd).join("colour.txt")).unwrap(),
            "blue\n"
        );
        // Auto-approve flips on and sticks on the row.
        let (code, body, _) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/auto"),
            json!({"on": true}),
        );
        assert_eq!(code, 200);
        assert_eq!(body["conversation"]["auto_approve"], true);
        assert_eq!(body["conversation"]["live"]["auto_approve"], true);
        let (code, _, _) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/archive"),
            Value::Null,
        );
        assert_eq!(code, 200);
        std::thread::sleep(Duration::from_millis(100));
    }

    #[test]
    fn a_restart_waits_for_working_conversations_and_a_new_hub_recovers_cut_ones() {
        let _env = pulse_testutil::env_lock();
        let _serial = crate::test_helpers::serial();
        let old_poll = {
            let mut poll = crate::register::RESTART_POLL.lock().expect("restart poll");
            let old = *poll;
            *poll = Duration::from_millis(10);
            old
        };
        struct Restore(Duration);
        impl Drop for Restore {
            fn drop(&mut self) {
                *crate::register::RESTART_POLL.lock().expect("restart poll") = self.0;
            }
        }
        let _restore = Restore(old_poll);
        let ts = additions_store(false);
        let (h, _script) = fake_handler(
            &ts,
            json!([{"content": "First."}, {"command": "sleep 2"}, {"content": "Done."}]),
        );
        let ctx = pulse_bus::CancellationToken::new();
        // Nothing works: the drain returns at once with nothing to say.
        assert_eq!(
            crate::register::drain_conversations(&ctx, &h.chats, Duration::from_secs(5)),
            ""
        );
        let (code, body, _) = call(
            &h,
            "POST",
            "/api/v1/chat",
            json!({"text": "take your time"}),
        );
        assert_eq!(code, 201);
        let id = body["conversation"]["id"].as_str().unwrap().to_string();
        wait_status(&h, &id, &["idle"]);
        let (code, _, _) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/message"),
            json!({"text": "go on"}),
        );
        assert_eq!(code, 200);
        wait_status(&h, &id, &["running"]);
        // Capped: the hub goes anyway, and says so.
        let started = std::time::Instant::now();
        let line = crate::register::drain_conversations(&ctx, &h.chats, Duration::from_millis(50));
        assert!(
            line.ends_with("for 1 conversation(s) and stopped them to restart"),
            "{line}"
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        // Within the cap: the drain lasts until the turn is over.
        let line = crate::register::drain_conversations(&ctx, &h.chats, Duration::from_secs(20));
        assert!(line.ends_with("for 1 conversation(s) to finish"), "{line}");
        assert!(
            started.elapsed() >= Duration::from_millis(1500),
            "{:?}",
            started.elapsed()
        );
        let shown = wait_status(&h, &id, &["idle"]);
        assert_eq!(shown["messages"].as_array().unwrap().len(), 4);

        // The hub dies with a turn in flight: the row keeps saying running.
        // A handler built over the same store recovers it as it opens.
        ts.store.chat_set_status(&id, "running", "").unwrap();
        let fresh = handler(&ts);
        let (_, cut, carried) = fresh.open_chats().expect("manager");
        assert_eq!(cut, [id.as_str()]);
        assert!(carried.is_empty(), "a cut turn is left to the owner");
        let (code, shown, _) = call(&fresh, "GET", &format!("/api/v1/chat/{id}"), Value::Null);
        assert_eq!(code, 200);
        assert_eq!(shown["live"]["status"], "idle");
        let messages = shown["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 5);
        let note = &messages[4];
        assert_eq!(note["role"], "note");
        assert_eq!(note["extra"]["restart"], true);
        assert_eq!(note["extra"]["session"], shown["session_id"]);
        assert!(
            note["content"]
                .as_str()
                .unwrap()
                .starts_with("PULSE restarted while the agent was working"),
            "{note}"
        );
        let (_, again, _) = fresh.open_chats().expect("manager");
        assert!(again.is_empty(), "the manager is built once");
    }

    #[test]
    fn artifacts_are_listed_and_served_safely() {
        let _env = pulse_testutil::env_lock();
        let ts = additions_store(false);
        let (h, _script) = fake_handler(&ts, json!([{"content": "ok"}]));
        let (code, body, _) = call(&h, "POST", "/api/v1/chat", json!({"text": "make a report"}));
        assert_eq!(code, 201);
        let id = body["conversation"]["id"].as_str().unwrap().to_string();
        wait_status(&h, &id, &["idle"]);
        let chats = h.chats().unwrap();
        let dir = chats.artifacts_dir(&id);
        std::fs::write(dir.join("report.html"), "<h1>hi</h1>").unwrap();
        std::fs::write(dir.join("notes.md"), "# notes\n").unwrap();
        std::fs::write(dir.join("data.bin"), [0u8, 1, 2, 3]).unwrap();
        let (_, shown, _) = call(&h, "GET", &format!("/api/v1/chat/{id}"), Value::Null);
        let names: Vec<&str> = shown["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap())
            .collect();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"report.html"));
        let srv = crate::test_helpers_http::server(h.clone());
        let client = crate::test_helpers::client();
        let resp = client
            .get(format!(
                "{}/api/v1/chat/{id}/artifacts/report.html",
                srv.url
            ))
            .send()
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        assert!(
            resp.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
        assert_eq!(
            resp.headers()["content-security-policy"],
            "sandbox allow-scripts"
        );
        // The chat page frames artifacts; the hub's blanket DENY gives way.
        assert_eq!(resp.headers()["x-frame-options"], "SAMEORIGIN");
        assert_eq!(resp.text().unwrap(), "<h1>hi</h1>");
        let resp = client
            .get(format!("{}/api/v1/chat/{id}/artifacts/data.bin", srv.url))
            .send()
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(resp.headers()["content-type"], "application/octet-stream");
        assert_eq!(resp.headers()["x-frame-options"], "SAMEORIGIN");
        assert!(
            !resp.headers()["content-security-policy"]
                .to_str()
                .unwrap()
                .contains("frame-ancestors"),
            "a non-page artifact may be framed by the hub page"
        );
        let resp = client
            .get(format!(
                "{}/api/v1/chat/{id}/artifacts/missing.txt",
                srv.url
            ))
            .send()
            .unwrap();
        assert_eq!(resp.status().as_u16(), 404);
        let resp = client
            .get(format!(
                "{}/api/v1/chat/{id}/artifacts/..%2Fscript.json",
                srv.url
            ))
            .send()
            .unwrap();
        assert_ne!(resp.status().as_u16(), 200, "no path escapes");
        let (code, _, _) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/archive"),
            Value::Null,
        );
        assert_eq!(code, 200);
        std::thread::sleep(Duration::from_millis(100));
    }

    #[test]
    fn attachments_upload_ride_with_a_message_and_the_model_can_change() {
        let _env = pulse_testutil::env_lock();
        let ts = additions_store(false);
        let (h, _script) = fake_handler(&ts, json!([{"content": "ok"}, {"content": "read it"}]));
        let (code, _, raw) = call(
            &h,
            "POST",
            "/api/v1/chat",
            json!({"text": "hello", "model": "no spaces"}),
        );
        assert_eq!(code, 400, "{raw}");
        let (code, body, raw) = call(
            &h,
            "POST",
            "/api/v1/chat",
            json!({"text": "hello", "model": "fake-1"}),
        );
        assert_eq!(code, 201, "{raw}");
        let id = body["conversation"]["id"].as_str().unwrap().to_string();
        assert_eq!(body["conversation"]["model"], "fake-1");
        wait_status(&h, &id, &["idle"]);

        let srv = crate::test_helpers_http::server(h.clone());
        let client = crate::test_helpers::client();
        let upload = |name: &str, bytes: &[u8]| {
            client
                .post(format!("{}/api/v1/chat/{id}/upload?name={name}", srv.url))
                .body(bytes.to_vec())
                .send()
                .unwrap()
        };
        let resp = upload("", b"x");
        assert_eq!(resp.status().as_u16(), 400, "a name is required");
        let resp = upload("notes.txt", b"");
        assert_eq!(resp.status().as_u16(), 413, "an empty body is refused");
        let resp = upload("..%2Fscript.json", b"evil");
        assert_eq!(resp.status().as_u16(), 400, "no path escapes");
        let resp = upload("my%20notes.txt", b"remember the milk");
        assert_eq!(resp.status().as_u16(), 201);
        let saved: Value = resp.json().unwrap();
        assert_eq!(saved["attachment"]["name"], "my notes.txt");
        assert_eq!(saved["attachment"]["size"], 17);
        let resp = upload("my%20notes.txt", b"again");
        assert_eq!(resp.status().as_u16(), 201);
        let again: Value = resp.json().unwrap();
        assert_eq!(again["attachment"]["name"], "my notes-2.txt");
        // A cross-site page cannot push files in.
        let resp = client
            .post(format!("{}/api/v1/chat/{id}/upload?name=x.txt", srv.url))
            .header("Origin", "http://evil.example")
            .body(b"x".to_vec())
            .send()
            .unwrap();
        assert_eq!(resp.status().as_u16(), 403);

        // The upload is served back, under its own route only.
        let resp = client
            .get(format!(
                "{}/api/v1/chat/{id}/attachments/my%20notes.txt",
                srv.url
            ))
            .send()
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(resp.text().unwrap(), "remember the milk");
        let resp = client
            .get(format!(
                "{}/api/v1/chat/{id}/artifacts/my%20notes.txt",
                srv.url
            ))
            .send()
            .unwrap();
        assert_eq!(resp.status().as_u16(), 404);
        let resp = client
            .get(format!(
                "{}/api/v1/chat/{id}/attachments/missing.txt",
                srv.url
            ))
            .send()
            .unwrap();
        assert_eq!(resp.status().as_u16(), 404);

        // The message names the uploads; unknown names and bad shapes are refused.
        let (code, _, _) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/message"),
            json!({"text": "read", "attachments": "my notes.txt"}),
        );
        assert_eq!(code, 400);
        let (code, _, _) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/message"),
            json!({"text": "read", "attachments": ["nope.txt"]}),
        );
        assert_eq!(code, 400);
        let (code, _, raw) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/message"),
            json!({"attachments": ["my notes.txt", "my notes-2.txt"]}),
        );
        assert_eq!(code, 200, "{raw}");
        let shown = wait_status(&h, &id, &["idle"]);
        let msgs = shown["messages"].as_array().unwrap();
        let user = msgs.iter().rev().find(|m| m["role"] == "user").unwrap();
        assert_eq!(user["content"], "Look at the attached files.");
        assert_eq!(user["extra"]["attachments"][0]["name"], "my notes.txt");
        assert_eq!(user["extra"]["attachments"][1]["name"], "my notes-2.txt");

        // The model changes for the next turn; a bad id is refused.
        let (code, _, _) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/model"),
            json!({"model": "bad model"}),
        );
        assert_eq!(code, 400);
        let (code, body, raw) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/model"),
            json!({"model": "fake-2"}),
        );
        assert_eq!(code, 200, "{raw}");
        assert_eq!(body["conversation"]["model"], "fake-2");
        // So does the reasoning effort; a word outside the list is refused.
        let (code, _, _) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/reasoning"),
            json!({"reasoning": "extreme"}),
        );
        assert_eq!(code, 400);
        let (code, body, raw) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/reasoning"),
            json!({"reasoning": "high"}),
        );
        assert_eq!(code, 200, "{raw}");
        assert_eq!(body["conversation"]["reasoning"], "high");
        let (code, _, _) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/archive"),
            Value::Null,
        );
        assert_eq!(code, 200);
        std::thread::sleep(Duration::from_millis(100));
    }

    /// The owner finishes a conversation from the hub: it reads closed with
    /// the owner's line in the transcript, stays in the open list, and the
    /// next message reopens it. The first message may name the effort.
    #[test]
    fn close_finishes_a_conversation_and_a_message_reopens_it() {
        let _env = pulse_testutil::env_lock();
        let ts = additions_store(false);
        let (h, _script) = fake_handler(&ts, json!([{"content": "ok"}, {"content": "again"}]));
        let (code, body, raw) = call(
            &h,
            "POST",
            "/api/v1/chat",
            json!({"text": "hi", "reasoning": "low"}),
        );
        assert_eq!(code, 201, "{raw}");
        assert_eq!(body["conversation"]["reasoning"], "low");
        let id = body["conversation"]["id"].as_str().unwrap().to_string();
        wait_status(&h, &id, &["idle"]);
        let (code, body, raw) = call(&h, "POST", &format!("/api/v1/chat/{id}/close"), Value::Null);
        assert_eq!(code, 200, "{raw}");
        assert_eq!(body["conversation"]["status"], "closed");
        assert_eq!(body["conversation"]["live"]["status"], "closed");
        let shown = wait_status(&h, &id, &["closed"]);
        let msgs = shown["messages"].as_array().unwrap();
        let last = msgs.last().unwrap();
        assert_eq!(last["role"], "note");
        assert_eq!(last["extra"]["closed"], "owner");
        let (code, body, _) = call(&h, "GET", "/api/v1/chat", Value::Null);
        assert_eq!(code, 200);
        assert_eq!(body["conversations"].as_array().unwrap().len(), 1);
        // Closing again changes nothing; a message reopens.
        let (code, _, _) = call(&h, "POST", &format!("/api/v1/chat/{id}/close"), Value::Null);
        assert_eq!(code, 200);
        let (code, _, raw) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/message"),
            json!({"text": "one more"}),
        );
        assert_eq!(code, 200, "{raw}");
        let shown = wait_status(&h, &id, &["idle"]);
        assert_eq!(shown["conversation"]["status"], "idle");
        let msgs = shown["messages"].as_array().unwrap();
        assert_eq!(msgs.last().unwrap()["content"], "again");
        let (code, _, _) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/archive"),
            Value::Null,
        );
        assert_eq!(code, 200);
        std::thread::sleep(Duration::from_millis(100));
    }

    /// `POST /chat` with `parent` starts a sub-conversation in the parent's
    /// workspace; the parent's page lists it under `children` and the
    /// child's names its `parent_title`; the child's reply reaches the
    /// parent as a report note; the list carries `parent` on every row.
    #[test]
    fn a_sub_conversation_is_started_shown_and_reports_over_http() {
        let _env = pulse_testutil::env_lock();
        let ts = additions_store(false);
        let (h, _script) = fake_handler(
            &ts,
            json!([{"content": "First."}, {"content": "Second."}, {"content": "Third."}]),
        );
        let (code, body, raw) = call(
            &h,
            "POST",
            "/api/v1/chat",
            json!({"text": "plan the release", "title": "Release"}),
        );
        assert_eq!(code, 201, "{raw}");
        let pid = body["conversation"]["id"].as_str().unwrap().to_string();
        assert_eq!(body["conversation"]["parent"], "");
        let parent = wait_status(&h, &pid, &["idle"]);
        let cwd = parent["conversation"]["cwd"].as_str().unwrap().to_string();
        assert!(!cwd.is_empty());
        let (code, body, raw) = call(
            &h,
            "POST",
            "/api/v1/chat",
            json!({"text": "check the changelog", "title": "Changelog", "parent": pid}),
        );
        assert_eq!(code, 201, "{raw}");
        let cid = body["conversation"]["id"].as_str().unwrap().to_string();
        assert_eq!(body["conversation"]["parent"], pid);
        assert_eq!(body["conversation"]["cwd"], cwd);
        let (code, _, raw) = call(
            &h,
            "POST",
            "/api/v1/chat",
            json!({"text": "x", "parent": "chat-missing"}),
        );
        assert_eq!(code, 409, "{raw}");
        assert!(raw.contains("no such conversation"), "{raw}");
        let child = wait_status(&h, &cid, &["idle"]);
        assert_eq!(child["parent_title"], "Release");
        assert_eq!(child["children"].as_array().unwrap().len(), 0);
        // The child's reply is a report in the parent, which ran on it.
        let mut parent = JsonMap::new();
        for _ in 0..400 {
            parent = wait_status(&h, &pid, &["idle"]);
            let got = parent["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["extra"]["report"]["child"] == cid);
            if got {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let msgs = parent["messages"].as_array().unwrap();
        let report = msgs
            .iter()
            .find(|m| m["extra"]["report"]["child"] == cid)
            .unwrap_or_else(|| panic!("no report: {msgs:#?}"));
        assert_eq!(report["role"], "note");
        assert_eq!(report["content"], "First.");
        assert_eq!(report["extra"]["report"]["kind"], "replied");
        assert_eq!(report["extra"]["report"]["title"], "Changelog");
        assert!(
            msgs.iter()
                .any(|m| m["extra"]["child"] == cid && m["role"] == "note"),
            "the parent notes the start: {msgs:#?}"
        );
        let children = parent["children"].as_array().unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0]["id"], cid);
        assert_eq!(children[0]["title"], "Changelog");
        assert!(children[0]["live"]["status"].is_string());
        assert_eq!(parent["parent_title"], "");
        let (code, body, _) = call(&h, "GET", "/api/v1/chat", Value::Null);
        assert_eq!(code, 200);
        let rows = body["conversations"].as_array().unwrap();
        let row = rows.iter().find(|r| r["id"] == cid).unwrap();
        assert_eq!(row["parent"], pid);
        std::thread::sleep(Duration::from_millis(100));
    }

    #[test]
    fn images_post_stores_the_bytes_and_the_image_route_serves_them() {
        let _env = pulse_testutil::env_lock();
        let ts = additions_store(false);
        let (h, _script) = fake_handler(&ts, json!([{"content": "ok"}, {"content": "saw it"}]));
        let (code, body, raw) = call(&h, "POST", "/api/v1/chat", json!({"text": "hi"}));
        assert_eq!(code, 201, "{raw}");
        let id = body["conversation"]["id"].as_str().unwrap().to_string();
        wait_status(&h, &id, &["idle"]);
        let png = b"\x89PNG fake bytes";
        let b64 = STANDARD.encode(png);
        let (code, _, raw) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/message"),
            json!({"text": "see", "images": [{"mime": "image/png", "data": b64}]}),
        );
        assert_eq!(code, 200, "{raw}");
        let shown = wait_status(&h, &id, &["idle"]);
        let msgs = shown["messages"].as_array().unwrap();
        let user = msgs.iter().rev().find(|m| m["role"] == "user").unwrap();
        assert_eq!(user["content"], "see");
        assert_eq!(user["extra"]["images"][0]["name"], "image-1.png");
        assert_eq!(user["extra"]["images"][0]["mime"], "image/png");
        let n = user["n"].as_i64().unwrap();

        // The image route serves the stored bytes with the stored mime.
        let srv = crate::test_helpers_http::server(h.clone());
        let client = crate::test_helpers::client();
        let resp = client
            .get(format!("{}/api/v1/chat/{id}/image/{n}/0", srv.url))
            .send()
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(resp.headers()["content-type"], "image/png");
        let got = resp.bytes().unwrap();
        assert_eq!(got.as_ref(), png);
        for (path, want) in [
            (format!("/api/v1/chat/{id}/image/{n}/1"), 404),
            (format!("/api/v1/chat/{id}/image/9999/0"), 404),
            (format!("/api/v1/chat/{id}/image/x/0"), 404),
            // a missing conversation answers like the rest of the chat
            // surface: 409 with "no such conversation".
            ("/api/v1/chat/missing/image/1/0".to_string(), 409),
        ] {
            let resp = client.get(format!("{}{path}", srv.url)).send().unwrap();
            assert_eq!(resp.status().as_u16(), want, "{path}");
        }

        // Bad shapes are refused before anything is stored.
        for body in [
            json!({"text": "x", "images": "one.png"}),
            json!({"text": "x", "images": [{"mime": "image/gif", "data": b64}]}),
            json!({"text": "x", "images": [{"mime": "image/png", "data": "nope!!!"}]}),
            json!({"text": "x", "images": [{"mime": "image/png", "data": STANDARD.encode(vec![0u8; pulse_harness::conversation::IMAGE_LIMIT + 1])}]}),
        ] {
            let (code, _, _) = call(&h, "POST", &format!("/api/v1/chat/{id}/message"), body);
            assert_eq!(code, 400);
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    #[test]
    fn a_project_conversation_reports_its_branch_and_attach_validates() {
        let _env = pulse_testutil::env_lock();
        let _nosys = pulse_testutil::setenv("GIT_CONFIG_NOSYSTEM", "1");
        let _noglobal = pulse_testutil::setenv("GIT_CONFIG_GLOBAL", "/dev/null");
        let ts = additions_store(false);
        additions_project(&ts, "demo");
        let path = std::path::Path::new(&ts.home).join("demo");
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
        ] {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(&args)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        }
        std::fs::write(path.join("README.md"), "hello\n").unwrap();
        for args in [vec!["add", "."], vec!["commit", "-q", "-m", "first"]] {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(&args)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        }
        ts.conn()
            .unwrap()
            .execute(
                "UPDATE projects SET default_branch = 'main' WHERE slug = 'demo'",
                [],
            )
            .unwrap();
        let (h, _script) = fake_handler(
            &ts,
            json!([
                {"command": "printf 'more\\n' >> README.md && git add README.md && git -c user.email=a@b.c -c user.name=a commit -q -m 'chat change' && echo done"},
                {"content": "Committed."}
            ]),
        );
        let (code, _, raw) = call(
            &h,
            "POST",
            "/api/v1/chat",
            json!({"text": "x", "project": "nope"}),
        );
        assert_eq!(code, 409, "unknown project: {raw}");
        let (code, body, raw) = call(
            &h,
            "POST",
            "/api/v1/chat",
            json!({"text": "add a line", "project": "demo"}),
        );
        assert_eq!(code, 201, "{raw}");
        let id = body["conversation"]["id"].as_str().unwrap().to_string();
        let shown = wait_status(&h, &id, &["idle"]);
        assert_eq!(shown["conversation"]["project"], "demo");
        assert_eq!(shown["conversation"]["branch"], format!("pulse/chat-{id}"));
        assert_eq!(shown["changes"]["commits"], 1, "{}", shown["changes"]);
        assert!(
            shown["changes"]["stat"]
                .as_str()
                .unwrap()
                .contains("README.md")
        );
        let (code, diff, _) = call(&h, "GET", &format!("/api/v1/chat/{id}/diff"), Value::Null);
        assert_eq!(code, 200);
        assert!(diff["diff"].as_str().unwrap().contains("+more"));
        let (code, _, raw) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/attach"),
            json!({"project": "nope"}),
        );
        assert_eq!(code, 409, "{raw}");
        let (code, body, raw) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/attach"),
            json!({"project": "demo"}),
        );
        assert_eq!(code, 200, "{raw}");
        assert_eq!(
            body["attached"]["created"], false,
            "the worktree already exists"
        );
        let (code, _, _) = call(
            &h,
            "POST",
            &format!("/api/v1/chat/{id}/archive"),
            Value::Null,
        );
        assert_eq!(code, 200);
        let (code, _, _) = call(&h, "GET", "/api/v1/chat/nope", Value::Null);
        assert_eq!(code, 404);
        let (code, _, _) = call(&h, "POST", "/api/v1/chat", json!({"text": ""}));
        assert_eq!(code, 400);
        std::thread::sleep(Duration::from_millis(100));
    }

    // The list carries the tasks the page groups conversations under: each
    // with its thread with the lead summed up and one session per step the
    // lead planned - a queued step with no agent yet, a running one with
    // its harness session. A task finished long ago is listed only with
    // `all=1`; the newest change sorts first.
    #[test]
    fn the_list_groups_a_tasks_sessions_and_keeps_old_finished_tasks_for_all() {
        let _env = pulse_testutil::env_lock();
        let ts = additions_store(false);
        additions_project(&ts, "one");
        let h = handler(&ts);
        let task = pulse_tasks::new(&ts, "one", "Ship the spec", "the brief", 10, "human").unwrap();
        let old = pulse_tasks::new(&ts, "one", "Long done", "the brief", 10, "human").unwrap();
        let conn = ts.conn().unwrap();
        conn.execute(
            "UPDATE tasks SET status='in_progress', lead='one-lead' WHERE id=?",
            [&task.id],
        )
        .unwrap();
        conn.execute(
            "UPDATE tasks SET status='done', updated_at='2026-01-01T00:00:00Z' WHERE id=?",
            [&old.id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (id, project, task_id, author, body, at) VALUES ('MSG-1', 'one', ?, 'one-lead', 'Planned 2 steps.', '2026-10-31T10:00:00Z')",
            [&task.id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO jobs (id, project, kind, title, source, status, lease_agent, created_at) VALUES \
             ('job-a', 'one', 'task', 'Write the page', ?1, 'leased', 'iris', '2026-10-31T10:01:00Z'), \
             ('job-b', 'one', 'task', 'Add the test', ?1 || ':ui', 'queued', '', '2026-10-31T10:02:00Z')",
            [format!("task:{}", task.id)],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO harness_sessions (id, job_id, agent, project, status, steps, cost, started_at, updated_at) VALUES \
             ('run-a', 'job-a', 'iris', 'one', 'running', 12, 0.25, '2026-10-31T10:05:00Z', '2026-10-31T10:09:00Z')",
            [],
        )
        .unwrap();
        drop(conn);

        let (code, body, raw) = call(&h, "GET", "/api/v1/chat", Value::Null);
        assert_eq!(code, 200, "{raw}");
        let tasks = body["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1, "the old finished task stays out: {raw}");
        let g = &tasks[0];
        assert_eq!(g["id"], task.id);
        assert_eq!(g["lead"], "one-lead");
        assert_eq!(g["messages"], 1);
        assert_eq!(g["last_author"], "one-lead");
        assert_eq!(g["needs_you"], false);
        assert_eq!(
            g["last_at"], "2026-10-31T10:09:00Z",
            "the newest change of any session"
        );
        let sessions = g["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0]["job"], "job-a");
        assert_eq!(sessions[0]["session"], "run-a");
        assert_eq!(sessions[0]["agent"], "iris");
        assert_eq!(sessions[0]["status"], "running");
        assert_eq!(sessions[0]["job_status"], "leased");
        assert_eq!(sessions[0]["steps"], 12);
        assert_eq!(sessions[1]["job"], "job-b");
        assert_eq!(sessions[1]["title"], "Add the test");
        assert_eq!(sessions[1]["session"], "");
        assert_eq!(sessions[1]["job_status"], "queued");

        let (code, body, raw) = call(&h, "GET", "/api/v1/chat?all=1", Value::Null);
        assert_eq!(code, 200, "{raw}");
        let ids: Vec<&str> = body["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec![task.id.as_str(), old.id.as_str()]);
    }
}
