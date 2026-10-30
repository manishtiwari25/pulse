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
    { name: "oauth", explain: "Signs in with OAuth 2.1 once, from here or a terminal (pulse mcp login NAME); PULSE registers itself with the server's authorization server when it can, keeps the tokens on that machine and refreshes them. No secret reference.", transports: ["http", "sse"] },
  ],
  cred_shapes: "op://VAULT/ITEM/FIELD[?ssh-format=openssh], keychain://SERVICE/ACCOUNT, or file:///ABSOLUTE/PATH",
};

export const TRANSPORT_LABEL: Record<McpTransport, string> = {
  stdio: "Run a program",
  http: "Web address",
  sse: "Older web stream",
};

// The short name a row shows next to its endpoint, with the technical one in brackets.
export const TRANSPORT_SHORT: Record<McpTransport, string> = {
  stdio: "Program (stdio)",
  http: "Web (HTTP)",
  sse: "Web stream (SSE)",
};

export const TRANSPORT_BLURB: Record<McpTransport, string> = {
  stdio: "PULSE starts a program on this machine and talks to it directly (stdio).",
  http: "The server answers at one web address. Most hosted servers work this way (HTTP).",
  sse: "An older style: an event stream plus an address the server announces (SSE).",
};

// How a server checks who is calling, in words.
export const AUTH_LABEL: Record<McpAuth, string> = {
  none: "No sign-in",
  bearer: "Access token",
  basic: "User name and password",
  header: "Secret in a header",
  query: "Secret in the web address",
  env: "Secret in an environment variable",
  oauth: "OAuth sign-in",
};

const AUTH_PLAIN: Record<McpAuth, string> = {
  none: "The server needs no secret: it is open, or trusts this machine.",
  bearer: "PULSE sends the secret as an access token (Authorization: Bearer).",
  basic: "PULSE signs in with a user name and password kept together as user:password.",
  header: "PULSE sends the secret in a header you name, such as X-Api-Key.",
  query: "PULSE adds the secret to the web address as a parameter you name.",
  env: "PULSE hands the secret to the program as an environment variable you name.",
  oauth: "You sign in once, from this page (Sign in) or a terminal (pulse mcp login NAME). PULSE registers itself with the server when it can - GitHub makes you create the app yourself - keeps the tokens on this machine and renews them; there is no secret to store.",
};

// The auths a transport can use, in catalogue order.
export function authsFor(options: McpOptions | undefined | null, transport: McpTransport): McpAuthOption[] {
  return (options ?? DEFAULT_OPTIONS).auths.filter((a) => a.transports.includes(transport));
}

// The plain explanation for the auths this console knows; the hub's own words for any other.
export function authExplain(options: McpOptions | undefined | null, auth: McpAuth): string {
  return AUTH_PLAIN[auth] ?? (options ?? DEFAULT_OPTIONS).auths.find((a) => a.name === auth)?.explain ?? "";
}

export const authLabel = (auth: McpAuth): string => AUTH_LABEL[auth] ?? auth;

// OAuth keeps its tokens on the machine that signed in; nothing to reference here.
export const needsSecret = (auth: McpAuth): boolean => auth !== "none" && auth !== "oauth";

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
      return "The saved item holds the token itself.";
    case "basic":
      return "The saved item holds user:password on one line.";
    case "header":
      return "The saved item holds the header's value.";
    case "query":
      return "The saved item holds the parameter's value.";
    case "env":
      return "The saved item holds the variable's value.";
    default:
      return "";
  }
}

// The words of one line of a command line, split like a POSIX shell would:
// on unquoted whitespace, with '...' and "..." holding their spaces and a
// backslash escaping the next character outside single quotes. The quotes
// themselves are dropped. An unclosed quote runs to the end of the line.
export function shellWords(line: string): string[] {
  const out: string[] = [];
  let word = "";
  let inWord = false;
  let quote: "'" | '"' | "" = "";
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (quote === "'") {
      if (c === "'") quote = "";
      else word += c;
    } else if (quote === '"') {
      if (c === '"') quote = "";
      else if (c === "\\" && i + 1 < line.length && '"\\$`'.includes(line[i + 1])) word += line[++i];
      else word += c;
    } else if (c === "'" || c === '"') {
      quote = c;
      inWord = true;
    } else if (c === "\\" && i + 1 < line.length) {
      word += line[++i];
      inWord = true;
    } else if (/\s/.test(c)) {
      if (inWord) out.push(word);
      word = "";
      inWord = false;
    } else {
      word += c;
      inWord = true;
    }
  }
  if (inWord) out.push(word);
  return out;
}

// The arguments of the form: one per line, or a line that holds several
// separated by spaces as they would be typed in a shell (quote one that holds
// spaces). Blank lines are dropped. Pasting `-y @azure/mcp@latest server
// start` on one line thus gives four arguments, never one the program does
// not know.
export function parseArgs(text: string): string[] {
  return text.split(/\r?\n/).flatMap(shellWords);
}

// A command pasted with its arguments (`npx -y @azure/mcp@latest server
// start`) becomes the program alone and the arguments it carried, in front of
// the ones in the Arguments box. Quote a program path that holds spaces.
export function splitCommandLine(command: string, args: string[]): { command: string; args: string[] } {
  const trimmed = command.trim();
  if (!/\s|["']/.test(trimmed)) return { command: trimmed, args };
  const words = shellWords(trimmed);
  if (words.length === 0) return { command: "", args };
  return { command: words[0], args: [...words.slice(1), ...args] };
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

// The scope as a label of its own ("All projects", "Only demo-app").
export const scopeTitle = (project: string): string => (project ? "Only " + project : "All projects");

export type CheckTone = "neutral" | "ok" | "bad";

// The last check, for a row: "Never checked", or what connect recorded.
export function lastCheck(s: Pick<McpServer, "last_check_at" | "last_check_ok" | "last_check_note">): { tone: CheckTone; label: string; detail: string } {
  if (!s.last_check_at) return { tone: "neutral", label: "Never checked", detail: "" };
  const detail = (s.last_check_note || "").split("\n")[0] ?? "";
  if (s.last_check_ok === true) return { tone: "ok", label: "Connected", detail };
  if (s.last_check_ok === false) return { tone: "bad", label: "Failed", detail };
  return { tone: "neutral", label: "Checked", detail };
}

// Why a check failed, in words: a note about the credential means PULSE
// could not read the secret; anything else is the server's own message.
export function checkReason(detail: string): string {
  const d = (detail || "").trim();
  if (!d) return "";
  if (/^credential\b/i.test(d)) return "PULSE couldn't read the secret";
  return d;
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
const NAME_RULE = "Use only lowercase letters, digits and dashes in the name, starting with a letter or digit (like github or release-check).";

// The required fields are filled, so the primary button can be pressed; the
// finer checks (name shape, URL scheme, secret reference) still run on submit.
export function mcpFormReady(f: Pick<McpForm, "name" | "transport" | "command" | "url">): boolean {
  return !!f.name.trim() && !!(f.transport === "stdio" ? f.command.trim() : f.url.trim());
}

export function validateMcpForm(f: McpForm, options: McpOptions | undefined | null = DEFAULT_OPTIONS): string[] {
  const errors: string[] = [];
  const name = f.name.trim();
  if (!name) errors.push("Give the tool a name.");
  else if (!SLUG.test(name)) errors.push(NAME_RULE);
  else if (name.length > MAX_NAME) errors.push("Keep the name to " + MAX_NAME + " characters or fewer.");
  if (f.transport === "stdio") {
    if (!f.command.trim()) errors.push("Say which command starts the program.");
  } else {
    const url = f.url.trim();
    if (!url) errors.push("Give the server's web address (URL).");
    else if (!/^https?:\/\/\S+$/i.test(url)) errors.push("The web address must start with http:// or https://.");
  }
  if (!authsFor(options, f.transport).some((a) => a.name === f.auth)) errors.push(authLabel(f.auth) + " doesn't work with " + TRANSPORT_LABEL[f.transport].toLowerCase() + ". Pick another sign-in.");
  if (needsSecret(f.auth) && !f.secret_ref.trim()) errors.push("Say where the secret is kept: a secret reference such as " + (options ?? DEFAULT_OPTIONS).cred_shapes + ".");
  if (needsSecret(f.auth) && f.secret_ref.trim() && !/^(op|keychain|file):\/\/\S+$/.test(f.secret_ref.trim())) errors.push("Enter a reference that starts with op://, keychain:// or file://, never the secret itself.");
  if (authFieldLabel(f.auth) && !f.auth_header.trim()) errors.push("Fill in the " + authFieldLabel(f.auth)!.toLowerCase() + " the secret goes in.");
  for (const r of f.env) {
    const key = r.key.trim();
    if (key && !ENV_KEY.test(key)) errors.push("The variable name " + key + " is not valid: use letters, digits and underscores, and don't start with a digit.");
  }
  return errors;
}

export function mcpInput(f: McpForm): McpServerInput {
  const stdio = f.transport === "stdio";
  const program = stdio ? splitCommandLine(f.command, parseArgs(f.args)) : { command: "", args: [] };
  return {
    name: f.name.trim(),
    project: f.project,
    transport: f.transport,
    command: program.command,
    args: program.args,
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

export function skillFormReady(f: Pick<SkillForm, "name" | "description" | "body">): boolean {
  return !!f.name.trim() && !!f.description.trim() && !!f.body.trim();
}

export function validateSkillForm(f: SkillForm): string[] {
  const errors: string[] = [];
  const name = f.name.trim();
  if (!name) errors.push("Give the skill a name.");
  else if (!SLUG.test(name)) errors.push(NAME_RULE);
  else if (name.length > MAX_NAME) errors.push("Keep the name to " + MAX_NAME + " characters or fewer.");
  const description = f.description.trim();
  if (!description) errors.push("Say in one line what the skill covers and when to use it.");
  else if (/\n/.test(description) || description.length > MAX_DESCRIPTION) errors.push("Keep the description to one line of " + MAX_DESCRIPTION + " characters or fewer; the instructions hold the rest.");
  if (!f.body.trim()) errors.push("Write the instructions: they are what the agent reads.");
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
