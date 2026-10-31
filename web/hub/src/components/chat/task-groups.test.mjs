import assert from "node:assert/strict";
import { test } from "node:test";
import { sessionWords, taskFinished, taskMood, taskTitleOf } from "./task-groups.ts";

const session = (over = {}) => ({ job: "job-1", kind: "feature", title: "Build it", job_status: "queued", agent: "", session: "", status: "", started_at: "", updated_at: "", ended_at: "", cost: 0, steps: 0, ...over });
const task = (over = {}) => ({ id: "TASK-001", project: "one", title: "Ship it", status: "in_progress", priority: 10, lead: "one-lead", created_by: "owner", created_at: "", updated_at: "", last_at: "", messages: 0, last_message_at: "", last_author: "", needs_you: false, sessions: [], ...over });

test("a task's word puts the owner's turn first, then what the agents do", () => {
  assert.equal(taskMood(task({ status: "review", needs_you: true })).word, "Needs your reply");
  assert.equal(taskMood(task({ status: "review", needs_you: true })).tone, "wait");
  assert.equal(taskMood(task({ sessions: [session({ job_status: "leased", status: "running" })] })).word, "Working");
  assert.equal(taskMood(task({ sessions: [session({ job_status: "leased" })] })).word, "Working");
  assert.equal(taskMood(task({ sessions: [session({ job_status: "done", status: "ended" })] })).word, "In progress");
  assert.equal(taskMood(task({ status: "open", lead: "" })).word, "New");
  assert.equal(taskMood(task({ status: "open" })).word, "Waiting for the lead agent");
  assert.equal(taskMood(task({ status: "planned" })).word, "Planned");
  assert.equal(taskMood(task({ status: "done", sessions: [session({ status: "running" })] })).word, "Done");
  assert.equal(taskMood(task({ status: "cancelled" })).word, "Cancelled");
  assert.equal(taskFinished(task({ status: "done" })), true);
  assert.equal(taskFinished(task({ status: "cancelled" })), true);
  assert.equal(taskFinished(task({ status: "review" })), false);
});

test("a step's word follows the job until its session runs", () => {
  assert.deepEqual(sessionWords(session()), { word: "Waiting for a free agent", tone: "neutral" });
  assert.deepEqual(sessionWords(session({ job_status: "leased" })), { word: "Starting", tone: "run" });
  assert.deepEqual(sessionWords(session({ job_status: "leased", status: "running" })), { word: "Working", tone: "run" });
  assert.deepEqual(sessionWords(session({ job_status: "blocked" })), { word: "Blocked", tone: "wait" });
  assert.deepEqual(sessionWords(session({ job_status: "done", status: "ended" })), { word: "Done", tone: "ok" });
  assert.deepEqual(sessionWords(session({ job_status: "failed", status: "provider_error" })), { word: "Failed", tone: "bad" });
  assert.deepEqual(sessionWords(session({ job_status: "cancelled" })), { word: "Cancelled", tone: "neutral" });
  assert.equal(sessionWords(session({ job_status: "odd_state" })).word, "Odd state");
});

test("the task's title is the brief's first real line, cut at a word when long", () => {
  assert.equal(taskTitleOf("Add a dark mode toggle\n\nAnd tests."), "Add a dark mode toggle");
  assert.equal(taskTitleOf("\n\n## Settings page\n- toggle"), "Settings page");
  assert.equal(taskTitleOf("1. Ship the login\n2. Then 2FA"), "Ship the login");
  assert.equal(taskTitleOf("2FA for login"), "2FA for login");
  assert.equal(taskTitleOf("   "), "");
  const long = "word ".repeat(40).trim();
  const title = taskTitleOf(long);
  assert.ok(title.endsWith("\u2026"), title);
  assert.ok(title.length <= 121, String(title.length));
  assert.ok(!title.includes("word\u2026") || title.endsWith("word\u2026"));
  assert.equal(taskTitleOf("a".repeat(200)), "a".repeat(120) + "\u2026");
});
