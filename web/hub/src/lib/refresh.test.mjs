import assert from "node:assert/strict";
import { test } from "node:test";
import { Coalescer, isSilent, projectView, registry, starts, touchedBy, touches } from "./refresh.ts";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// The paths a typical set of open tabs subscribes to.
const PATHS = [
  "org", "projects", "timeline", "notices", "runner", "running", "service", "settings", "cred", "heldout", "roster", "votes", "compute",
  "tasks", "tasks?project=demo", "tasks/TASK-001", "messages?project=demo&task=TASK-001", "code-deliveries?limit=20", "jobs/failed?project=demo&limit=20",
  "records", "records?kind=adr&status=Proposed", "records/ADR-001?project=demo", "constitution?project=demo", "hygiene?limit=30", "events?limit=40",
  "harness", "harness/prompt", "harness/models", "harness/sessions?limit=50", "harness/sessions/abc", "mcp", "mcp?project=demo", "mcp/MCP-001", "mcp/options", "skills", "skills/SKILL-001",
  "projects/demo/overview", "projects/demo/project", "projects/demo/timeline", "projects/demo/threads", "projects/demo/threads/k1", "projects/demo/posts/p1",
  "projects/demo/proposals", "projects/demo/questions", "projects/demo/suggestions", "projects/demo/amendments", "projects/demo/evolution", "projects/demo/notices", "projects/demo/goal",
];
const hit = (match) => PATHS.filter(match);

test("path matchers tell a prefix from a sibling and a project view from the registry", () => {
  const tasks = starts("tasks");
  assert.equal(tasks("tasks"), true);
  assert.equal(tasks("tasks?project=demo"), true);
  assert.equal(tasks("tasks/TASK-001"), true);
  assert.equal(tasks("taskstats"), false);
  assert.equal(registry("projects"), true);
  assert.equal(registry("projects/demo/overview"), false);
  assert.equal(projectView()("projects/demo/overview"), true);
  assert.equal(projectView("overview")("projects/demo/timeline"), false);
  assert.equal(projectView("overview")("projects"), false);
});

test("harness.step is silent: it is neither queued nor does it touch any view", () => {
  assert.equal(isSilent("harness.step"), true);
  assert.deepEqual(hit(touches("harness.step")), []);
  assert.equal(isSilent("harness.finish"), false);
  assert.deepEqual(hit(touches("harness.finish")), ["roster", "harness/sessions?limit=50", "harness/sessions/abc"]);
});

test("each action refreshes the views it can change and nothing else", () => {
  const job = hit(touches("job.done"));
  // messages too: the task chat's "reads it next / reading / seen" line follows the lead's job.
  for (const p of ["runner", "roster", "org", "projects", "tasks", "tasks/TASK-001", "jobs/failed?project=demo&limit=20", "code-deliveries?limit=20", "projects/demo/overview", "messages?project=demo&task=TASK-001"]) assert.ok(job.includes(p), p);
  for (const p of ["settings", "harness/models", "events?limit=40", "projects/demo/threads", "records"]) assert.ok(!job.includes(p), p);

  const post = hit(touches("post.question"));
  for (const p of ["org", "timeline", "votes", "roster", "projects/demo/questions", "projects/demo/overview", "projects/demo/threads/k1"]) assert.ok(post.includes(p), p);
  for (const p of ["tasks", "runner", "settings", "harness", "records", "compute", "events?limit=40"]) assert.ok(!post.includes(p), p);

  const msg = hit(touches("message.sent"));
  assert.deepEqual(msg, ["tasks", "tasks?project=demo", "tasks/TASK-001", "messages?project=demo&task=TASK-001"]);

  const setting = hit(touches("setting"));
  for (const p of ["settings", "org", "runner", "harness", "heldout", "compute"]) assert.ok(setting.includes(p), p);
  for (const p of ["tasks", "records", "projects/demo/overview", "cred"]) assert.ok(!setting.includes(p), p);

  assert.deepEqual(hit(touches("mcp.checked")), ["mcp", "mcp?project=demo", "mcp/MCP-001", "mcp/options"]);
  assert.deepEqual(hit(touches("skill.added")), ["skills", "skills/SKILL-001"]);
  for (const p of ["mcp", "skills"]) assert.ok(!setting.includes(p), p);

  const land = hit(touches("land.landed"));
  for (const p of ["org", "projects", "code-deliveries?limit=20", "runner", "tasks?project=demo", "projects/demo/overview", "projects/demo/timeline"]) assert.ok(land.includes(p), p);
  for (const p of ["settings", "projects/demo/questions", "harness", "records"]) assert.ok(!land.includes(p), p);
});

test("an unknown action costs at most the summary views", () => {
  assert.deepEqual(hit(touches("something.brand_new")), ["org", "timeline", "notices"]);
  assert.deepEqual(hit(touches("mystery")), ["org", "timeline", "notices"]);
});

test("a batch of actions refreshes the union of what they touch, once", () => {
  const both = hit(touchedBy(["job.done", "message.sent", "harness.step"]));
  const union = new Set([...hit(touches("job.done")), ...hit(touches("message.sent"))]);
  assert.deepEqual(new Set(both), union);
  assert.ok(both.includes("messages?project=demo&task=TASK-001"));
  assert.ok(both.includes("runner"));
  assert.ok(!both.includes("settings"));
});

test("a burst flushes once per ceiling and once more when it goes quiet", async () => {
  // A burst at one instant: with the injected clock frozen, every event
  // carries the same timestamp, so the quiet window and the ceiling are both
  // still open when the burst ends - however long the event loop actually
  // takes to run the adds. The old wall-clock version read a flush count off
  // the timing of its own sleeps, which is why it flaked one run in three
  // under load. The yields give every timer that a wake sets a wake-up.
  let now = 1_000_000;
  const flushes = [];
  const c = new Coalescer((actions, lastAt) => flushes.push({ actions: [...actions], lastAt }), { quietMs: 40, maxMs: 100 }, () => now);
  for (let i = 0; i < 16; i++) {
    for (let j = 0; j < 5; j++) c.add("harness.step");
    c.add("job.done");
    c.add("post.vote");
    await sleep(5);
    await sleep(0);
    await sleep(0);
    await sleep(0);
  }
  // No time passed on the clock the coalescer reads, so nothing was ever
  // due: if the timers used the wall clock, the ~80 ms of sleeps would have
  // fired the 40 ms quiet window several times over.
  assert.equal(flushes.length, 0, "the burst sits inside one quiet window and one ceiling");
  // Move past the last event's quiet window and yield until the trailing
  // flush fires (a wake reschedules by what is left, so give the
  // rescheduled timer a second round of wake-ups).
  now += 60;
  for (let i = 0; i < 8 && flushes.length < 1; i++) await sleep(25);
  for (let i = 0; i < 8 && flushes.length < 1; i++) await sleep(25);
  assert.equal(flushes.length, 1, "one flush for the whole burst, once it went quiet");
  assert.deepEqual(new Set(flushes[0].actions), new Set(["job.done", "post.vote"]), "the burst merged into one flush");
  assert.ok(!flushes[0].actions.includes("harness.step"), "steps are never queued");
  assert.equal(c.pending, 0);
  await sleep(120);
  assert.equal(flushes.length, 1, "nothing refreshes while nothing happens");
});

test("a paused coalescer queues and flushes once on resume, reporting when the last event came", async () => {
  const flushes = [];
  const c = new Coalescer((actions, lastAt) => flushes.push({ actions: [...actions], lastAt }), { quietMs: 30, maxMs: 100 });
  c.pause();
  c.add("job.done", 1000);
  c.add("post.vote", 2000);
  c.add("job.done", 3000);
  await sleep(150);
  assert.equal(flushes.length, 0, "a hidden tab refreshes nothing");
  assert.equal(c.pending, 2);
  c.resume();
  assert.equal(flushes.length, 0, "resume waits one quiet window");
  await sleep(80);
  assert.equal(flushes.length, 1);
  assert.deepEqual(flushes[0].actions.sort(), ["job.done", "post.vote"]);
  assert.equal(flushes[0].lastAt, 3000);
  c.resume();
  await sleep(60);
  assert.equal(flushes.length, 1, "resume with nothing pending flushes nothing");
});
