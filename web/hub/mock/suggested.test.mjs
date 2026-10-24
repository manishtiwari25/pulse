import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { createSuggestedMock, SUGGESTED_STATUSES } from "./suggested.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const fixture = JSON.parse(await readFile(join(here, "api", "suggested.json"), "utf8"));

// The mock answers the shapes crates/pulse-hub/src/suggestions_api.rs
// serializes and the owner console (web/hub/src/lib/api.ts) reads:
// list -> { project, suggestions, statuses }, a single suggestion and every
// verdict -> { suggestion, task }, suggest -> { suggestion }.
const LIST_KEYS = ["project", "statuses", "suggestions"];
const WRITE_KEYS = ["suggestion", "task"];

const NOW = "2026-09-08T09:00:00Z";
// timers 0 keep the daemon's review synchronous; a test still awaits a tick so
// the timer callbacks have run before it asserts.
const tick = () => new Promise((r) => setTimeout(r, 5));
const setup = () => {
  const suggestions = structuredClone(fixture.suggestions);
  const tasks = [];
  const replies = [];
  const events = [];
  const mock = createSuggestedMock({
    suggestions,
    tasks,
    now: () => NOW,
    timers: { reviewing: 0, accepted: 0 },
    leadReplies: (project, taskId, title) => replies.push({ project, taskId, title }),
    emit: (actor, action, subject, detail) => events.push({ actor, action, subject, detail }),
  });
  return { mock, suggestions, tasks, replies, events };
};
const keys = (o) => Object.keys(o).sort();

test("list: answers project, suggestions and statuses, newest first", () => {
  const { mock } = setup();
  const r = mock.list(new URLSearchParams());
  assert.equal(r.status, 200);
  assert.deepEqual(keys(r.body), LIST_KEYS);
  assert.equal(r.body.project, "");
  assert.deepEqual(r.body.statuses, SUGGESTED_STATUSES);
  assert.deepEqual(r.body.suggestions.map((s) => s.id), ["SUG-004", "SUG-003", "SUG-002", "SUG-001"]);
});

test("list: ?project= filters to one project and echoes it", () => {
  const { mock } = setup();
  const r = mock.list(new URLSearchParams("project=demo-app"));
  assert.equal(r.status, 200);
  assert.equal(r.body.project, "demo-app");
  assert.equal(r.body.suggestions.length, fixture.suggestions.length);
  assert.ok(r.body.suggestions.every((s) => s.project === "demo-app"));
  const empty = mock.list(new URLSearchParams("project=other-app"));
  assert.equal(empty.body.project, "other-app");
  assert.deepEqual(empty.body.suggestions, []);
});

test("list: ?status=open keeps only suggested, reviewing and owner rows", () => {
  const { mock } = setup();
  const r = mock.list(new URLSearchParams("project=demo-app&status=open"));
  assert.equal(r.status, 200);
  assert.deepEqual(keys(r.body), LIST_KEYS);
  assert.deepEqual(r.body.suggestions.map((s) => s.id), ["SUG-004", "SUG-003", "SUG-002"]);
  assert.ok(r.body.suggestions.every((s) => ["suggested", "reviewing", "owner"].includes(s.status)));
});

test("list: ?status=<closed> keeps exactly that status", () => {
  const { mock } = setup();
  const r = mock.list(new URLSearchParams("status=declined"));
  assert.deepEqual(r.body.suggestions.map((s) => s.id), ["SUG-001"]);
});

test("show: answers the suggestion and its accepted task", () => {
  const { mock, suggestions, tasks } = setup();
  const accepted = { id: "TASK-777", project: "demo-app", title: "Accepted", _detail: { task: {} } };
  tasks.push(accepted);
  suggestions.find((s) => s.id === "SUG-002").task_id = "TASK-777";
  const r = mock.show("SUG-002");
  assert.equal(r.status, 200);
  assert.deepEqual(keys(r.body), WRITE_KEYS);
  assert.equal(r.body.suggestion.id, "SUG-002");
  assert.equal(r.body.task.id, "TASK-777");
  assert.equal(r.body.task._detail, undefined);
});

test("show: an open suggestion answers task: null", () => {
  const { mock } = setup();
  const r = mock.show("SUG-002");
  assert.equal(r.status, 200);
  assert.equal(r.body.task, null);
});

test("show: an unknown id answers 404 No suggestion", () => {
  const { mock } = setup();
  const r = mock.show("SUG-999");
  assert.equal(r.status, 404);
  assert.equal(r.body.error, 'No suggestion "SUG-999"');
});

test("suggest: creates the suggestion the daemon then reviews and accepts", async () => {
  const { mock, tasks, events } = setup();
  const r = mock.suggest({ project: "demo-app", title: "Add a health endpoint", brief: "The load balancer needs /healthz.", priority: 4 });
  assert.equal(r.status, 201);
  assert.deepEqual(keys(r.body), ["suggestion"]);
  assert.equal(r.body.suggestion.id, "SUG-005");
  assert.equal(r.body.suggestion.status, "suggested");
  assert.equal(r.body.suggestion.suggested_by, "human");
  assert.equal(r.body.suggestion.priority, 4);
  assert.equal(r.body.suggestion.created_at, NOW);
  await tick();
  const g = r.body.suggestion;
  assert.equal(g.status, "accepted");
  assert.equal(g.reviewer, "basalt");
  assert.equal(g.reviewed_at, NOW);
  assert.equal(tasks.length, 1);
  assert.equal(g.task_id, tasks[0].id);
  assert.equal(tasks[0].title, "Add a health endpoint");
  assert.equal(tasks[0].status, "open");
  assert.ok(tasks[0].brief.startsWith("The load balancer needs /healthz."));
  assert.deepEqual(events.map((e) => e.action), ["suggestion.new", "suggestion.reviewing", "suggestion.review"]);
});

test("suggest: an open suggestion with the same title is a 400", () => {
  const { mock } = setup();
  const r = mock.suggest({ project: "demo-app", title: "add a smoke test for the checkout page" });
  assert.equal(r.status, 400);
  assert.equal(r.body.error, "Already suggested as SUG-002");
});

test("suggest: a closed suggestion with the same title does not block", () => {
  const { mock } = setup();
  const r = mock.suggest({ project: "demo-app", title: "Rename the runner's ready state to idle" });
  assert.equal(r.status, 201);
});

test("suggest: without a project or a title it is a 400", () => {
  const { mock } = setup();
  assert.equal(mock.suggest({ title: "x" }).status, 400);
  assert.equal(mock.suggest({ project: "demo-app", title: "  " }).status, 400);
  assert.equal(mock.suggest({ project: "demo-app", title: "  " }).body.error, "Invalid title");
});

test("accept: creates the task, keeps the title override, links both rows", () => {
  const { mock, tasks, replies } = setup();
  const r = mock.act("SUG-002", "accept", { title: "Smoke-test the checkout page", reason: "Worth a run." });
  assert.equal(r.status, 200);
  assert.deepEqual(keys(r.body), WRITE_KEYS);
  assert.equal(r.body.suggestion.status, "accepted");
  assert.equal(r.body.suggestion.reviewer, "human");
  assert.equal(r.body.suggestion.verdict, "Worth a run.");
  assert.equal(r.body.suggestion.reviewed_at, NOW);
  assert.equal(tasks.length, 1);
  assert.equal(r.body.suggestion.task_id, tasks[0].id);
  assert.equal(r.body.task.id, tasks[0].id);
  assert.equal(r.body.task.title, "Smoke-test the checkout page");
  assert.equal(r.body.task.status, "open");
  assert.equal(r.body.task.created_by, "human");
  assert.equal(r.body.task._detail, undefined);
  assert.ok(tasks[0]._detail.task.brief.includes("Suggested by human as SUG-002; accepted by human: Worth a run."));
  assert.deepEqual(replies, [{ project: "demo-app", taskId: tasks[0].id, title: "Smoke-test the checkout page" }]);
});

test("accept: without an override the suggestion title becomes the task title", () => {
  const { mock, tasks } = setup();
  const r = mock.act("SUG-002", "accept", {});
  assert.equal(r.status, 200);
  assert.equal(r.body.task.title, "Add a smoke test for the checkout page");
  assert.equal(tasks[0].title, "Add a smoke test for the checkout page");
});

test("accept: a reviewed suggestion does not change again", () => {
  const { mock } = setup();
  const r = mock.act("SUG-001", "accept", {});
  assert.equal(r.status, 400);
  assert.equal(r.body.error, "SUG-001 is already declined");
});

test("accept: an unknown id answers 404 No suggestion", () => {
  const { mock } = setup();
  const r = mock.act("SUG-999", "accept", {});
  assert.equal(r.status, 404);
  assert.equal(r.body.error, 'No suggestion "SUG-999"');
});

test("decline: records the verdict and no task", () => {
  const { mock, tasks } = setup();
  const r = mock.act("SUG-002", "decline", { reason: "Covered by TASK-009." });
  assert.equal(r.status, 200);
  assert.deepEqual(keys(r.body), WRITE_KEYS);
  assert.equal(r.body.suggestion.status, "declined");
  assert.equal(r.body.suggestion.reviewer, "human");
  assert.equal(r.body.suggestion.verdict, "Covered by TASK-009.");
  assert.equal(r.body.suggestion.reviewed_at, NOW);
  assert.equal(r.body.task, null);
  assert.equal(tasks.length, 0);
});

test("decline: a reviewed suggestion does not change again", () => {
  const { mock } = setup();
  const r = mock.act("SUG-001", "decline", { reason: "x" });
  assert.equal(r.status, 400);
  assert.equal(r.body.error, "SUG-001 is already declined");
});

test("withdraw: the owner retires their own suggestion", () => {
  const { mock } = setup();
  const r = mock.act("SUG-002", "withdraw", { reason: "Stale after the feed rewrite." });
  assert.equal(r.status, 200);
  assert.deepEqual(keys(r.body), WRITE_KEYS);
  assert.equal(r.body.suggestion.status, "withdrawn");
  assert.equal(r.body.suggestion.verdict, "Stale after the feed rewrite.");
  assert.equal(r.body.task, null);
});

test("withdraw: an unknown id answers 404 No suggestion", () => {
  const { mock } = setup();
  const r = mock.act("SUG-999", "withdraw", { reason: "x" });
  assert.equal(r.status, 404);
  assert.equal(r.body.error, 'No suggestion "SUG-999"');
});
