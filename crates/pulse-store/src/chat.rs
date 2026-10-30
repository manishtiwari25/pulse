//! Conversations: the owner's chats with an agent from the hub, and their
//! messages. See `chat_ddl.rs` for the tables.
use crate::store::{Store, input_err};
use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CHAT_STATUSES: &[&str] = &["idle", "running", "waiting", "ended", "closed", "archived"];
pub const CHAT_ROLES: &[&str] = &[
    "user",
    "assistant",
    "question",
    "answer",
    "approval",
    "note",
];

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub project: String,
    pub cwd: String,
    pub branch: String,
    pub status: String,
    pub session_id: String,
    pub provider: String,
    pub model: String,
    /// The reasoning effort the agent runs with; empty for the provider's
    /// default.
    pub reasoning: String,
    pub auto_approve: bool,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_at: String,
    pub turns: i64,
    pub cost: f64,
    pub note: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct ConversationMessage {
    pub conversation_id: String,
    pub n: i64,
    pub at: String,
    pub role: String,
    pub content: String,
    pub extra: Value,
}

const CHAT_COLS: &str = "id, title, project, cwd, branch, status, session_id, provider, model, auto_approve, created_by, created_at, updated_at, last_at, turns, cost, note, reasoning";
const MESSAGE_COLS: &str = "conversation_id, n, at, role, content, extra";

fn scan_chat(r: &rusqlite::Row<'_>) -> rusqlite::Result<Conversation> {
    let auto: i64 = r.get(9)?;
    Ok(Conversation {
        id: r.get(0)?,
        title: r.get(1)?,
        project: r.get(2)?,
        cwd: r.get(3)?,
        branch: r.get(4)?,
        status: r.get(5)?,
        session_id: r.get(6)?,
        provider: r.get(7)?,
        model: r.get(8)?,
        auto_approve: auto != 0,
        created_by: r.get(10)?,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
        last_at: r.get(13)?,
        turns: r.get(14)?,
        cost: r.get(15)?,
        note: r.get(16)?,
        reasoning: r.get(17)?,
    })
}

fn scan_message(r: &rusqlite::Row<'_>) -> rusqlite::Result<ConversationMessage> {
    let extra: String = r.get(5)?;
    Ok(ConversationMessage {
        conversation_id: r.get(0)?,
        n: r.get(1)?,
        at: r.get(2)?,
        role: r.get(3)?,
        content: r.get(4)?,
        extra: serde_json::from_str(&extra).unwrap_or(Value::Object(Default::default())),
    })
}

impl Store {
    /// Rebuilds a `conversations` table from before the `closed` status,
    /// whose CHECK would refuse it. `conversation_messages` references the
    /// table, so it cannot be moved aside by name like `WIDENED_CHECKS`
    /// does (the rename would follow into the children's constraint): the
    /// current shape is built beside it, the rows copied over by column
    /// name, the old table dropped and the new one renamed into place, with
    /// foreign keys off for the span so the messages stay where they are.
    pub(crate) fn widen_chat_statuses(&self) -> Result<()> {
        use crate::store::{table_columns, table_present, table_sql_has};
        let conn = self.conn()?;
        if !table_present(&conn, "conversations")?
            || table_sql_has(&conn, "conversations", "'closed'")?
        {
            return Ok(());
        }
        let create = crate::chat_ddl::CHAT_DDL[0].replacen(
            "CREATE TABLE IF NOT EXISTS conversations (",
            "CREATE TABLE conversations_widened (",
            1,
        );
        conn.pragma_update(None, "foreign_keys", 0)?;
        let rebuilt = (|| -> Result<()> {
            conn.execute_batch("BEGIN IMMEDIATE; DROP TABLE IF EXISTS conversations_widened;")?;
            let work = (|| -> Result<i64> {
                conn.execute_batch(&create)?;
                let old = table_columns(&conn, "conversations")?;
                let shared = table_columns(&conn, "conversations_widened")?
                    .into_iter()
                    .filter(|c| old.contains(c))
                    .collect::<Vec<_>>()
                    .join(", ");
                let rows = conn.execute(
                    &format!(
                        "INSERT INTO conversations_widened ({shared}) SELECT {shared} FROM conversations"
                    ),
                    [],
                )?;
                conn.execute_batch(
                    "DROP TABLE conversations; ALTER TABLE conversations_widened RENAME TO conversations;",
                )?;
                for stmt in crate::chat_ddl::CHAT_DDL {
                    conn.execute(stmt, [])?;
                }
                crate::store::event_insert(
                    &conn,
                    &self.now(),
                    "pulse",
                    "schema.widened",
                    "conversations",
                    &format!("rows={rows}"),
                )?;
                Ok(rows as i64)
            })();
            match work {
                Ok(_) => Ok(conn.execute_batch("COMMIT")?),
                Err(e) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    Err(e)
                }
            }
        })();
        let restored = conn.pragma_update(None, "foreign_keys", 1);
        rebuilt?;
        Ok(restored?)
    }

    /// Records a new conversation; `c.id` is the caller's, the clock stamps
    /// fill in.
    pub fn chat_create(&self, c: &Conversation) -> Result<Conversation> {
        if c.id.trim().is_empty() {
            return Err(input_err("conversation id is empty"));
        }
        if !CHAT_STATUSES.contains(&c.status.as_str()) {
            return Err(input_err(format!(
                "conversation status {:?} is not one of {}",
                c.status,
                CHAT_STATUSES.join(", ")
            )));
        }
        let now = self.now();
        let tx = self.begin()?;
        tx.execute(
            &format!("INSERT INTO conversations ({CHAT_COLS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"),
            params![
                c.id,
                c.title,
                c.project,
                c.cwd,
                c.branch,
                c.status,
                c.session_id,
                c.provider,
                c.model,
                c.auto_approve as i64,
                c.created_by,
                now,
                now,
                c.last_at,
                c.turns,
                c.cost,
                c.note,
                c.reasoning
            ],
        )?;
        tx.commit()?;
        self.chat_get(&c.id)?
            .ok_or_else(|| input_err(format!("conversation {} was not written", c.id)))
    }

    pub fn chat_get(&self, id: &str) -> Result<Option<Conversation>> {
        let conn = self.conn()?;
        Ok(conn
            .query_row(
                &format!("SELECT {CHAT_COLS} FROM conversations WHERE id = ?"),
                params![id],
                scan_chat,
            )
            .optional()?)
    }

    /// The conversations, newest activity first; archived ones only when
    /// asked for.
    pub fn chat_list(&self, archived: bool, limit: i64) -> Result<Vec<Conversation>> {
        let conn = self.conn()?;
        let where_ = if archived {
            "WHERE status = 'archived'"
        } else {
            "WHERE status != 'archived'"
        };
        let mut stmt = conn.prepare(&format!(
            "SELECT {CHAT_COLS} FROM conversations {where_} ORDER BY COALESCE(NULLIF(last_at, ''), updated_at) DESC, id DESC LIMIT ?"
        ))?;
        Ok(stmt
            .query_map(params![limit.max(1)], scan_chat)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Sets the status and the one-line note that explains it.
    pub fn chat_set_status(&self, id: &str, status: &str, note: &str) -> Result<()> {
        if !CHAT_STATUSES.contains(&status) {
            return Err(input_err(format!(
                "conversation status {status:?} is not one of {}",
                CHAT_STATUSES.join(", ")
            )));
        }
        self.chat_update(id, "status = ?1, note = ?2", params![status, note])
    }

    /// Records the harness session the conversation now runs in.
    pub fn chat_set_session(
        &self,
        id: &str,
        session_id: &str,
        provider: &str,
        model: &str,
    ) -> Result<()> {
        self.chat_update(
            id,
            "session_id = ?1, provider = ?2, model = ?3",
            params![session_id, provider, model],
        )
    }

    /// Moves the conversation into a project's worktree (or out of one).
    pub fn chat_set_workspace(
        &self,
        id: &str,
        project: &str,
        cwd: &str,
        branch: &str,
    ) -> Result<()> {
        self.chat_update(
            id,
            "project = ?1, cwd = ?2, branch = ?3",
            params![project, cwd, branch],
        )
    }

    pub fn chat_set_auto_approve(&self, id: &str, on: bool) -> Result<()> {
        self.chat_update(id, "auto_approve = ?1", params![on as i64])
    }

    pub fn chat_set_title(&self, id: &str, title: &str) -> Result<()> {
        self.chat_update(id, "title = ?1", params![title.trim()])
    }

    /// The model the next turn runs on (the runtime restarts its session
    /// onto it with the conversation carried over).
    pub fn chat_set_model(&self, id: &str, model: &str) -> Result<()> {
        self.chat_update(id, "model = ?1", params![model.trim()])
    }

    /// The reasoning effort the next turn runs with (empty: the provider's
    /// default); the runtime restarts its session like for a model change.
    pub fn chat_set_reasoning(&self, id: &str, reasoning: &str) -> Result<()> {
        self.chat_update(id, "reasoning = ?1", params![reasoning.trim()])
    }

    /// Adds a finished turn's cost and count.
    pub fn chat_add_turn(&self, id: &str, cost: f64) -> Result<()> {
        self.chat_update(id, "turns = turns + 1, cost = cost + ?1", params![cost])
    }

    fn chat_update(&self, id: &str, set: &str, values: &[&dyn rusqlite::ToSql]) -> Result<()> {
        let now = self.now();
        let conn = self.conn()?;
        let n = values.len() + 1;
        let mut all: Vec<&dyn rusqlite::ToSql> = values.to_vec();
        all.push(&now);
        let id_index = n + 1;
        all.push(&id);
        let changed = conn.execute(
            &format!("UPDATE conversations SET {set}, updated_at = ?{n} WHERE id = ?{id_index}"),
            all.as_slice(),
        )?;
        if changed == 0 {
            return Err(input_err(format!("conversation {id:?} does not exist")));
        }
        Ok(())
    }

    /// Appends a message and returns it with its number.
    pub fn chat_add_message(
        &self,
        id: &str,
        role: &str,
        content: &str,
        extra: &Value,
    ) -> Result<ConversationMessage> {
        if !CHAT_ROLES.contains(&role) {
            return Err(input_err(format!(
                "message role {role:?} is not one of {}",
                CHAT_ROLES.join(", ")
            )));
        }
        let now = self.now();
        let tx = self.begin()?;
        let n: i64 = tx.query_row(
            "SELECT COALESCE(MAX(n), 0) + 1 FROM conversation_messages WHERE conversation_id = ?",
            params![id],
            |r| r.get(0),
        )?;
        let extra_text = if extra.is_null() {
            "{}".to_string()
        } else {
            extra.to_string()
        };
        tx.execute(
            &format!(
                "INSERT INTO conversation_messages ({MESSAGE_COLS}) VALUES (?, ?, ?, ?, ?, ?)"
            ),
            params![id, n, now, role, content, extra_text],
        )?;
        let changed = tx.execute(
            "UPDATE conversations SET last_at = ?, updated_at = ? WHERE id = ?",
            params![now, now, id],
        )?;
        if changed == 0 {
            return Err(input_err(format!("conversation {id:?} does not exist")));
        }
        tx.commit()?;
        Ok(ConversationMessage {
            conversation_id: id.to_string(),
            n,
            at: now,
            role: role.to_string(),
            content: content.to_string(),
            extra: extra.clone(),
        })
    }

    /// One message by its sequence number within the conversation.
    pub fn chat_message(&self, id: &str, n: i64) -> Result<Option<ConversationMessage>> {
        let conn = self.conn()?;
        Ok(conn
            .query_row(
                &format!(
                    "SELECT {MESSAGE_COLS} FROM conversation_messages WHERE conversation_id = ? AND n = ?"
                ),
                params![id, n],
                scan_message,
            )
            .optional()?)
    }

    /// The messages after number `after`, oldest first.
    pub fn chat_messages(&self, id: &str, after: i64) -> Result<Vec<ConversationMessage>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {MESSAGE_COLS} FROM conversation_messages WHERE conversation_id = ? AND n > ? ORDER BY n"
        ))?;
        Ok(stmt
            .query_map(params![id, after], scan_message)?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Removes a conversation and its messages.
    pub fn chat_delete(&self, id: &str) -> Result<()> {
        let tx = self.begin()?;
        tx.execute(
            "DELETE FROM conversation_messages WHERE conversation_id = ?",
            params![id],
        )?;
        tx.execute("DELETE FROM conversations WHERE id = ?", params![id])?;
        tx.commit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Options;

    fn new_store() -> (Store, tempfile::TempDir) {
        let _guard = crate::test_helpers::serial();
        // Clear PULSE_JOB so the worker guard does not refuse a store opened
        // inside this job's checkout-adjacent tempdir (every other store
        // module's new_store does this; without it these tests fail under a
        // PULSE job whenever they run before a crate that clears it).
        unsafe { std::env::set_var("PULSE_JOB", "") };
        let home = tempfile::tempdir().unwrap();
        let s = Store::open(Options {
            home: home.path().display().to_string(),
            create: true,
            now: "2026-10-19T10:00:00Z".into(),
            ..Options::default()
        })
        .unwrap();
        (s, home)
    }

    fn chat(id: &str) -> Conversation {
        Conversation {
            id: id.into(),
            title: "fix the login".into(),
            status: "idle".into(),
            auto_approve: true,
            ..Conversation::default()
        }
    }

    #[test]
    fn conversations_round_trip_with_messages() {
        let (s, _home) = new_store();
        let c = s.chat_create(&chat("chat-1")).expect("create");
        assert_eq!(c.status, "idle");
        assert!(c.auto_approve);
        assert!(!c.created_at.is_empty());
        let m = s
            .chat_add_message("chat-1", "user", "hello", &Value::Null)
            .expect("message");
        assert_eq!(m.n, 1);
        let m2 = s
            .chat_add_message(
                "chat-1",
                "assistant",
                "hi",
                &serde_json::json!({"session": "hs-1"}),
            )
            .expect("message");
        assert_eq!(m2.n, 2);
        let all = s.chat_messages("chat-1", 0).expect("messages");
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].extra["session"], "hs-1");
        assert_eq!(s.chat_messages("chat-1", 1).expect("after").len(), 1);
        let got = s.chat_get("chat-1").expect("get").expect("row");
        assert_eq!(got.last_at, m2.at);
        s.chat_set_status("chat-1", "waiting", "a question waits")
            .expect("status");
        s.chat_set_workspace("chat-1", "proj", "/tmp/w", "pulse/chat-1")
            .expect("workspace");
        s.chat_set_session("chat-1", "hs-2", "fake", "m1")
            .expect("session");
        s.chat_add_turn("chat-1", 0.5).expect("turn");
        s.chat_set_auto_approve("chat-1", false).expect("auto");
        s.chat_set_title("chat-1", " a title ").expect("title");
        let got = s.chat_get("chat-1").expect("get").expect("row");
        assert_eq!(got.status, "waiting");
        assert_eq!(got.note, "a question waits");
        assert_eq!(got.project, "proj");
        assert_eq!(got.branch, "pulse/chat-1");
        assert_eq!(got.session_id, "hs-2");
        assert_eq!(got.turns, 1);
        assert_eq!(got.cost, 0.5);
        assert!(!got.auto_approve);
        assert_eq!(got.title, "a title");
    }

    #[test]
    fn list_hides_archived_and_rejects_bad_values() {
        let (s, _home) = new_store();
        s.chat_create(&chat("chat-a")).expect("a");
        s.chat_create(&chat("chat-b")).expect("b");
        s.chat_set_status("chat-a", "archived", "")
            .expect("archive");
        let open = s.chat_list(false, 10).expect("list");
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, "chat-b");
        let archived = s.chat_list(true, 10).expect("archived");
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].id, "chat-a");
        assert!(s.chat_set_status("chat-a", "gone", "").is_err());
        assert!(
            s.chat_add_message("chat-a", "robot", "x", &Value::Null)
                .is_err()
        );
        assert!(
            s.chat_add_message("chat-zzz", "user", "x", &Value::Null)
                .is_err()
        );
        assert!(s.chat_set_title("chat-zzz", "t").is_err());
        s.chat_delete("chat-a").expect("delete");
        assert!(s.chat_get("chat-a").expect("get").is_none());
    }

    #[test]
    fn closed_and_reasoning_round_trip() {
        let (s, _home) = new_store();
        s.chat_create(&chat("chat-c")).expect("c");
        s.chat_set_status("chat-c", "closed", "done: the login works")
            .expect("closed");
        s.chat_set_reasoning("chat-c", " high ").expect("reasoning");
        let got = s.chat_get("chat-c").expect("get").expect("row");
        assert_eq!(got.status, "closed");
        assert_eq!(got.reasoning, "high");
        // Closed conversations stay in the open list; archived leave it.
        assert_eq!(s.chat_list(false, 10).expect("list").len(), 1);
        let made = s
            .chat_create(&Conversation {
                reasoning: "low".into(),
                ..chat("chat-d")
            })
            .expect("d");
        assert_eq!(made.reasoning, "low");
    }

    /// A store from before the `closed` status (and the `reasoning`
    /// column) is rebuilt on open: the conversations and their messages
    /// survive, `closed` is accepted, and it is not rebuilt again.
    #[test]
    fn an_older_conversations_table_is_widened_for_closed() {
        let _serial = crate::test_helpers::serial();
        unsafe { std::env::set_var("PULSE_JOB", "") };
        let dir = tempfile::tempdir().unwrap();
        let open = || {
            Store::open(Options {
                home: dir.path().display().to_string(),
                create: true,
                now: "2026-10-30T10:00:00Z".into(),
                ..Options::default()
            })
            .unwrap()
        };
        let s = open();
        s.chat_create(&chat("chat-old")).expect("create");
        s.chat_add_message("chat-old", "user", "hello", &Value::Null)
            .expect("message");
        s.chat_add_message("chat-old", "assistant", "hi", &Value::Null)
            .expect("message");
        // Give the table the shape a store made before `closed` has: no
        // `closed` in the CHECK, no `reasoning` column.
        let narrow = crate::chat_ddl::CHAT_DDL[0]
            .replace("'closed',", "")
            .replace(
                "  reasoning TEXT NOT NULL DEFAULT '',  -- effort the agent runs with; '' = the provider's default\n",
                "",
            )
            .replacen(
                "CREATE TABLE IF NOT EXISTS conversations (",
                "CREATE TABLE conversations_old (",
                1,
            );
        assert!(!narrow.contains("reasoning"), "{narrow}");
        {
            let conn = s.conn().unwrap();
            conn.pragma_update(None, "foreign_keys", 0).unwrap();
            let cols = "id, title, project, cwd, branch, status, session_id, provider, model, auto_approve, created_by, created_at, updated_at, last_at, turns, cost, note";
            conn.execute_batch(&format!(
                "BEGIN; {narrow}; INSERT INTO conversations_old ({cols}) SELECT {cols} FROM conversations; \
                 DROP TABLE conversations; ALTER TABLE conversations_old RENAME TO conversations; \
                 UPDATE meta SET value = '6' WHERE key = 'records_version'; COMMIT;"
            ))
            .unwrap();
            conn.pragma_update(None, "foreign_keys", 1).unwrap();
            assert!(!crate::store::table_sql_has(&conn, "conversations", "'closed'").unwrap());
            assert!(!crate::store::has_column(&conn, "conversations", "reasoning").unwrap());
        }
        s.close().unwrap();
        let s = open();
        {
            let conn = s.conn().unwrap();
            assert!(crate::store::table_sql_has(&conn, "conversations", "'closed'").unwrap());
            assert!(crate::store::has_column(&conn, "conversations", "reasoning").unwrap());
            assert!(!crate::store::table_present(&conn, "conversations_widened").unwrap());
            let fk: i64 = conn
                .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(fk, 0, "the messages still point at their conversation");
            let widened: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM events WHERE action = 'schema.widened' AND subject = 'conversations' AND detail = 'rows=1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(widened, 1);
        }
        let got = s.chat_get("chat-old").expect("get").expect("row");
        assert_eq!(got.title, "fix the login");
        assert_eq!(got.reasoning, "");
        assert_eq!(s.chat_messages("chat-old", 0).expect("messages").len(), 2);
        s.chat_set_status("chat-old", "closed", "done")
            .expect("closed");
        assert!(s.additive_complete());
        s.close().unwrap();
        let s = open();
        let again: i64 = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM events WHERE action = 'schema.widened' AND subject = 'conversations'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(again, 1, "a widened table is not rebuilt again");
    }
}
