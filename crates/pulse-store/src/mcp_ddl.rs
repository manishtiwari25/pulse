pub const MCP_DDL: &[&str] = &[
    r####"CREATE TABLE IF NOT EXISTS mcp_servers (
  id TEXT PRIMARY KEY,                -- MCP-001
  name TEXT NOT NULL,                 -- slug [a-z0-9][a-z0-9-]*
  project TEXT NOT NULL DEFAULT '',   -- '' = every project
  transport TEXT NOT NULL CHECK (transport IN ('stdio','http','sse')),
  command TEXT NOT NULL DEFAULT '',   -- stdio: the program to spawn
  args TEXT NOT NULL DEFAULT '[]',    -- stdio: JSON array of strings
  url TEXT NOT NULL DEFAULT '',       -- http/sse: the endpoint
  auth TEXT NOT NULL CHECK (auth IN ('none','bearer','basic','header','query','env','oauth')),
  auth_header TEXT NOT NULL DEFAULT 'Authorization', -- header, query parameter or env var name
  secret_ref TEXT NOT NULL DEFAULT '', -- pulse_cred reference, never the secret
  env TEXT NOT NULL DEFAULT '{}',     -- stdio: JSON object of strings; values may be pulse_cred references
  note TEXT NOT NULL DEFAULT '',      -- what the server is for, shown to agents
  status TEXT NOT NULL DEFAULT 'enabled' CHECK (status IN ('enabled','disabled')),
  last_check_at TEXT NOT NULL DEFAULT '',
  last_check_ok INTEGER,
  last_check_note TEXT NOT NULL DEFAULT '',
  created_by TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(project, name)
);"####,
    r####"CREATE TABLE IF NOT EXISTS skills (
  id TEXT PRIMARY KEY,                -- SKILL-001
  name TEXT NOT NULL UNIQUE,          -- slug [a-z0-9][a-z0-9-]*
  project TEXT NOT NULL DEFAULT '',   -- '' = every project
  description TEXT NOT NULL,          -- one line
  body TEXT NOT NULL,                 -- markdown
  status TEXT NOT NULL DEFAULT 'enabled' CHECK (status IN ('enabled','disabled')),
  created_by TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  uses INTEGER NOT NULL DEFAULT 0
);"####,
];
