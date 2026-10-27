import assert from "node:assert/strict";
import { test } from "node:test";
import { chapterLineCount, durationText, eventTouchesTask, exitBadge, groupActivity, lastAt, runOutcome, runSummaryLine, systemLine, whoWord, workedFor } from "./task-activity.ts";

const run = (over = {}) => ({
  id: "RUN-1",
  job_id: "JOB-1",
  job_title: "Do the work",
  agent: "cinder",
  executor: "local",
  status: "done",
  exit_code: 0,
  started_at: "2026-10-27T10:00:00Z",
  finished_at: "2026-10-27T10:04:10Z",
  summary: "did it",
  session_id: "HS-1",
  calls: 5,
  commands: 3,
  edits: 1,
  steps: 20,
  ...over,
});

const item = (id, over = {}) => ({ id, at: "2026-10-27T10:00:00Z", kind: "step", what: "assistant", who: "cinder", ...over });

test("lines of one run gather into a chapter; a message between them splits it", () => {
  const items = [
    item("task:created", { kind: "task", what: "created", who: "human" }),
    item("run:start:RUN-1", { kind: "run", what: "started", run_id: "RUN-1" }),
    item("step:HS-1:2", { run_id: "RUN-1", text: "hello" }),
    item("msg:MSG-1", { kind: "message", what: "sent", who: "human", at: "2026-10-27T10:01:00Z" }),
    item("step:HS-1:8", { run_id: "RUN-1", what: "command", at: "2026-10-27T10:02:00Z" }),
    item("run:end:RUN-1", { kind: "run", what: "ended", run_id: "RUN-1", at: "2026-10-27T10:04:10Z" }),
    item("step:HS-9:1", { run_id: "RUN-unknown", at: "2026-10-27T10:05:00Z" }),
  ];
  const blocks = groupActivity({ items, runs: [run()] });
  assert.deepEqual(
    blocks.map((b) => (b.kind === "run" ? ["run", b.run.id, b.items.map((i) => i.id), b.first] : ["line", b.item.id])),
    [
      ["line", "task:created"],
      ["run", "RUN-1", ["run:start:RUN-1", "step:HS-1:2"], true],
      ["line", "msg:MSG-1"],
      ["run", "RUN-1", ["step:HS-1:8", "run:end:RUN-1"], false],
      // A run the list does not know stands as a plain line.
      ["line", "step:HS-9:1"],
    ],
  );
  assert.equal(chapterLineCount(blocks[3].items), 1);
  assert.equal(lastAt(blocks[3].items), "2026-10-27T10:04:10Z");
});

test("the summary line says who worked how long on what, and how it ended", () => {
  const now = Date.parse("2026-10-27T10:10:00Z");
  assert.equal(runSummaryLine(run(), now), "cinder worked 4 minutes: 3 commands, 1 file changed \u2014 finished.");
  assert.equal(runSummaryLine(run({ status: "running", finished_at: "", commands: 0, edits: 0, calls: 2 }), now), "cinder has been on it 10 minutes: 2 model calls.");
  assert.equal(runSummaryLine(run({ status: "failed", commands: 0, edits: 0, calls: 0, finished_at: "2026-10-27T10:00:20Z" }), now), "cinder worked 20 seconds \u2014 stopped with an error.");
  assert.equal(runSummaryLine(run({ status: "held", edits: 2, commands: 1 }), now), "cinder worked 4 minutes: 1 command, 2 files changed \u2014 paused for the runner's restart.");
  assert.equal(workedFor({ started_at: "2026-10-27T08:00:00Z", finished_at: "2026-10-27T09:30:00Z" }, now), "1.5 hours");
  assert.equal(workedFor({ started_at: "bad", finished_at: "" }, now), "");
});

test("outcome words cover every run status the store writes", () => {
  for (const s of ["done", "running", "held", "failed", "error", "timeout", "lost", "stale", "killed", "cancelled", "dry-run"]) assert.ok(runOutcome(s).text, s);
  assert.equal(runOutcome("done").tone, "ok");
  assert.equal(runOutcome("running").tone, "live");
  assert.equal(runOutcome("something_else").text, "something else");
});

test("badges and durations stay short", () => {
  assert.equal(exitBadge(null), null);
  assert.equal(exitBadge(undefined), null);
  assert.deepEqual(exitBadge(0), { text: "ok", bad: false });
  assert.deepEqual(exitBadge(101), { text: "exit 101", bad: true });
  assert.equal(durationText(0), "");
  assert.equal(durationText(800), "");
  assert.equal(durationText(4200), "4.2s");
  assert.equal(durationText(12000), "12s");
  assert.equal(durationText(150_000), "3 min");
});

test("system lines read as sentences", () => {
  assert.equal(whoWord("human"), "You");
  assert.equal(whoWord("owner"), "You");
  assert.equal(whoWord(""), "PULSE");
  assert.equal(systemLine(item("task:3", { kind: "task", what: "status", who: "demo-lead", title: "in_progress", status: "review" })), "demo-lead moved it from in progress to review.");
  assert.equal(systemLine(item("task:4", { kind: "task", what: "lead", who: "runner", title: "demo-lead" })), "demo-lead became the lead agent for this task.");
  assert.equal(systemLine(item("rev:2", { kind: "task", what: "revised", who: "human", title: "New title" })), "You changed the task: New title.");
  assert.equal(systemLine(item("task:5", { kind: "task", what: "cancelled", who: "human", text: "not needed" })), "You cancelled the task: not needed.");
  assert.equal(systemLine(item("task:6", { kind: "task", what: "followup", who: "daemon", title: "failed" })), "PULSE queued a follow-up because the step failed.");
  assert.equal(systemLine(item("job:add:J", { kind: "job", what: "added", who: "demo-lead", title: "Write the tests" })), "demo-lead added a step: Write the tests.");
  assert.equal(systemLine(item("job:add:J2", { kind: "job", what: "added", who: "daemon", title: "Plan" })), "PULSE added a step: Plan.");
  assert.equal(systemLine(item("land:1", { kind: "land", what: "landed", who: "daemon", status: "bd8231184f433706bf9" })), "PULSE landed the work on the main branch (bd8231184f43).");
  assert.equal(systemLine(item("land:2", { kind: "land", what: "held", who: "daemon", text: "verify failed" })), "PULSE held the landing: verify failed.");
  assert.equal(systemLine(item("land:3", { kind: "land", what: "pushed", who: "daemon" })), "PULSE pushed the main branch to the remote.");
  assert.equal(systemLine(item("run:start:R", { kind: "run", what: "started", who: "cinder", title: "Do it" })), "cinder started on \u201cDo it\u201d.");
  assert.equal(systemLine(item("run:end:R", { kind: "run", what: "ended", who: "cinder", status: "timeout" })), "cinder ran out of time.");
});

test("only harness events of the task's own jobs or sessions move the feed", () => {
  const jobs = new Set(["JOB-1"]);
  const sessions = new Set(["HS-1"]);
  assert.ok(eventTouchesTask({ action: "harness.step", subject: "HS-9", detail: { job: "JOB-1", n: 3 } }, jobs, sessions));
  assert.ok(eventTouchesTask({ action: "harness.finish", subject: "HS-1", detail: null }, jobs, sessions));
  assert.ok(!eventTouchesTask({ action: "harness.step", subject: "HS-9", detail: { job: "JOB-2" } }, jobs, sessions));
  assert.ok(!eventTouchesTask({ action: "harness.step", subject: "HS-9", detail: "text" }, jobs, sessions));
  // Job, run, task and message events already refresh tasks/* through the hub's refresh rules.
  assert.ok(!eventTouchesTask({ action: "job.done", subject: "JOB-1", detail: {} }, jobs, sessions));
});
