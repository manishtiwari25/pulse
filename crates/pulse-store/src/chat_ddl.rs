//! The conversations the owner holds with an agent from the hub (`/chat`):
//! one row per conversation, one per message either way. The steps a turn
//! took are the harness session's (`harness_steps`), reached through
//! `session_id`; a conversation outlives its harness sessions (a hub
//! restart starts a new one with the messages carried over).
use crate::Column;

pub const CHAT_DDL: &[&str] = &[
    r####"CREATE TABLE IF NOT EXISTS conversations (
  id TEXT PRIMARY KEY,                 -- chat-YYYYMMDDTHHMMSS-xxxxxx
  title TEXT NOT NULL DEFAULT '',
  project TEXT NOT NULL DEFAULT '',    -- '' = not attached yet; the agent decides
  cwd TEXT NOT NULL DEFAULT '',        -- where the agent works
  branch TEXT NOT NULL DEFAULT '',     -- pulse/chat-<id> once a worktree exists
  status TEXT NOT NULL DEFAULT 'idle' CHECK (status IN ('idle','running','waiting','ended','closed','archived')),
  session_id TEXT NOT NULL DEFAULT '', -- the current harness session
  provider TEXT NOT NULL DEFAULT '',
  model TEXT NOT NULL DEFAULT '',
  reasoning TEXT NOT NULL DEFAULT '',  -- effort the agent runs with; '' = the provider's default
  auto_approve INTEGER NOT NULL DEFAULT 1,
  created_by TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  last_at TEXT NOT NULL DEFAULT '',    -- the newest message either way
  turns INTEGER NOT NULL DEFAULT 0,
  cost REAL NOT NULL DEFAULT 0,
  note TEXT NOT NULL DEFAULT ''        -- why it ended or waits, one line
);"####,
    r####"CREATE TABLE IF NOT EXISTS conversation_messages (
  conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  n INTEGER NOT NULL,
  at TEXT NOT NULL,
  role TEXT NOT NULL CHECK (role IN ('user','assistant','question','answer','approval','note')),
  content TEXT NOT NULL,
  extra TEXT NOT NULL DEFAULT '{}',    -- JSON: session, steps range, tool, decision
  PRIMARY KEY (conversation_id, n)
);"####,
    "CREATE INDEX IF NOT EXISTS idx_conversations_updated ON conversations(status, updated_at);",
];

/// Columns added after the first cut; `ensure_records` adds them to an
/// older table. `reasoning`: the effort the owner picks per conversation.
pub const CHAT_COLUMNS: &[Column] = &[Column {
    table: r####"conversations"####,
    name: r####"reasoning"####,
    ddl: r####"TEXT NOT NULL DEFAULT ''"####,
}];
