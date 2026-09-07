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

const root = join(dirname(fileURLToPath(import.meta.url)), "api");
const port = Number(process.env.PULSE_HUB_PORT ?? 8787);
const delay = Number(process.env.MOCK_DELAY_MS ?? 120);
// MOCK_STATIC=out serves the static export next to the API the way the Go
// hub does (dist.go embeds web/hub/out); used by `npm run serve:out`.
const staticDir = process.env.MOCK_STATIC ? normalize(join(dirname(fileURLToPath(import.meta.url)), "..", process.env.MOCK_STATIC)) : "";
const now = () => new Date().toISOString().replace(/\.\d{3}Z$/, "Z");

// ----- in-memory state for writes ----------------------------------------

const state = { tasks: null, messages: null, compute: null, settings: null, proposals: null, events: null, projects: null, seq: 5000 };
const loadJson = async (rel) => JSON.parse(await readFile(join(root, rel), "utf8"));
async function init() {
  state.tasks = (await loadJson("tasks.json")).tasks;
  state.messages = (await loadJson("messages.json")).messages;
  state.compute = await loadJson("compute.json");
  state.settings = await loadJson("settings.json");
  state.proposals = await loadJson("proposals.json");
  state.events = (await loadJson("events.json")).events;
  const org = await loadJson("org.json");
  state.projects = org.projects
    .filter((p) => p.kind !== "commons")
    .map((p) => ({ slug: p.slug, name: p.slug, path: p.path, remote: p.remote || "", brain: !!p.brain, status: "active", verify_cmd: "", default_branch: "main", added_at: "2026-09-01T09:00:00Z", added_by: "human", last_seen: "2026-09-07T09:00:00Z", present: p.present !== false }));
  state.projects.push({ slug: "old-site", name: "old-site", path: "/home/owner/src/old-site", remote: "", brain: false, status: "inactive", verify_cmd: "", default_branch: "main", added_at: "2026-08-20T09:00:00Z", added_by: "human", last_seen: "2026-09-03T09:00:00Z", present: true });
  for (const t of state.tasks) {
    try {
      t._detail = await loadJson(`tasks/${t.id}.json`);
    } catch {
      t._detail = { task: t, revisions: [{ rev: 1, title: t.title, brief: t.brief, note: "", author: t.created_by, at: t.created_at }], messages: [], jobs: [] };
    }
  }
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
      const t = { id: nextId("TASK", state.tasks), project: body.project, title: body.title.trim(), status: "open", priority: Number(body.priority ?? 10), lead: "", job_id: "", created_by: "human", created_at: now(), updated_at: now(), current_rev: 1, brief: body.brief || "" };
      t._detail = { task: t, revisions: [{ rev: 1, title: t.title, brief: t.brief, note: "", author: "human", at: t.created_at }], messages: [], jobs: [] };
      state.tasks.push(t);
      emit("human", "task.created", t.id, { project: t.project, title: t.title });
      setTimeout(() => {
        t.lead = t.project + "-lead";
        t.status = "planned";
        t.updated_at = now();
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
      if (s.kind === "identifier" && secretShaped(v)) return fail(res, 400, "looks like a secret", s.name + " is an identifier, not a credential", "auth is the hub machine's az login; set only ids, names and regions here");
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
        p = { slug, name: body.name || slug, path, remote: "", brain: false, status: "active", verify_cmd: body.verify || "", default_branch: "main", added_at: now(), added_by: "human", last_seen: now(), present: true };
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
    return fail(res, 405, "no such write", req.method + " " + url.pathname + " is not an owner action", "the writes are tasks, tasks/<id>/amend|cancel, messages, decide, settings, compute/<id>/approve|deny, projects, projects/<slug>/remove|restore, compute/executors/<name>/remove");
  }

  // GET - live state first, fixtures second
  if (segs[0] === "tasks" && segs.length === 1) {
    const p = q.get("project");
    const st = q.get("status");
    return send(res, 200, { tasks: state.tasks.filter((t) => (!p || t.project === p) && (!st || t.status === st)).map(strip) });
  }
  if (segs[0] === "tasks" && segs.length === 2) {
    const t = state.tasks.find((x) => x.id === segs[1]);
    if (!t) return fail(res, 404, "no such task", segs[1] + " is not in the store", "open it from the Tasks page");
    return send(res, 200, { ...t._detail, task: strip(t), messages: state.messages.filter((m) => m.task_id === t.id) });
  }
  if (segs[0] === "messages") {
    const p = q.get("project");
    const t = q.get("task");
    const since = q.get("since");
    return send(res, 200, { messages: state.messages.filter((m) => (!p || m.project === p) && (!t || m.task_id === t) && (!since || m.at > since)) });
  }
  if (segs[0] === "compute" && segs.length === 1) return send(res, 200, state.compute);
  if (segs[0] === "projects" && segs.length === 1) return send(res, 200, { projects: state.projects, brain: state.projects.find((p) => p.brain)?.slug || "" });
  if (segs[0] === "settings" && segs.length === 1) return send(res, 200, { ...state.settings, settings: state.settings.settings.map(maskSetting) });
  if (segs[0] === "proposals" && segs.length === 1) return send(res, 200, state.proposals);
  if (segs[0] === "org") {
    const org = await fixture("org");
    // The real hub lists active projects only; mirror removals made in memory.
    const projects = org.projects.filter((p) => p.kind === "commons" || state.projects.some((x) => x.slug === p.slug && x.status === "active"));
    return send(res, 200, { ...org, projects, proposals: state.proposals, generated_at: now() });
  }
  if (segs[0] === "events" && segs.length === 1) {
    const p = q.get("project");
    const since = Number(q.get("since") || 0);
    const limit = Number(q.get("limit") || 50);
    return send(res, 200, { events: state.events.filter((e) => e.id > since && (!p || (e.detail && e.detail.project === p))).slice(0, limit) });
  }
  if (segs[0] === "records" && segs.length === 1) {
    const all = (await fixture("records")).records;
    const p = q.get("project");
    const k = q.get("kind");
    const st = q.get("status");
    return send(res, 200, { records: all.filter((r) => (!p || r.project === p) && (!k || r.kind === k) && (!st || r.status === st)) });
  }
  if (segs[0] === "records" && segs.length === 2) {
    const d = await fixture("records/" + segs[1]);
    if (!d) return fail(res, 404, "no such record", segs[1] + " is not indexed", "run pulse init in the project checkout");
    const rev = q.get("rev");
    if (rev && Number(rev) !== d.record.current_rev) return send(res, 200, { ...d, body: `_(revision r${rev} of ${d.record.id} - the mock keeps only the current body)_\n\n` + d.body });
    return send(res, 200, d);
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
