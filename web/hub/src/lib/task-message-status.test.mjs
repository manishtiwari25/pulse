import assert from "node:assert/strict";
import { test } from "node:test";
import { STALE_AFTER_MS, taskMessageLine, taskMessageState } from "./task-message-status.ts";

const AT = "2026-10-25T14:05:00Z";
const NOW = Date.parse(AT) + 10_000;
const lead = "amistio-template-lead";
const owner = (read_by = []) => ({ author: "human", at: AT, read_by });
const idle = { state: "idle" };

test("an agent's message after the owner's is the answer: nothing to say", () => {
  const s = taskMessageState(owner(), [{ author: lead, at: "2026-10-25T14:12:00Z" }], lead, idle, NOW);
  assert.deepEqual(s, { kind: "answered" });
  assert.equal(taskMessageLine(s), "");
});

test("a worker's report after it is news, not the lead's answer", () => {
  const report = { author: "cinder", at: "2026-10-25T14:07:00Z", job_id: "job-20261025T140600Z-ab12cd" };
  const s = taskMessageState(owner(), [report], lead, idle, NOW);
  assert.equal(s.kind, "sent");
  const answered = taskMessageState(owner(), [report, { author: lead, at: "2026-10-25T14:12:00Z" }], lead, idle, NOW);
  assert.deepEqual(answered, { kind: "answered" });
});

test("more owner messages after it are not an answer", () => {
  const s = taskMessageState(owner(), [{ author: "human", at: "2026-10-25T14:06:00Z" }], lead, idle, NOW);
  assert.equal(s.kind, "sent");
});

test("just sent, nobody holds it: Sent", () => {
  const s = taskMessageState(owner(), [], lead, idle, NOW);
  assert.deepEqual(s, { kind: "sent", lead, stale: false, busy: false });
  assert.equal(taskMessageLine(s), "Sent");
});

test("unpicked past the daemon's own tick: the line says so", () => {
  const s = taskMessageState(owner(), [], lead, idle, Date.parse(AT) + STALE_AFTER_MS + 1);
  assert.equal(s.kind, "sent");
  assert.equal(s.stale, true);
  assert.equal(taskMessageLine(s), "Sent · nobody has picked it up yet");
});

test("the lead is in an earlier sitting that does not hold it: busy, reads it next", () => {
  const work = { state: "running", since: "2026-10-25T14:00:00Z", agent: lead };
  const s = taskMessageState(owner(), [], lead, work, NOW);
  assert.deepEqual(s, { kind: "sent", lead, stale: false, busy: true });
  assert.equal(taskMessageLine(s), "Sent · amistio-template-lead is in another sitting and reads it next");
});

test("read by the lead and its sitting queued: in line", () => {
  const work = { state: "queued", since: "2026-10-25T14:05:01Z" };
  const s = taskMessageState(owner([lead]), [], lead, work, NOW);
  assert.deepEqual(s, { kind: "in_line", lead });
  assert.equal(taskMessageLine(s), "Sent · amistio-template-lead reads it next");
});

test("a sitting queued after the message holds it even before read_by arrives", () => {
  const work = { state: "queued", since: "2026-10-25T14:05:30Z" };
  assert.equal(taskMessageState(owner(), [], lead, work, NOW).kind, "in_line");
});

test("read by the lead and its sitting running: reading, with the run's start", () => {
  const work = { state: "running", since: "2026-10-25T14:06:00Z", agent: lead };
  const s = taskMessageState(owner([lead]), [], lead, work, NOW);
  assert.deepEqual(s, { kind: "reading", lead, since: "2026-10-25T14:06:00Z" });
  assert.equal(taskMessageLine(s), "amistio-template-lead is reading it…");
});

test("read by the lead, sitting over, no reply here: seen", () => {
  const work = { state: "idle", read_at: "2026-10-25T14:06:00Z" };
  const s = taskMessageState(owner([lead]), [], lead, work, NOW);
  assert.deepEqual(s, { kind: "seen", lead, at: "2026-10-25T14:06:00Z" });
  assert.equal(taskMessageLine(s), "Seen by amistio-template-lead");
});

test("a sitting that began after the message counts as seen even without read_by", () => {
  const work = { state: "idle", read_at: "2026-10-25T14:07:00Z" };
  assert.equal(taskMessageState(owner(), [], lead, work, NOW).kind, "seen");
  const earlier = { state: "idle", read_at: "2026-10-25T14:04:00Z" };
  assert.equal(taskMessageState(owner(), [], lead, earlier, NOW).kind, "sent");
});

test("without a lead name the work's agent, else a plain phrase, names the reader", () => {
  const work = { state: "running", since: "2026-10-25T14:06:00Z", agent: "demo-lead" };
  assert.equal(taskMessageState(owner(), [], undefined, work, NOW).lead, "demo-lead");
  assert.equal(taskMessageState(owner(), [], undefined, null, NOW).lead, "the lead agent");
});

test("the owner's own name may be 'owner'", () => {
  const s = taskMessageState({ author: "owner", at: AT }, [{ author: "owner", at: AT }], lead, idle, NOW);
  assert.equal(s.kind, "sent");
});
