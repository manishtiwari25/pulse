// The Connections page's pure parts: which auth fits which transport, what
// the form must say before it is sent, how a row's last check reads. No
// React and no DOM, so mcp.test.mjs can drive it. The rules mirror
// pulse_store::mcp_servers so the console refuses what the hub would refuse,
// with the same words.

import type { McpAuth, McpAuthOption, McpOptions, McpServer, McpServerInput, McpServerPatch, McpTransport, Skill, SkillInput, SkillPatch } from "./api";

export const SLUG = /^[a-z0-9][a-z0-9-]*$/;
export const MAX_NAME = 64;
export const MAX_DESCRIPTION = 200;

export interface EnvRow {
  key: string;
  value: string;
}

export interface McpForm {
  name: string;
  project: string; // "" = every project
  transport: McpTransport;
  command: string;
  args: string; // one argument per line
  url: string;
  auth: McpAuth;
  auth_header: string;
  secret_ref: string;
  env: EnvRow[];
  note: string;
}

export const EMPTY_MCP_FORM: McpForm = { name: "", project: "", transport: "stdio", command: "", args: "", url: "", auth: "none", auth_header: "", secret_ref: "", env: [], note: "" };

// Fallbacks for a hub without /mcp/options (an older build): the same
// catalogue pulse_store::MCP_AUTHS carries.
export const DEFAULT_OPTIONS: McpOptions = {
  transports: ["stdio", "http", "sse"],
  auths: [
    { name: "none", explain: "No credential; the server is open or trusts the connection.", transports: ["stdio", "http", "sse"] },
    { name: "bearer", explain: "Sends `Authorization: Bearer <secret>`; the secret is the token.", transports: ["http", "sse"] },
    { name: "basic", explain: "Sends HTTP basic auth; the secret is `user:password`.", transports: ["http", "sse"] },
    { name: "header", explain: "Sends the secret in a header of your choice (auth header name, e.g. X-Api-Key).", transports: ["http", "sse"] },
    { name: "query", explain: "Appends the secret as a query parameter of your choice (auth header name is the parameter).", transports: ["http", "sse"] },
    { name: "env", explain: "Passes the secret to the stdio command as an environment variable (auth header name is the variable).", transports: ["stdio"] },
  ],
  cred_shapes: "op://VAULT/ITEM/FIELD[?ssh-format=openssh], keychain://SERVICE/ACCOUNT, or file:///ABSOLUTE/PATH",
};

export const TRANSPORT_LABEL: Record<McpTransport, string> = {
  stdio: "Command (stdio)",
  http: "HTTP (streamable)",
  sse: "HTTP + SSE (legacy)",
};

export const TRANSPORT_BLURB: Record<McpTransport, string> = {
  stdio: "PULSE starts the command on this machine and talks to it over stdin/stdout.",
  http: "One URL that answers JSON-RPC over POST, the current MCP transport.",
  sse: "The older pairing: an event stream for answers and a POST endpoint it announces.",
};

// The auths a transport can use, in catalogue order.
export function authsFor(options: McpOptions | undefined | null, transport: McpTransport): McpAuthOption[] {
  return (options ?? DEFAULT_OPTIONS).auths.filter((a) => a.transports.includes(transport));
}

export function authExplain(options: McpOptions | undefined | null, auth: McpAuth): string {
  return (options ?? DEFAULT_OPTIONS).auths.find((a) => a.name === auth)?.explain ?? "";
}

export const needsSecret = (auth: McpAuth): boolean => auth !== "none";

// What the auth_header field is called for an auth, or null when the auth has no such field.
export function authFieldLabel(auth: McpAuth): string | null {
  switch (auth) {
    case "header":
      return "Header name";
    case "query":
      return "Query parameter";
    case "env":
      return "Environment variable";
    default:
      return null;
  }
}

export function authFieldPlaceholder(auth: McpAuth): string {
  switch (auth) {
    case "header":
      return "X-Api-Key";
    case "query":
      return "api_key";
    case "env":
      return "GITHUB_TOKEN";
    default:
      return "";
  }
}

// What the secret must contain, said next to the reference input.
export function secretHint(auth: McpAuth): string {
  switch (auth) {
    case "bearer":
      return "The item holds the token itself.";
    case "basic":
      return "The item holds user:password on one line.";
    case "header":
      return "The item holds the header's value.";
    case "query":
      return "The item holds the parameter's value.";
    case "env":
      return "The item holds the variable's value.";
    default:
      return "";
  }
}

// One argument per line; blank lines are dropped, surrounding spaces trimmed.
export function parseArgs(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map((s) => s.trim())
    .filter(Boolean);
}

export const argsText = (args: string[] | undefined | null): string => (args ?? []).join("\n");

// Env rows → the object the hub takes. A row without a key is ignored; a
// repeated key keeps the last row, like a shell would.
export function envObject(rows: EnvRow[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const r of rows) {
    const key = r.key.trim();
    if (key) out[key] = r.value;
  }
  return out;
}

export const envRows = (env: Record<string, string> | undefined | null): EnvRow[] => Object.entries(env ?? {}).map(([key, value]) => ({ key, value }));

export const ENV_KEY = /^[A-Za-z_][A-Za-z0-9_]*$/;

// Where the server is, for the table: the command line or the url.
export function endpoint(s: Pick<McpServer, "transport" | "command" | "args" | "url">): string {
  if (s.transport === "stdio") return [s.command, ...(s.args ?? [])].join(" ").trim();
  return s.url;
}

export const scopeLabel = (project: string): string => project || "all projects";

export type CheckTone = "neutral" | "ok" | "bad";

// The last check, for a row: "Never checked", or what connect recorded.
export function lastCheck(s: Pick<McpServer, "last_check_at" | "last_check_ok" | "last_check_note">): { tone: CheckTone; label: string; detail: string } {
  if (!s.last_check_at) return { tone: "neutral", label: "Never checked", detail: "" };
  const detail = (s.last_check_note || "").split("\n")[0] ?? "";
  if (s.last_check_ok === true) return { tone: "ok", label: "Connected", detail };
  if (s.last_check_ok === false) return { tone: "bad", label: "Failed", detail };
  return { tone: "neutral", label: "Checked", detail };
}

// Tool names from a check note of the shape "name version: N tools: a, b".
export function toolNames(note: string): string[] {
  const m = /\btools?:\s*([^]+)$/.exec(note || "");
  if (!m) return [];
  return m[1]
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
}

export function formFromServer(s: McpServer): McpForm {
  return { name: s.name, project: s.project, transport: s.transport, command: s.command, args: argsText(s.args), url: s.url, auth: s.auth, auth_header: s.auth_header, secret_ref: s.secret_ref, env: envRows(s.env), note: s.note };
}

// Everything the hub would refuse, said before the request leaves. The
// hub checks again; this only spares the round trip.
export function validateMcpForm(f: McpForm, options: McpOptions | undefined | null = DEFAULT_OPTIONS): string[] {
  const errors: string[] = [];
  const name = f.name.trim();
  if (!name) errors.push("Give the server a name.");
  else if (!SLUG.test(name)) errors.push("The name is a slug: lowercase letters, digits and dashes, starting with a letter or digit.");
  else if (name.length > MAX_NAME) errors.push("The name is longer than " + MAX_NAME + " characters.");
  if (f.transport === "stdio") {
    if (!f.command.trim()) errors.push("A stdio server needs the command to start.");
  } else {
    const url = f.url.trim();
    if (!url) errors.push("An " + f.transport + " server needs its URL.");
    else if (!/^https?:\/\/\S+$/i.test(url)) errors.push("The URL must start with http:// or https://.");
  }
  if (!authsFor(options, f.transport).some((a) => a.name === f.auth)) errors.push("Auth " + f.auth + " does not fit the " + f.transport + " transport.");
  if (needsSecret(f.auth) && !f.secret_ref.trim()) errors.push("Auth " + f.auth + " needs a secret reference (" + (options ?? DEFAULT_OPTIONS).cred_shapes + ").");
  if (f.secret_ref.trim() && !/^(op|keychain|file):\/\/\S+$/.test(f.secret_ref.trim())) errors.push("The secret reference must be op://, keychain:// or file://, never the secret itself.");
  if (authFieldLabel(f.auth) && !f.auth_header.trim()) errors.push("Auth " + f.auth + " needs the " + authFieldLabel(f.auth)!.toLowerCase() + ".");
  for (const r of f.env) {
    const key = r.key.trim();
    if (key && !ENV_KEY.test(key)) errors.push("Environment variable name " + key + " is not valid.");
  }
  return errors;
}

export function mcpInput(f: McpForm): McpServerInput {
  const stdio = f.transport === "stdio";
  return {
    name: f.name.trim(),
    project: f.project,
    transport: f.transport,
    command: stdio ? f.command.trim() : "",
    args: stdio ? parseArgs(f.args) : [],
    url: stdio ? "" : f.url.trim(),
    auth: f.auth,
    auth_header: authFieldLabel(f.auth) ? f.auth_header.trim() : "",
    secret_ref: needsSecret(f.auth) ? f.secret_ref.trim() : "",
    env: stdio ? envObject(f.env) : {},
    note: f.note.trim(),
  };
}

const sameList = (a: string[], b: string[]) => a.length === b.length && a.every((x, i) => x === b[i]);
const sameMap = (a: Record<string, string>, b: Record<string, string>) => {
  const ka = Object.keys(a).sort();
  const kb = Object.keys(b).sort();
  return sameList(ka, kb) && ka.every((k) => a[k] === b[k]);
};

// Only what changed, so an edit that touches the note does not resend the
// reference - the hub rejects an empty patch, so a no-op edit is caught here.
export function mcpPatch(before: McpServer, f: McpForm): McpServerPatch {
  const next = mcpInput(f);
  const patch: McpServerPatch = {};
  if (next.name !== before.name) patch.name = next.name;
  if ((next.project ?? "") !== before.project) patch.project = next.project ?? "";
  if (next.transport !== before.transport) patch.transport = next.transport;
  if ((next.command ?? "") !== before.command) patch.command = next.command;
  if (!sameList(next.args ?? [], before.args ?? [])) patch.args = next.args;
  if ((next.url ?? "") !== before.url) patch.url = next.url;
  if (next.auth !== before.auth) patch.auth = next.auth;
  if ((next.auth_header ?? "") !== before.auth_header) patch.auth_header = next.auth_header;
  if ((next.secret_ref ?? "") !== before.secret_ref) patch.secret_ref = next.secret_ref;
  if (!sameMap(next.env ?? {}, before.env ?? {})) patch.env = next.env;
  if ((next.note ?? "") !== before.note) patch.note = next.note;
  return patch;
}

// ----- skills ----------------------------------------------------------------

export interface SkillForm {
  name: string;
  project: string;
  description: string;
  body: string;
}

export const EMPTY_SKILL_FORM: SkillForm = { name: "", project: "", description: "", body: "" };

export const formFromSkill = (k: Skill): SkillForm => ({ name: k.name, project: k.project, description: k.description, body: k.body });

export function validateSkillForm(f: SkillForm): string[] {
  const errors: string[] = [];
  const name = f.name.trim();
  if (!name) errors.push("Give the skill a name.");
  else if (!SLUG.test(name)) errors.push("The name is a slug: lowercase letters, digits and dashes, starting with a letter or digit.");
  else if (name.length > MAX_NAME) errors.push("The name is longer than " + MAX_NAME + " characters.");
  const description = f.description.trim();
  if (!description) errors.push("Say in one line what the skill covers.");
  else if (/\n/.test(description) || description.length > MAX_DESCRIPTION) errors.push("The description is one line of at most " + MAX_DESCRIPTION + " characters; the body holds the rest.");
  if (!f.body.trim()) errors.push("The body is what the agent reads; it cannot be empty.");
  return errors;
}

export const skillInput = (f: SkillForm): SkillInput => ({ name: f.name.trim(), project: f.project, description: f.description.trim(), body: f.body.replace(/\s+$/, "") + "\n" });

export function skillPatch(before: Skill, f: SkillForm): SkillPatch {
  const next = skillInput(f);
  const patch: SkillPatch = {};
  if (next.name !== before.name) patch.name = next.name;
  if ((next.project ?? "") !== before.project) patch.project = next.project ?? "";
  if (next.description !== before.description) patch.description = next.description;
  if (next.body.trimEnd() !== before.body.trimEnd()) patch.body = next.body;
  return patch;
}

// Global rows first, then by name - the order the brief lists them in.
export function sortScoped<T extends { project: string; name: string }>(rows: T[]): T[] {
  return [...rows].sort((a, b) => (a.project === b.project ? a.name.localeCompare(b.name) : a.project === "" ? -1 : b.project === "" ? 1 : a.project.localeCompare(b.project)));
}
