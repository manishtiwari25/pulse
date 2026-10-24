// Suggested-work mock for `tasks/suggested` (crates/pulse-hub/src/suggestions_api.rs):
// the owner or an agent suggests the next work item, a daemon agent reviews it,
// and an accepted suggestion becomes a task. Pure functions over in-memory
// state, so the shape is testable without a server: createSuggestedMock({...})
// mirrors mock/api/suggested.json plus the deps server.mjs injects.
//
// The list body is { project, suggestions, statuses }; a single suggestion and
// every verdict body is { suggestion, task } (task null unless accepted); the
// suggest body is { suggestion }. Those keys are the contract the owner console
// reads - freeze them here.
const pad = (n, w) => String(n).padStart(w, "0");
const nextId = (prefix, rows) => prefix + "-" + pad(rows.reduce((m, r) => Math.max(m, Number(String(r.id).split("-").pop()) || 0), 0) + 1, prefix === "MSG" ? 4 : 3);
const strip = (t) => Object.fromEntries(Object.entries(t).filter(([k]) => !k.startsWith("_")));

export const SUGGESTED_STATUSES = ["suggested", "reviewing", "accepted", "declined", "owner", "withdrawn"];
const OPEN = ["suggested", "reviewing", "owner"];

// deps: { now, leadReplies, tasks, onTask, emit, timers } - now() returns the
// clock; leadReplies(project, taskId, title) plants the lead's messages; tasks
// is the shared task rows array; onTask(t) mirrors server.mjs's push of a new
// task; emit feeds the activity log; timers is the two review delays (0 in
// tests so the run stays synchronous).
export function createSuggestedMock({ suggestions = [], tasks = [], now = () => new Date().toISOString(), leadReplies = () => {}, onTask = (t) => tasks.push(t), emit = () => {}, timers = { reviewing: 5000, accepted: 14000 } } = {}) {
  const failure = (status, error, why, fix) => ({ status, body: { error, why, fix } });
  const success = (body) => ({ status: 200, body });
  const find = (id) => suggestions.find((x) => x.id === id);

  function list(query) {
    const p = query.get("project") || "";
    const st = query.get("status") || "";
    const rows = suggestions.filter((g) => (!p || g.project === p) && (!st || (st === "open" ? OPEN.includes(g.status) : g.status === st))).sort((a, b) => (a.id < b.id ? 1 : -1));
    return success({ project: p, suggestions: rows, statuses: SUGGESTED_STATUSES });
  }

  function show(id) {
    const g = find(id);
    if (!g) return failure(404, "No suggestion " + JSON.stringify(id), "the id is not in this store", "pulse task suggestions");
    const t = g.task_id ? tasks.find((x) => x.id === g.task_id) : null;
    return success({ suggestion: g, task: t ? strip(t) : null });
  }

  // A new suggestion from the owner. The daemon queues a review a little
  // later and the reviewing agent accepts it; both steps are timers so the
  // console shows the suggestion moving through the states.
  function suggest(body) {
    if (!body.project || !body.title?.trim()) return failure(400, "Invalid title", "title must be a non-empty string.", "Send a JSON object with a valid title.");
    const title = body.title.trim();
    const dup = suggestions.find((x) => x.project === body.project && x.title.toLowerCase() === title.toLowerCase() && OPEN.includes(x.status));
    if (dup) return failure(400, "Already suggested as " + dup.id, "an open suggestion in this project has the same title", "add your evidence on its thread or suggest a different slice");
    const g = { id: nextId("SUG", suggestions), project: body.project, title, brief: body.brief || "", priority: Number(body.priority ?? 10), suggested_by: "human", status: "suggested", reviewer: "", verdict: "", job_id: "", reviews: 0, task_id: "", created_at: now(), updated_at: now(), reviewed_at: "" };
    suggestions.push(g);
    emit("human", "suggestion.new", g.id, { project: g.project, title: g.title, priority: g.priority });
    setTimeout(() => {
      if (g.status !== "suggested") return;
      g.status = "reviewing";
      g.job_id = "job-" + Date.now();
      g.job_status = "running";
      g.job_agent = "basalt";
      g.updated_at = now();
      emit("daemon", "suggestion.reviewing", g.id, { project: g.project, job: g.job_id, review: 1 });
    }, timers.reviewing);
    setTimeout(() => {
      if (g.status !== "reviewing") return;
      const t = { id: nextId("TASK", tasks), project: g.project, title: g.title, status: "open", priority: g.priority, lead: "", job_id: "", created_by: "human", created_at: now(), updated_at: now(), current_rev: 1, brief: g.brief + "\n\n---\nSuggested by human as " + g.id + "; accepted by basalt: the check it names fails today.\n",
        jobs_total: 0, jobs_queued: 0, jobs_blocked: 0, jobs_running: 0, jobs_done: 0, jobs_failed: 0, working: [] };
      t._detail = { task: t, revisions: [{ rev: 1, title: t.title, brief: t.brief, note: "", author: "human", at: t.created_at }], messages: [], jobs: [] };
      onTask(t);
      Object.assign(g, { status: "accepted", reviewer: "basalt", verdict: "the check it names fails today", task_id: t.id, reviewed_at: now(), updated_at: now(), job_status: "done" });
      emit("basalt", "suggestion.review", g.id, { project: g.project, verdict: "accept", task: t.id });
    }, timers.accepted);
    return { status: 201, body: { suggestion: g } };
  }

  // accept, decline or withdraw one suggestion. A reviewed suggestion does
  // not change again.
  function act(id, action, body) {
    const g = find(id);
    if (!g) return failure(404, "No suggestion " + JSON.stringify(id || ""), "the id is not in this store", "pulse task suggestions");
    if (!["accept", "decline", "withdraw"].includes(action)) return failure(404, "Unknown API route", "That owner-console route does not exist.", "Use an /api/v1 route.");
    if (!OPEN.includes(g.status)) return failure(400, g.id + " is already " + g.status, "a reviewed suggestion does not change", "pulse task show " + g.id);
    const reason = (body.reason || "").trim();
    g.updated_at = now();
    g.reviewed_at = now();
    if (action === "withdraw") {
      Object.assign(g, { status: "withdrawn", verdict: reason });
      emit("human", "suggestion.withdrawn", g.id, { project: g.project, reason });
      return success({ suggestion: g, task: null });
    }
    if (action === "decline") {
      Object.assign(g, { status: "declined", reviewer: "human", verdict: reason });
      emit("human", "suggestion.review", g.id, { project: g.project, verdict: "decline", reason });
      return success({ suggestion: g, task: null });
    }
    const title = (body.title || "").trim() || g.title;
    const t = { id: nextId("TASK", tasks), project: g.project, title, status: "open", priority: Number(body.priority ?? g.priority), lead: "", job_id: "", created_by: g.suggested_by, created_at: now(), updated_at: now(), current_rev: 1, brief: ((body.brief || "").trim() || g.brief) + "\n\n---\nSuggested by " + g.suggested_by + " as " + g.id + "; accepted by human" + (reason ? ": " + reason : "") + ".\n",
      jobs_total: 0, jobs_queued: 0, jobs_blocked: 0, jobs_running: 0, jobs_done: 0, jobs_failed: 0, working: [] };
    t._detail = { task: t, revisions: [{ rev: 1, title: t.title, brief: t.brief, note: "", author: g.suggested_by, at: t.created_at }], messages: [], jobs: [] };
    onTask(t);
    Object.assign(g, { status: "accepted", reviewer: "human", verdict: reason, task_id: t.id });
    emit("human", "suggestion.review", g.id, { project: g.project, verdict: "accept", task: t.id });
    leadReplies(t.project, t.id, t.title);
    return success({ suggestion: g, task: strip(t) });
  }

  return { list, show, suggest, act };
}
