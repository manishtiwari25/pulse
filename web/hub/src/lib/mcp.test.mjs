import assert from "node:assert/strict";
import { test } from "node:test";
import { DEFAULT_OPTIONS, authFieldLabel, authsFor, endpoint, envObject, envRows, formFromServer, lastCheck, mcpFormReady, mcpInput, mcpPatch, parseArgs, skillFormReady, skillPatch, sortScoped, toolNames, validateMcpForm, validateSkillForm } from "./mcp.ts";

const server = {
  id: "MCP-001",
  name: "github",
  project: "",
  transport: "stdio",
  command: "github-mcp-server",
  args: ["stdio"],
  url: "",
  auth: "env",
  auth_header: "GITHUB_PERSONAL_ACCESS_TOKEN",
  secret_ref: "op://PULSE/github-mcp/token",
  env: { GITHUB_HOST: "https://github.com" },
  note: "Issues and pull requests of every project.",
  status: "enabled",
  last_check_at: "2026-09-07T08:00:00Z",
  last_check_ok: true,
  last_check_note: "github-mcp 1.2.0: 3 tools: list_issues, get_issue, create_issue",
  created_by: "human",
  created_at: "2026-09-01T09:00:00Z",
  updated_at: "2026-09-07T08:00:00Z",
};

const form = (over = {}) => ({ ...formFromServer(server), ...over });

test("each transport offers only the auths the hub accepts for it", () => {
  assert.deepEqual(
    authsFor(DEFAULT_OPTIONS, "stdio").map((a) => a.name),
    ["none", "env"],
  );
  assert.deepEqual(
    authsFor(DEFAULT_OPTIONS, "http").map((a) => a.name),
    ["none", "bearer", "basic", "header", "query", "oauth"],
  );
  assert.deepEqual(authsFor(null, "sse").map((a) => a.name), authsFor(DEFAULT_OPTIONS, "sse").map((a) => a.name));
  assert.equal(authFieldLabel("header"), "Header name");
  assert.equal(authFieldLabel("query"), "Query parameter");
  assert.equal(authFieldLabel("env"), "Environment variable");
  assert.equal(authFieldLabel("bearer"), null);
  assert.equal(authFieldLabel("none"), null);
  assert.equal(authFieldLabel("oauth"), null);
});

test("the form refuses what the hub would refuse, before the request", () => {
  assert.deepEqual(validateMcpForm(form()), []);
  assert.match(validateMcpForm(form({ name: "GitHub!" }))[0], /lowercase letters/);
  assert.match(validateMcpForm(form({ name: "" }))[0], /name/);
  assert.match(validateMcpForm(form({ command: "" }))[0], /command/);
  assert.match(validateMcpForm(form({ transport: "http", auth: "none", url: "" }))[0], /URL/);
  assert.match(validateMcpForm(form({ transport: "http", auth: "none", url: "ftp://x" }))[0], /http/);
  assert.match(validateMcpForm(form({ transport: "http", auth: "env", url: "https://x" }))[0], /doesn't work with/);
  // OAuth signs in from a terminal: no secret reference, and a stale one is ignored.
  assert.deepEqual(validateMcpForm(form({ transport: "http", auth: "oauth", url: "https://x", secret_ref: "" })), []);
  assert.deepEqual(validateMcpForm(form({ transport: "http", auth: "oauth", url: "https://x", secret_ref: "left over" })), []);
  assert.match(validateMcpForm(form({ transport: "stdio", auth: "oauth" }))[0], /doesn't work with/);
  assert.match(validateMcpForm(form({ secret_ref: "" }))[0], /secret reference/);
  assert.match(validateMcpForm(form({ secret_ref: "ghp_abcdefghijklmnopqrstuvwxyz" }))[0], /never the secret itself/);
  assert.match(validateMcpForm(form({ auth_header: "" }))[0], /environment variable/);
  assert.match(validateMcpForm(form({ env: [{ key: "1BAD", value: "x" }] }))[0], /not valid/);
  // A blank env row is skipped, not refused.
  assert.deepEqual(validateMcpForm(form({ env: [{ key: "", value: "" }] })), []);
});

test("the input drops what the transport and auth do not use", () => {
  const http = mcpInput(form({ transport: "http", url: " https://mcp.example.com/mcp ", auth: "bearer", auth_header: "X", env: [{ key: "A", value: "b" }], args: "ignored" }));
  assert.equal(http.url, "https://mcp.example.com/mcp");
  assert.equal(http.command, "");
  assert.deepEqual(http.args, []);
  assert.deepEqual(http.env, {});
  assert.equal(http.auth_header, "");
  assert.equal(http.secret_ref, "op://PULSE/github-mcp/token");
  const open = mcpInput(form({ auth: "none" }));
  assert.equal(open.secret_ref, "");
  assert.equal(mcpInput(form({ transport: "http", url: "https://x", auth: "oauth" })).secret_ref, "");
  assert.equal(open.auth_header, "");
  assert.deepEqual(open.env, { GITHUB_HOST: "https://github.com" });
  assert.deepEqual(parseArgs(" stdio \n\n--read-only\n"), ["stdio", "--read-only"]);
  assert.deepEqual(envObject([{ key: " A ", value: "1" }, { key: "", value: "x" }, { key: "A", value: "2" }]), { A: "2" });
  assert.deepEqual(envRows({ B: "2", A: "1" }), [{ key: "B", value: "2" }, { key: "A", value: "1" }]);
});

test("a patch carries only the changed fields and an unchanged form is empty", () => {
  assert.deepEqual(mcpPatch(server, form()), {});
  assert.deepEqual(mcpPatch(server, form({ note: "Issues only." })), { note: "Issues only." });
  assert.deepEqual(mcpPatch(server, form({ args: "stdio\n--read-only" })), { args: ["stdio", "--read-only"] });
  assert.deepEqual(mcpPatch(server, form({ env: [] })), { env: {} });
  assert.deepEqual(mcpPatch(server, form({ project: "demo-app" })), { project: "demo-app" });
  assert.deepEqual(mcpPatch(server, form({ auth: "none" })), { auth: "none", auth_header: "", secret_ref: "" });
});

test("a row reads its endpoint and last check", () => {
  assert.equal(endpoint(server), "github-mcp-server stdio");
  assert.equal(endpoint({ transport: "http", command: "", args: [], url: "https://x/mcp" }), "https://x/mcp");
  assert.deepEqual(lastCheck(server), { tone: "ok", label: "Connected", detail: "github-mcp 1.2.0: 3 tools: list_issues, get_issue, create_issue" });
  assert.deepEqual(lastCheck({ last_check_at: "", last_check_ok: null, last_check_note: "" }), { tone: "neutral", label: "Never checked", detail: "" });
  assert.deepEqual(lastCheck({ last_check_at: "2026-09-07T08:00:00Z", last_check_ok: false, last_check_note: "connection refused\nmore" }), { tone: "bad", label: "Failed", detail: "connection refused" });
  assert.deepEqual(toolNames(server.last_check_note), ["list_issues", "get_issue", "create_issue"]);
  assert.deepEqual(toolNames("docs 0.1: 1 tool: search"), ["search"]);
  assert.deepEqual(toolNames("connection refused"), []);
});

test("skills validate and patch like servers", () => {
  const skill = { id: "SKILL-001", name: "release-check", project: "", description: "How to verify a release.", body: "# Release\n\nRun the checks.\n", status: "enabled", created_by: "human", created_at: "", updated_at: "", uses: 2 };
  assert.deepEqual(validateSkillForm({ name: "release-check", project: "", description: "ok", body: "x" }), []);
  assert.match(validateSkillForm({ name: "Bad Name", project: "", description: "ok", body: "x" })[0], /lowercase letters/);
  assert.match(validateSkillForm({ name: "ok", project: "", description: "", body: "x" })[0], /one line/);
  assert.match(validateSkillForm({ name: "ok", project: "", description: "a".repeat(201), body: "x" })[0], /200/);
  assert.match(validateSkillForm({ name: "ok", project: "", description: "ok", body: "  " })[0], /instructions/);
  assert.deepEqual(skillPatch(skill, { name: "release-check", project: "", description: "How to verify a release.", body: "# Release\n\nRun the checks." }), {});
  assert.deepEqual(skillPatch(skill, { name: "release-check", project: "demo-app", description: "How to verify a release.", body: "# Release\n\nRun the checks.\n" }), { project: "demo-app" });
  assert.deepEqual(skillPatch(skill, { name: "release-check", project: "", description: "Verify.", body: "new" }), { description: "Verify.", body: "new\n" });
});

test("rows sort global first, then by project and name, as the brief lists them", () => {
  const rows = [
    { project: "demo-app", name: "b" },
    { project: "", name: "z" },
    { project: "demo-app", name: "a" },
    { project: "", name: "c" },
    { project: "blog", name: "q" },
  ];
  assert.deepEqual(
    sortScoped(rows).map((r) => r.project + "/" + r.name),
    ["/c", "/z", "blog/q", "demo-app/a", "demo-app/b"],
  );
});

test("the primary button waits for the required fields", () => {
  assert.equal(mcpFormReady({ name: "", transport: "stdio", command: "", url: "" }), false);
  assert.equal(mcpFormReady({ name: "github", transport: "stdio", command: " ", url: "" }), false);
  assert.equal(mcpFormReady({ name: "github", transport: "stdio", command: "npx", url: "" }), true);
  assert.equal(mcpFormReady({ name: "linear", transport: "http", command: "npx", url: "" }), false);
  assert.equal(mcpFormReady({ name: "linear", transport: "http", command: "", url: "https://mcp.linear.app/mcp" }), true);
  assert.equal(skillFormReady({ name: "release-check", description: "How to tag", body: "" }), false);
  assert.equal(skillFormReady({ name: "release-check", description: "How to tag", body: "## Steps" }), true);
});
