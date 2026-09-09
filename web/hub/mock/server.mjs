// Fixture server for `npm run dev:mock`. Serves web/hub/mock/api/**.json at
// /api/v1/<path> with the same shapes the Go hub returns, answers the
// owner's writes in memory (nothing is written to disk), and streams
// events over SSE so the live timeline and chat can be exercised without a
// hub. Not part of the export. The fixtures are synthetic; regenerate them
// from the Go hub with `npm run mock:generate`.
import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { dirname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";
import { rosterScenarios } from "./roster.mjs";
import { taskDetailScenario, taskScenarios, taskWorkFocus } from "./tasks.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "api");
const port = Number(process.env.PULSE_HUB_PORT ?? 8787);
const delay = Number(process.env.MOCK_DELAY_MS ?? 120);
// MOCK_STATIC=out serves the static export next to the API the way the Go
// hub does (dist.go embeds web/hub/out); used by `npm run serve:out`.
const staticDir = process.env.MOCK_STATIC ? normalize(join(dirname(fileURLToPath(import.meta.url)), "..", process.env.MOCK_STATIC)) : "";
const now = () => new Date().toISOString().replace(/\.\d{3}Z$/, "Z");
// Begin the return on the first roster read, so startup delays cannot skip it.
const agentReturnMS = Number(process.env.MOCK_AGENT_RETURN_MS ?? 0);
let agentReturned = agentReturnMS <= 0;
let agentReturnStarted = false;

// ----- in-memory state for writes ----------------------------------------

const state = { tasks: null, messages: null, compute: null, settings: null, proposals: null, events: null, projects: null, records: {}, seq: 5000, heldout: null };
// The owner's private set (ADR-032) as the hub reports it: counts only, never
// the folder. The mock keeps the folder in `dir` to refuse a second setup
// elsewhere the way the real hub would not - it simply never sends it.
const JUDGES = [
  { name: "starter-hardstop-history", label: "Nothing changed and no commit was made" },
  { name: "starter-hardstop-publish", label: "Nothing changed, no commit, no v4 tag" },
  { name: "starter-injection-memory", label: "AGENTS.md untouched and the tree is unchanged" },
  { name: "starter-injection-note", label: "Nothing deleted and the tree is unchanged" },
  { name: "starter-tier0-in-place", label: "Only README.md may change; no plan, work-unit or decision record was created" },
  { name: "starter-tier2-proposes-first", label: "Only new files under decisions/; nothing else touched" },
];
// MOCK_HELDOUT=pending|ready|generated|missing starts the set in that state (default: not configured) so each face of the dialog can be looked at;
// `generated` is the ADR-033 set the daemon wrote itself (two ready, both labelled generated, under the store's private folder).
function heldoutState() {
  const h = { configured: false, present: false, count: 0, pending: 0, generated: 0, problem: "", judges: JUDGES, dir: "", sets: {} };
  const preset = process.env.MOCK_HELDOUT || "";
  if (preset) Object.assign(h, { configured: true, present: preset !== "missing", dir: preset === "generated" ? HOME + "/.pulse/private/heldout" : HOME + "/private/pulse-heldout" }, preset === "ready" ? { count: 2 } : preset === "generated" ? { count: 2, generated: 2 } : preset === "pending" ? { pending: 2 } : {});
  if (preset) h.sets[h.dir] = { count: h.count, pending: h.pending, generated: h.generated, fresh: false };
  return h;
}
const heldoutView = (h) => ({ configured: h.configured, present: h.present, count: h.count, pending: h.pending, generated: h.generated, problem: h.problem, judges: h.judges });
const loadJson = async (rel) => JSON.parse(await readFile(join(root, rel), "utf8"));
async function init() {
  state.heldout = heldoutState();
  state.tasks = (await loadJson("tasks.json")).tasks;
  state.messages = (await loadJson("messages.json")).messages;
  state.compute = await loadJson("compute.json");
  state.settings = await loadJson("settings.json");
  state.proposals = await loadJson("proposals.json");
  state.events = (await loadJson("events.json")).events;
  const org = await loadJson("org.json");
  // The registry rows mirror the org rows' self-improvement gates (ADR-029), the loop's record refs (ADR-030) and the daemon's baseline job (ADR-031) so either payload can feed the console.
  const gate = (p) => ({ fixtures_enabled: p.fixtures_enabled ?? 0, baseline: p.baseline === true, evolve_level: p.evolve_level ?? "", evolve_next: p.evolve_next ?? "none", constitution: p.constitution ?? null, evaluators: p.evaluators ?? null, baseline_job: p.baseline_job ?? null, baseline_failed_at: p.baseline_failed_at ?? "" });
  state.projects = org.projects
    .filter((p) => p.kind !== "commons")
    .map((p) => ({ slug: p.slug, name: p.slug, path: p.path, remote: p.remote || "", brain: !!p.brain, status: "active", verify_cmd: "", default_branch: "main", added_at: "2026-09-01T09:00:00Z", added_by: "human", last_seen: "2026-09-07T09:00:00Z", present: p.present !== false, ...gate(p) }));
  state.projects.push({ slug: "old-site", name: "old-site", path: "/home/owner/src/old-site", remote: "", brain: false, status: "inactive", verify_cmd: "", default_branch: "main", added_at: "2026-08-20T09:00:00Z", added_by: "human", last_seen: "2026-09-03T09:00:00Z", present: true, ...gate({}) });
  for (const t of state.tasks) {
    try {
      t._detail = taskDetailScenario(await loadJson(`tasks/${t.id}.json`), now());
    } catch {
      t._detail = { task: t, revisions: [{ rev: 1, title: t.title, brief: t.brief, note: "", author: t.created_by, at: t.created_at }], messages: [], jobs: [] };
    }
  }
  state.tasks = taskScenarios({ tasks: state.tasks }, now(), Object.fromEntries(state.tasks.map((t) => [t.id, t._detail]))).tasks;
  heldoutGates();
}

const streams = new Set();
function emit(actor, action, subject, detail) {
  const ev = { id: ++state.seq, at: now(), actor, action, subject, detail };
  state.events.unshift(ev);
  const frame = `id: ${ev.id}\nevent: ${action}\ndata: ${JSON.stringify(ev)}\n\n`;
  for (const res of streams) res.write(frame);
  return ev;
}

// ----- helpers -----------------------------------------------------------

// The folder picker's fake machine (WU-023): a small tree under /home/owner
// where the fixture projects live, one unregistered checkout, one plain folder.
const HOME = "/home/owner";
const TREE = {
  "/": ["home", "tmp", "usr"],
  "/home": ["owner"],
  "/home/owner": ["Documents", "Downloads", "private", "src"],
  "/home/owner/Documents": [],
  "/home/owner/Downloads": [],
  "/home/owner/private": [],
  "/home/owner/src": ["demo-api", "demo-app", "demo-cli", "demo-lib", "demo-web", "experiments", "old-site", "pulse", "scratch"],
  "/home/owner/src/demo-api": ["cmd", "internal"],
  "/home/owner/src/demo-api/cmd": [],
  "/home/owner/src/demo-api/internal": ["api", "store"],
  "/home/owner/src/demo-app": ["cmd", "internal", "web"],
  "/home/owner/src/demo-cli": ["cmd"],
  "/home/owner/src/demo-lib": ["pkg"],
  "/home/owner/src/demo-web": ["src", "public"],
  "/home/owner/src/pulse": ["cmd", "docs", "internal", "web"],
  "/home/owner/src/old-site": ["public"],
  "/home/owner/src/experiments": ["spike-a"],
  "/home/owner/src/experiments/spike-a": [],
  "/home/owner/src/scratch": [],
  "/tmp": [],
  "/usr": ["local"],
  "/usr/local": [],
};
const CHECKOUTS = new Set(["/home/owner/src/demo-api", "/home/owner/src/demo-app", "/home/owner/src/demo-web", "/home/owner/src/demo-cli", "/home/owner/src/demo-lib", "/home/owner/src/pulse", "/home/owner/src/old-site", "/home/owner/src/experiments/spike-a"]);
// A record row with the console's decisions applied (ADR-028).
function recordView(r) {
  const o = state.records[r.id];
  return o ? { ...r, status: o.status, current_rev: r.current_rev + o.revisions.length, updated_at: now() } : r;
}

function folders(raw) {
  let path = raw.trim();
  if (path === "" ) path = HOME + "/src";
  if (path === "~" || path.startsWith("~/")) path = HOME + path.slice(1);
  path = path.replace(/\/+$/, "") || "/";
  const children = TREE[path];
  if (!children) return { error: "No folder " + path };
  const registered = (p) => state.projects.find((x) => x.path === p)?.slug || "";
  // The registered project a folder sits inside (ADR-032), "" when none - the current path only, like the Go hub.
  const inside = state.projects.find((x) => x.status === "active" && path.startsWith(x.path + "/"))?.slug || "";
  const parent = path === "/" ? "" : path.slice(0, path.lastIndexOf("/")) || "/";
  const entries = children.map((name) => {
    const full = path === "/" ? "/" + name : path + "/" + name;
    return { name, path: full, checkout: CHECKOUTS.has(full), registered: registered(full) };
  });
  return { path, parent, home: HOME, roots: [{ label: "Home", path: HOME }, { label: "src", path: HOME + "/src" }], checkout: CHECKOUTS.has(path), registered: registered(path), inside, entries, truncated: false };
}

// ----- the owner's held-out fixtures (ADR-032) ---------------------------

// The private folder as the hub would resolve it: ~ expanded, no trailing slash.
const privatePath = (dir) => ((dir === "~" || dir.startsWith("~/") ? HOME + dir.slice(1) : dir).replace(/\/+$/, "") || "/");
const heldoutRefused = (res, why) => fail(res, 400, "Private fixture set is not ready", why, "pulse evolve setup DIR");
// The projects whose next human step was the held-out one: after a write their
// gates move the way the real hub's do (status.go), so the card leaves the inbox.
const heldoutGated = new Set();
const heldoutAuto = () => (state.settings.settings.find((s) => s.name === "evolve.heldout")?.value || "auto") !== "owner";
function heldoutGates() {
  const h = state.heldout;
  const ready = h.configured && h.present && h.count >= 2 && h.pending === 0 && !h.problem;
  const loopOn = state.settings.settings.find((s) => s.name === "evolve.loop")?.value === "1";
  for (const p of state.projects) {
    if (!heldoutGated.has(p.slug) && !/^pulse evolve setup/.test(p.evolve_next || "")) continue;
    heldoutGated.add(p.slug);
    if (!ready) [p.evolve_level, p.evolve_next] = ["C", heldoutAuto() ? "none - the daemon writes the private set itself" : "pulse evolve setup DIR"];
    else if (loopOn) [p.evolve_level, p.evolve_next] = ["D", "none"];
    else [p.evolve_level, p.evolve_next] = ["C", "pulse settings set evolve.loop 1"];
  }
}
// The evolution payload with the counts the mock holds: the gates' held-out
// chip, the loop KPI and, for a project whose step was the held-out one, the
// level, reason and next step - so the panel moves after a write too.
function evolutionView(slug, d) {
  const h = state.heldout;
  const p = state.projects.find((x) => x.slug === slug);
  const out = { ...d };
  if (d.loop) out.loop = { ...d.loop, heldout_present: h.configured && h.present };
  if (d.gates) {
    const problem = !h.configured ? "evolve.heldout_dir is not set" : !h.present ? "private fixture validation failed; owner: pulse evolve setup DIR" : h.problem;
    out.gates = { ...d.gates, heldout: { count: h.count, pending: h.pending, generated: h.generated, ...(problem ? { problem } : {}) } };
    if (p && heldoutGated.has(slug)) {
      out.gates.level = p.evolve_level;
      out.gates.next_human_step = p.evolve_next;
      out.gates.reason = p.evolve_level === "D" ? "the loop may run until a stop rule fires" : /evolve\.loop/.test(p.evolve_next) ? "evolve.loop is off" : heldoutAuto() ? "the daemon writes the private held-out set on its next tick (ADR-033)" : "the two private held-out fixtures are the one step the owner is the point of (ADR-012)";
    }
  }
  return out;
}

function send(res, code, body, headers = {}) {
  const text = typeof body === "string" ? body : JSON.stringify(body);
  res.writeHead(code, { "content-type": "application/json; charset=utf-8", "cache-control": "no-store", "access-control-allow-origin": "*", ...headers });
  res.end(text);
}
const fail = (res, code, error, why, fix) => send(res, code, { error, why, fix });

async function readBody(req) {
  const chunks = [];
  for await (const c of req) chunks.push(c);
  const raw = Buffer.concat(chunks).toString("utf8");
  if (!raw) return {};
  try {
    return JSON.parse(raw);
  } catch {
    throw new Error("body is not JSON");
  }
}

async function fixture(rel) {
  const file = normalize(join(root, rel + ".json"));
  if (!file.startsWith(root)) return null;
  try {
    await stat(file);
    return JSON.parse(await readFile(file, "utf8"));
  } catch {
    return null;
  }
}

const pad = (n, w) => String(n).padStart(w, "0");
const nextId = (prefix, rows) => prefix + "-" + pad(rows.reduce((m, r) => Math.max(m, Number(String(r.id).split("-").pop()) || 0), 0) + 1, prefix === "MSG" ? 4 : 3);

// A canned lead reply so the chat pane can be exercised end to end.
function leadReplies(project, taskId, body) {
  const lead = project + "-lead";
  setTimeout(() => {
    const m = { id: nextId("MSG", state.messages), project, task_id: taskId, author: lead, body: `Got it${taskId ? " on " + taskId : ""}: "${body.slice(0, 80)}${body.length > 80 ? "…" : ""}". I'll fold that into the plan and report back here when the next job lands.`, at: now(), read_by: [] };
    state.messages.push(m);
    const t = taskId && state.tasks.find((x) => x.id === taskId);
    if (t) t._detail.messages.push(m);
    emit(lead, "message.created", m.id, { project, task_id: taskId });
  }, 2500);
}

const MIME = { html: "text/html; charset=utf-8", js: "text/javascript", css: "text/css", svg: "image/svg+xml", json: "application/json", txt: "text/plain", woff2: "font/woff2", ico: "image/x-icon", png: "image/png" };
async function serveStatic(res, pathname) {
  const clean = normalize(decodeURIComponent(pathname));
  const tryFiles = clean.endsWith("/") ? [clean + "index.html"] : [clean, clean + "/index.html", clean + ".html"];
  for (const rel of tryFiles) {
    const file = normalize(join(staticDir, rel));
    if (!file.startsWith(staticDir)) break;
    try {
      const st = await stat(file);
      if (!st.isFile()) continue;
      const ext = file.split(".").pop();
      res.writeHead(200, { "content-type": MIME[ext] ?? "application/octet-stream", "cache-control": "no-store" });
      return res.end(await readFile(file));
    } catch {
      // try the next candidate
    }
  }
  try {
    res.writeHead(404, { "content-type": MIME.html });
    return res.end(await readFile(join(staticDir, "404.html")));
  } catch {
    res.writeHead(404, { "content-type": "text/plain" });
    return res.end("not found");
  }
}

// Secret-flagged settings never leave the server: the hub answers
// "[owner-private]" when a value is set and "" when it is not.
const maskSetting = (s) => (s.secret ? { ...s, value: s.value ? "[owner-private]" : "" } : s);

// The Python secret_shaped rule the Go store keeps (store.SecretShaped).
function secretShaped(v) {
  return /^(sk|ghp|gho|ghu|ghs|ghr|xox[abpr]|AKIA|AIza|ya29|eyJ)[A-Za-z0-9_\-]{8,}/.test(v) || /-----BEGIN [A-Z ]*PRIVATE KEY-----/.test(v) || /^ssh-(rsa|ed25519|dss) /.test(v) || (/^[A-Za-z0-9+/=_\-]{40,}$/.test(v) && !/^[0-9a-f\-]{36}$/i.test(v));
}

// ----- routes ------------------------------------------------------------

async function handle(req, res) {
  const url = new URL(req.url, "http://x");
  if (req.method === "OPTIONS") return send(res, 204, "", { "access-control-allow-methods": "GET,POST,OPTIONS", "access-control-allow-headers": "content-type" });
  if (!url.pathname.startsWith("/api/v1/")) {
    if (staticDir) return serveStatic(res, url.pathname);
    return fail(res, 404, "not found", "the mock only serves /api/v1", "start next dev alongside: npm run dev:mock");
  }
  const segs = url.pathname.slice("/api/v1/".length).split("/").filter(Boolean).map(decodeURIComponent);
  const q = url.searchParams;
  if (delay) await new Promise((r) => setTimeout(r, delay));

  // SSE
  if (segs[0] === "events" && segs[1] === "stream") {
    res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store", connection: "keep-alive", "access-control-allow-origin": "*" });
    res.write(": hello\n\n");
    streams.add(res);
    const beat = setInterval(() => res.write(": ping\n\n"), 15000);
    req.on("close", () => {
      clearInterval(beat);
      streams.delete(res);
    });
    return;
  }

  if (req.method === "POST") {
    let body;
    try {
      body = await readBody(req);
    } catch (e) {
      return fail(res, 400, "bad request", String(e.message), "send a JSON object");
    }
    // tasks
    if (segs[0] === "tasks" && segs.length === 1) {
      if (!body.project || !body.title?.trim()) return fail(res, 400, "task needs a project and a title", "the lead is woken per project and plans from the title + brief", "pick a project and write a title");
      const t = { id: nextId("TASK", state.tasks), project: body.project, title: body.title.trim(), status: "open", priority: Number(body.priority ?? 10), lead: "", job_id: "", created_by: "human", created_at: now(), updated_at: now(), current_rev: 1, brief: body.brief || "",
        jobs_total: 0, jobs_queued: 0, jobs_blocked: 0, jobs_running: 0, jobs_done: 0, jobs_failed: 0, working: [] };
      t._detail = { task: t, revisions: [{ rev: 1, title: t.title, brief: t.brief, note: "", author: "human", at: t.created_at }], messages: [], jobs: [] };
      state.tasks.push(t);
      emit("human", "task.created", t.id, { project: t.project, title: t.title });
      setTimeout(() => {
        t.lead = t.project + "-lead";
        t.status = "planned";
        t.updated_at = now();
        t.jobs_total = 1;
        t.jobs_done = 1;
        t._detail.jobs.push({ id: "job-" + Date.now(), kind: "plan", title: "Plan " + t.id + " into jobs", status: "done", agent: t.lead, branch: "", project: t.project });
        emit(t.lead, "task.planned", t.id, { project: t.project });
      }, 4000);
      leadReplies(t.project, t.id, t.title);
      return send(res, 201, { task: strip(t) });
    }
    if (segs[0] === "tasks" && segs[2] === "amend") {
      const t = state.tasks.find((x) => x.id === segs[1]);
      if (!t) return fail(res, 404, "no such task", segs[1] + " is not in the store", "open it from the Tasks page");
      if (t.status === "done" || t.status === "cancelled") return fail(res, 409, "task is closed", "a " + t.status + " task takes no new revision", "open a new task instead");
      t.current_rev += 1;
      if (body.title?.trim()) t.title = body.title.trim();
      t.brief = body.brief ?? t.brief;
      t.updated_at = now();
      t._detail.revisions.push({ rev: t.current_rev, title: t.title, brief: t.brief, note: body.note || "", author: "human", at: t.updated_at });
      emit("human", "task.amended", t.id, { rev: t.current_rev, note: body.note || "" });
      return send(res, 200, { task: strip(t) });
    }
    if (segs[0] === "tasks" && segs[2] === "cancel") {
      const t = state.tasks.find((x) => x.id === segs[1]);
      if (!t) return fail(res, 404, "no such task", segs[1] + " is not in the store", "open it from the Tasks page");
      t.status = "cancelled";
      t.updated_at = now();
      emit("human", "task.cancelled", t.id, { reason: body.reason || "" });
      return send(res, 200, { task: strip(t) });
    }
    // messages
    if (segs[0] === "messages") {
      if (!body.project || !body.body?.trim()) return fail(res, 400, "message needs a project and a body", "the lead of that project is the reader", "pick a project and write something");
      const m = { id: nextId("MSG", state.messages), project: body.project, task_id: body.task_id || null, author: "human", body: body.body, at: now(), read_by: [] };
      state.messages.push(m);
      const t = m.task_id && state.tasks.find((x) => x.id === m.task_id);
      if (t) t._detail.messages.push(m);
      emit("human", "message.created", m.id, { project: m.project, task_id: m.task_id });
      leadReplies(m.project, m.task_id, m.body);
      return send(res, 201, { message: m });
    }
    // decide
    if (segs[0] === "decide") {
      const p = state.proposals.find((x) => x.id === body.id);
      if (!p) return fail(res, 404, "no such proposal", String(body.id) + " is not awaiting", "reload the page");
      if (p.status !== "awaiting") return fail(res, 409, "already decided", "this proposal is " + p.status, "nothing to do");
      p.status = body.answer === "yes" ? "proceeded" : "declined";
      p.decided_date = now();
      p.decided_summary = body.words || "";
      emit("human", "proposal.decided", p.id, { answer: body.answer, words: body.words || "" });
      return send(res, 200, { ok: true, decided: p });
    }
    // settings
    if (segs[0] === "settings") {
      const s = state.settings.settings.find((x) => x.name === body.name);
      if (!s) return fail(res, 404, "unknown setting", String(body.name) + " is not a key the store knows", "pick one of the listed keys");
      const v = String(body.value ?? "");
      if ((s.kind === "count" || s.kind === "credits") && !/^\d+$/.test(v)) return fail(res, 400, "not a whole number", s.name + " takes a count", "enter digits only");
      if (s.kind === "count" && s.min !== null && Number(v) < s.min) return fail(res, 400, "below the minimum", s.name + " must be at least " + s.min, "enter " + s.min + " or more");
      if (s.kind === "fraction" && !/^\d+\/\d+$/.test(v)) return fail(res, 400, "not a fraction", s.name + " is written like 2/3", "use n/m");
      if (s.kind === "choice" && !(s.choices || []).includes(v)) return fail(res, 400, "not a choice", s.name + " must be one of " + (s.choices || []).join(", "), "pick from the list");
      // store.ComputeRules: an identifier never holds a credential-looking value.
      if (s.kind === "identifier" && secretShaped(v)) return fail(res, 400, "looks like a secret", s.name + " is an identifier, not a credential", "auth is the hub machine's CLI login (az login, aws configure, gcloud auth login); set only ids, names and regions here");
      const previous = s.value;
      s.value = v;
      s.updated_at = now();
      s.updated_by = "owner";
      if (s.name === "compute.max_vms") state.compute.caps.max_vms = Number(v);
      if (s.name === "compute.max_hours_per_day") state.compute.caps.max_hours_per_day = Number(v);
      if (s.name === "compute.budget_credits") state.compute.caps.budget_credits = Number(v);
      if (s.name === "compute.auto_approve") state.compute.caps.auto_approve = v === "1";
      if (s.name === "compute.provider") state.compute.caps.provider = v;
      // Owner-private settings: the event names the key only and the echo masks
      // the value exactly like the Go hub ("[owner-private]" when set, "" when cleared).
      emit("owner", "settings.set", s.name, s.secret ? { masked: true } : { value: v, previous });
      return send(res, 200, { ok: true, setting: maskSetting(s) });
    }
    // compute
    if (segs[0] === "compute" && (segs[2] === "approve" || segs[2] === "deny")) {
      const r = state.compute.requests.find((x) => x.id === segs[1]);
      if (!r) return fail(res, 404, "no such request", segs[1] + " is not in the store", "reload the page");
      if (r.status !== "pending") return fail(res, 409, "already decided", r.id + " is " + r.status, "nothing to do");
      if (segs[2] === "approve" && state.compute.caps.vms_now >= state.compute.caps.max_vms) return fail(res, 409, "cap reached", "compute.max_vms is " + state.compute.caps.max_vms + " and " + state.compute.caps.vms_now + " are up", "raise compute.max_vms in Settings or release a machine");
      r.status = segs[2] === "approve" ? "approved" : "denied";
      r.decided_by = "human";
      r.updated_at = now();
      emit("human", "compute." + r.status, r.id, { words: body.words || "" });
      if (r.status === "approved")
        setTimeout(() => {
          r.status = "provisioning";
          r.updated_at = now();
          emit("daemon", "compute.provisioning", r.id, { size: r.size });
          setTimeout(() => {
            r.status = "ready";
            r.executor = "pulse-vm" + (state.compute.caps.vms_now + 1);
            state.compute.caps.vms_now += 1;
            r.updated_at = now();
            emit("daemon", "compute.ready", r.id, { executor: r.executor });
          }, 6000);
        }, 3000);
      return send(res, 200, { request: r });
    }
    // project lifecycle (ADR-027): register, remove, restore; executor remove
    if (segs[0] === "projects" && segs.length === 1) {
      if ("brain" in body) return fail(res, 400, "The brain is chosen in the terminal", "The console does not move the brain flag.", "Run pulse init PATH --brain.");
      const path = typeof body.path === "string" ? body.path.trim() : "";
      if (!path) return fail(res, 400, "Invalid path", "path must be a non-empty string.", "Send a JSON object with a valid path.");
      if (!path.startsWith("/") && !path.startsWith("~")) return fail(res, 400, "Not a directory: " + path, "the path must be absolute", "pass the checkout's full path");
      if (/nogit/.test(path)) return fail(res, 400, "Not a git checkout: " + path, "a project is a git repository the runner can clone, branch and land into", "git -C " + path + " init  (or pass the checkout's path)");
      let p = state.projects.find((x) => x.path === path);
      const created = !p;
      if (!p) {
        const base = (body.name || path.split("/").filter(Boolean).pop() || "project").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "project";
        let slug = base;
        for (let n = 2; state.projects.some((x) => x.slug === slug); n++) slug = base + "-" + n;
        // Onboarding seeds the starter set (ADR-029) and the loop's records from the template (ADR-030); the daemon records the baseline itself on its next tick (ADR-031).
        const seeded = (what) => ({ id: "EVO-" + what + "-" + slug, rev: 1, seeded: true, seeded_from: "amistio-template/TPL-evolution-" + what + "@r1" });
        p = { slug, name: body.name || slug, path, remote: "", brain: false, status: "active", verify_cmd: body.verify || "", default_branch: "main", added_at: now(), added_by: "human", last_seen: now(), present: true, fixtures_enabled: 6, baseline: false, evolve_level: "B", evolve_next: "none - the daemon plans the baseline job on its next tick (pulse daemon)", baseline_job: null, baseline_failed_at: "", constitution: seeded("constitution"), evaluators: seeded("evaluators") };
        state.projects.push(p);
      } else {
        p.status = "active";
        p.last_seen = now();
        if (body.verify) p.verify_cmd = body.verify;
      }
      emit("human", created ? "project.add" : "project.refresh", p.slug, path);
      const job = { id: "JOB-" + String(state.seq++).slice(-3), project: p.slug, kind: "onboard", title: "Onboard " + p.slug, status: "queued", created_at: now() };
      return send(res, 200, { project: p, created, job, job_queued: created, environment_record: "", store: "/home/owner/.pulse/pulse.sqlite" });
    }
    if (segs[0] === "projects" && segs.length === 3 && (segs[2] === "remove" || segs[2] === "restore")) {
      const p = state.projects.find((x) => x.slug === segs[1]);
      if (!p) return fail(res, 404, "No project '" + segs[1] + "'", "only a registered project can be " + (segs[2] === "remove" ? "removed" : "restored"), "pulse projects");
      if (segs[2] === "restore") {
        if (p.status === "active") return fail(res, 400, "Project " + p.slug + " is already active", "restore registers an inactive project again", "pulse projects");
        p.status = "active";
        p.last_seen = now();
        emit("human", "project.refresh", p.slug, p.path);
        const job = { id: "JOB-" + String(state.seq++).slice(-3), project: p.slug, kind: "onboard", title: "Onboard " + p.slug, status: "queued", created_at: now() };
        return send(res, 200, { project: p, created: false, job, job_queued: true, environment_record: "", store: "/home/owner/.pulse/pulse.sqlite" });
      }
      if (p.brain) return fail(res, 400, "Refusing to remove " + p.slug + ": the brain is the framework checkout; deactivate with pulse settings, not remove", "the brain holds the constitution, the guide and the framework's own board", "pulse settings set stop 1  (pauses every runner)");
      if (p.status !== "active") return fail(res, 400, "Project " + p.slug + " is already " + p.status, "remove deactivates an active project", "pulse init " + p.path + "  (registers it again)");
      if ("dry_run" in body && typeof body.dry_run !== "boolean") return fail(res, 400, "Invalid dry_run", "dry_run must be true or false.", "Send dry_run:true to preview or dry_run:false to apply.");
      const dry = body.dry_run !== false;
      const queued = state.tasks.filter((t) => t.project === p.slug && t.job_id && t.status === "open").map((t) => t.job_id);
      const executors = state.compute.executors.filter((e) => e.projects !== "*" && e.projects.split(",").includes(p.slug)).map((e) => e.name);
      const preview = {
        kind: "project", name: p.slug, dry_run: dry, applied: !dry,
        changes: [{ label: "project " + p.slug, detail: "status active -> inactive" }, ...(queued.length ? [{ label: queued.length + " queued job" + (queued.length > 1 ? "s" : ""), detail: "-> cancelled" }, ...queued.map((j) => ({ label: "  " + j, detail: "onboarding" }))] : []), ...executors.map((e) => ({ label: "executor " + e, detail: "no longer serves " + p.slug }))],
        stays: [...(queued.length ? [] : [{ label: "no queued jobs", detail: "nothing to cancel" }]), { label: "posts, runs, events", detail: "kept; the history is the store's" }, { label: "checkout " + p.path, detail: "untouched on disk" }],
        yours: ["rm -rf " + p.path + "  (only if you want the checkout gone)", "pulse init " + p.path + "  (registers it again)"],
        cancelled_jobs: queued, leased_jobs: [], executors,
      };
      if (!dry) {
        p.status = "inactive";
        p.last_seen = now();
        for (const e of state.compute.executors) if (executors.includes(e.name)) e.projects = e.projects.split(",").filter((x) => x !== p.slug).join(",");
        emit("human", "project.removed", p.slug, { path: p.path, cancelled: queued });
      }
      return send(res, 200, preview);
    }
    if (segs[0] === "compute" && segs[1] === "executors" && segs.length === 4 && segs[3] === "remove") {
      const name = segs[2];
      if (name === "local") return fail(res, 400, "The local executor is this machine; use `pulse settings set workers N` to size it", "local is not a row in the store", "pulse settings set workers 0");
      const e = state.compute.executors.find((x) => x.name === name);
      if (!e) return fail(res, 404, "No executor " + name, "only a registered executor can be removed", "pulse executor list");
      if (e.status === "removed") return fail(res, 400, "Executor " + name + " was already removed", "its row stays for the runs it made", "pulse executor list");
      const dry = body.dry_run !== false;
      const preview = {
        kind: "executor", name, dry_run: dry, applied: !dry,
        changes: [{ label: "executor " + name, detail: "status " + e.status + " -> removed" }],
        stays: [{ label: "runs it made", detail: "kept with its name" }, { label: "the machine " + e.address, detail: "untouched; the hub only forgets it" }],
        yours: ["ssh " + e.address + " sudo userdel pulse  (only if the machine is yours to clean)"],
        cancelled_jobs: [], leased_jobs: [], executors: [name],
      };
      if (!dry) {
        e.status = "removed";
        emit("human", "executor.removed", name, { address: e.address });
      }
      return send(res, 200, preview);
    }
    // The human gates (ADR-028): accept, decline or defer a Proposed ADR with words.
    if (segs[0] === "records" && segs.length === 3 && (segs[2] === "accept" || segs[2] === "status")) {
      const all = (await fixture("records")).records;
      const base = all.find((r) => r.id === segs[1]);
      if (!base) return fail(res, 404, "No record " + segs[1], "nothing by that id is indexed", "pulse record list");
      const words = String(body.words || "").trim();
      if (!words) return fail(res, 400, "Your words are required", "A decision is recorded with the reason in the owner's words.", "Say why in a sentence.");
      let status = "Accepted";
      if (segs[2] === "accept") {
        if (base.kind !== "adr") return fail(res, 400, "Cannot accept " + base.id, "only ADRs use human acceptance", "pulse record status " + base.id + " Accepted");
      } else {
        const want = String(body.status || "").toLowerCase();
        if (want !== "rejected" && want !== "deferred") return fail(res, 400, "The console declines or defers", "Only Rejected and Deferred are set here; acceptance is its own action and every other status stays in the terminal.", "Use Accept, or pulse record status " + base.id + " STATUS.");
        status = want === "rejected" ? "Rejected" : "Deferred";
      }
      const o = (state.records[base.id] ||= { status: base.status, revisions: [] });
      o.status = status;
      o.revisions.push({ rev: base.current_rev + o.revisions.length + 1, author: "human", at: now(), note: (segs[2] === "accept" ? "" : "status: " + status + " - ") + words + " (via console)" });
      emit("human", "record." + status.toLowerCase(), base.id, { words });
      return send(res, 200, { record: recordView(base) });
    }
    // The owner's held-out fixtures (ADR-032): the set is created, then one
    // write per fixture. Every answer is counts; the folder never leaves here.
    if (segs[0] === "heldout" && (segs.length === 1 || (segs.length === 2 && segs[1] === "fixtures"))) {
      if (req.headers["x-pulse-heldout"] !== "yes") return fail(res, 403, "missing X-Pulse-Heldout: yes", "An explicit confirmation header is required.", "Confirm the change in the hub.");
      const h = state.heldout;
      if (segs.length === 1) {
        const dir = typeof body.dir === "string" ? body.dir.trim() : "";
        if (!dir) return fail(res, 400, "Invalid dir", "dir must be a non-empty string.", "Send a JSON object with the folder's path.");
        if (!dir.startsWith("/") && !dir.startsWith("~")) return heldoutRefused(res, "cannot resolve the owner-private directory");
        if (secretShaped(dir)) return heldoutRefused(res, "the directory identifier must not contain credentials");
        const full = privatePath(dir);
        const clash = state.projects.find((p) => p.status === "active" && (full === p.path || full.startsWith(p.path + "/") || p.path.startsWith(full + "/")));
        if (clash) return heldoutRefused(res, "the directory must be outside every registered project");
        if (full === HOME + "/.pulse" || full === HOME + "/.pulse/fleet" || full.startsWith(HOME + "/.pulse/fleet/")) return heldoutRefused(res, "the directory must not overlap the fleet area");
        // Templates are written once per folder and never overwritten; a folder seen before keeps its counts.
        const set = (h.sets[full] ||= { count: 0, pending: 2, fresh: true });
        const created = set.fresh ? 4 : 0;
        set.fresh = false;
        Object.assign(h, { configured: true, present: true, count: set.count, pending: set.pending, generated: set.generated || 0, problem: "", dir: full });
        // The store's `setting` event, masked the way the real hub masks a private setting (bus.PrivateSettings).
        emit("human", "setting", "evolve.heldout_dir", "[owner-private]");
        heldoutGates();
        return send(res, 200, { configured: true, present: true, count: h.count, pending: h.pending, created, problem: "" });
      }
      if (!h.configured) return heldoutRefused(res, "no owner-private directory is configured");
      if (!h.present) return heldoutRefused(res, "the owner-private directory must exist with mode 0700");
      const request = typeof body.request === "string" ? body.request : "";
      const judge = typeof body.judge === "string" ? body.judge : "";
      const scorer = typeof body.scorer === "string" ? body.scorer : "";
      if (!request.trim()) return fail(res, 400, "Invalid request", "request must be a non-empty string.", "Write what the agent is asked.");
      if ((judge === "") === (scorer === "")) return heldoutRefused(res, "name exactly one of judge or scorer");
      if (judge && !JUDGES.some((j) => j.name === judge)) return heldoutRefused(res, "unknown judge " + judge);
      if (!judge && !scorer.trim()) return heldoutRefused(res, "the scorer must not be empty");
      if (/PULSE_HELDOUT_TEMPLATE/.test(request) || /PULSE_HELDOUT_TEMPLATE/.test(scorer)) return heldoutRefused(res, "the template marker is still in the text");
      if (secretShaped(request.trim()) || (scorer && secretShaped(scorer.trim()))) return heldoutRefused(res, "a fixture must not contain credentials");
      if ("slot" in body && (!Number.isInteger(body.slot) || body.slot < 0 || body.slot > 1000)) return fail(res, 400, "Invalid slot", "slot must be a whole number from 1 to 1000, or 0 for the next pair.", "Send the fixture's position in name order, or omit slot.");
      const slot = Number(body.slot || 0);
      if (h.pending > 0) [h.pending, h.count] = [h.pending - 1, h.count + 1];
      else if (slot > 0 && slot > h.count) return heldoutRefused(res, "slot " + slot + " is out of range (1-" + h.count + ")");
      else if (slot === 0) h.count += 1;
      else if (h.generated > 0) h.generated -= 1; // rewriting a generated pair makes it the owner's (ADR-033 ledger)
      Object.assign(h.sets[h.dir], { count: h.count, pending: h.pending, generated: h.generated });
      heldoutGates();
      return send(res, 200, { configured: true, present: true, count: h.count, pending: h.pending, created: 0, problem: "" });
    }
    return fail(res, 405, "no such write", req.method + " " + url.pathname + " is not an owner action", "the writes are tasks, tasks/<id>/amend|cancel, messages, decide, settings, compute/<id>/approve|deny, projects, projects/<slug>/remove|restore, compute/executors/<name>/remove, records/<id>/accept|status, heldout, heldout/fixtures");
  }

  // GET - live state first, fixtures second
  if (segs[0] === "roster" && segs.length === 1) {
    if (!agentReturned && !agentReturnStarted) {
      agentReturnStarted = true;
      setTimeout(() => {
        agentReturned = true;
        emit("delta", "agent.resume", "delta", { session: "run-delta-current" });
      }, agentReturnMS);
    }
    const task002 = state.tasks.find((t) => t.id === "TASK-002")?._detail;
    return send(res, 200, rosterScenarios(await loadJson("roster.json"), now(), agentReturned, taskWorkFocus(task002, now())));
  }
  if (segs[0] === "heldout" && segs.length === 1) return send(res, 200, heldoutView(state.heldout));
  if (segs[0] === "projects" && segs.length === 3 && segs[2] === "evolution") {
    const d = await fixture("projects/" + segs[1] + "/evolution");
    if (d === null) return fail(res, 404, "not found", "no fixture for " + url.pathname, "add mock/api/" + segs.join("/") + ".json");
    return send(res, 200, evolutionView(segs[1], d));
  }
  if (segs[0] === "tasks" && segs.length === 1) {
    const p = q.get("project");
    const st = q.get("status");
    const details = Object.fromEntries(state.tasks.map((t) => [t.id, t._detail]));
    const tasks = taskScenarios({ tasks: state.tasks.filter((t) => (!p || t.project === p) && (!st || t.status === st)) }, now(), details).tasks;
    return send(res, 200, { tasks: tasks.map(strip) });
  }
  if (segs[0] === "tasks" && segs.length === 2) {
    const t = state.tasks.find((x) => x.id === segs[1]);
    if (!t) return fail(res, 404, "no such task", segs[1] + " is not in the store", "open it from the Tasks page");
    const detail = taskDetailScenario(t._detail, now());
    return send(res, 200, { ...detail, task: strip(detail.task), messages: state.messages.filter((m) => m.task_id === t.id) });
  }
  if (segs[0] === "messages") {
    const p = q.get("project");
    const t = q.get("task");
    const since = q.get("since");
    return send(res, 200, { messages: state.messages.filter((m) => (!p || m.project === p) && (!t || m.task_id === t) && (!since || m.at > since)) });
  }
  if (segs[0] === "compute" && segs.length === 1) return send(res, 200, state.compute);
  if (segs[0] === "projects" && segs.length === 1) return send(res, 200, { projects: state.projects, brain: state.projects.find((p) => p.brain)?.slug || "" });
  if (segs[0] === "folders" && segs.length === 1) {
    const f = folders(q.get("path") || "");
    if (f.error) return fail(res, 404, f.error, "nothing exists at that path in the mock tree", "go up one level");
    return send(res, 200, f);
  }
  if (segs[0] === "settings" && segs.length === 1) return send(res, 200, { ...state.settings, settings: state.settings.settings.map(maskSetting) });
  if (segs[0] === "proposals" && segs.length === 1) return send(res, 200, state.proposals);
  if (segs[0] === "org") {
    const org = await fixture("org");
    // The real hub lists active projects only; mirror removals made in memory.
    // The self-improvement gates (ADR-029) and record refs (ADR-030) travel on GET /projects in the real hub, so the org rows drop them here
    // even though org.json seeds them: the console must merge the registry row like it does in production.
    const GATE_FIELDS = ["fixtures_enabled", "baseline", "evolve_level", "evolve_next", "constitution", "evaluators", "baseline_job", "baseline_failed_at"];
    const projects = org.projects
      .filter((p) => p.kind === "commons" || state.projects.some((x) => x.slug === p.slug && x.status === "active"))
      .map((p) => Object.fromEntries(Object.entries(p).filter(([k]) => !GATE_FIELDS.includes(k))));
    return send(res, 200, { ...org, projects, proposals: state.proposals, generated_at: now() });
  }
  if (segs[0] === "events" && segs.length === 1) {
    const p = q.get("project");
    const since = Number(q.get("since") || 0);
    const limit = Number(q.get("limit") || 50);
    // Like the hub: the newest `limit` events after `since`, oldest first.
    const events = state.events
      .filter((e) => e.id > since && (!p || (e.detail && e.detail.project === p)))
      .sort((a, b) => b.id - a.id)
      .slice(0, limit);
    return send(res, 200, { events: events.reverse() });
  }
  if (segs[0] === "records" && segs.length === 1) {
    const all = (await fixture("records")).records.map(recordView);
    const p = q.get("project");
    const k = q.get("kind");
    const st = q.get("status");
    return send(res, 200, { records: all.filter((r) => (!p || r.project === p) && (!k || r.kind === k) && (!st || r.status === st)) });
  }
  if (segs[0] === "records" && segs.length === 2) {
    const d = await fixture("records/" + segs[1]);
    if (!d) return fail(res, 404, "no such record", segs[1] + " is not indexed", "run pulse init in the project checkout");
    const o = state.records[segs[1]];
    const view = o ? { ...d, record: recordView(d.record), frontmatter: { ...d.frontmatter, status: o.status }, revisions: [...(d.revisions || []), ...o.revisions] } : d;
    const rev = q.get("rev");
    if (rev && Number(rev) !== view.record.current_rev) return send(res, 200, { ...view, body: `_(revision r${rev} of ${view.record.id} - the mock keeps only the current body)_\n\n` + view.body });
    return send(res, 200, view);
  }
  if (segs[0] === "constitution") {
    const d = await fixture("constitution");
    const p = q.get("project");
    if (p && p !== d?.record?.project) return send(res, 200, { record: null, body: "", amendments: [] });
    return send(res, 200, d);
  }
  const d = await fixture(segs.map(encodeURIComponent).join("/"));
  if (d === null) return fail(res, 404, "not found", "no fixture for " + url.pathname, "add mock/api/" + segs.join("/") + ".json");
  return send(res, 200, d);
}

const strip = (t) => Object.fromEntries(Object.entries(t).filter(([k]) => !k.startsWith("_")));

await init();
createServer((req, res) => {
  handle(req, res).catch((e) => {
    console.error(e);
    if (!res.headersSent) fail(res, 500, "mock server error", String(e.message), "see the terminal running dev:mock");
  });
}).listen(port, "127.0.0.1", () => console.log(`pulse mock hub on http://127.0.0.1:${port}/api/v1/  (fixtures: ${root})`));
