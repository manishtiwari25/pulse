import assert from "node:assert/strict";
import { test } from "node:test";
import { beatsOf, brief, countWords, groupNotes, streamTail, thoughtSeconds } from "./transcript.ts";

// A minimal HarnessStep; the folds below only read these fields.
function step(kind, fields = {}) {
  return {
    n: 0,
    kind,
    title: "",
    command: "",
    content: "",
    at: "2026-10-20T10:00:00Z",
    duration_ms: 0,
    depth: 0,
    ...fields,
  };
}

test("thoughtSeconds prefers the measured latency over the step gap", () => {
  const prev = step("command", { at: "2026-10-20T10:00:00Z" });
  const s = step("assistant", { at: "2026-10-20T10:00:40Z", duration_ms: 3200 });
  assert.equal(thoughtSeconds(s, prev), 3.2);
});

test("thoughtSeconds falls back to the gap since the step before", () => {
  const prev = step("command", { at: "2026-10-20T10:00:00Z" });
  const s = step("assistant", { at: "2026-10-20T10:00:05Z" });
  assert.equal(thoughtSeconds(s, prev), 5);
});

test("thoughtSeconds ignores sub-second answers and missing or reversed clocks", () => {
  assert.equal(thoughtSeconds(step("assistant", { duration_ms: 400 }), null), 0);
  const prev = step("command", { at: "2026-10-20T10:00:05Z" });
  assert.equal(thoughtSeconds(step("assistant", { at: "2026-10-20T10:00:00Z" }), prev), 0);
  assert.equal(thoughtSeconds(step("assistant", { at: "not a date" }), prev), 0);
});

test("beatsOf folds wordless replies into the tool run as thought rows", () => {
  const steps = [
    step("assistant", { content: "Let me look.", at: "2026-10-20T10:00:00Z" }),
    step("read", { title: "Read a file", at: "2026-10-20T10:00:01Z" }),
    step("assistant", { content: "", duration_ms: 2600, at: "2026-10-20T10:00:04Z" }),
    step("command", { title: "Ran tests", at: "2026-10-20T10:00:05Z" }),
  ];
  const beats = beatsOf(steps);
  assert.deepEqual(beats.map((b) => b.kind), ["said", "actions"]);
  assert.deepEqual(
    beats[1].items.map((it) => it.step.kind),
    ["read", "assistant", "command"],
  );
  assert.equal(beats[1].items[1].thought, 2.6);
});

test("beatsOf pairs an observation with the latest unmatched action at the same depth", () => {
  const steps = [
    step("subagent", { depth: 0 }),
    step("command", { depth: 1 }),
    step("observation", { content: "helper output", depth: 1 }),
    step("observation", { content: "helper report", depth: 0 }),
  ];
  const beats = beatsOf(steps);
  assert.deepEqual(beats.map((b) => b.kind), ["actions"]);
  const items = beats[0].items;
  assert.equal(items.length, 2);
  assert.equal(items[0].step.kind, "subagent");
  assert.equal(items[0].result?.content, "helper report");
  assert.equal(items[1].step.kind, "command");
  assert.equal(items[1].result?.content, "helper output");
});

test("beatsOf leaves an observation without a matching action unmatched and drops the row", () => {
  const steps = [
    step("command", { depth: 0 }),
    step("observation", { content: "done", depth: 0 }),
    step("observation", { content: "orphan", depth: 0 }),
  ];
  const items = beatsOf(steps)[0].items;
  assert.equal(items.length, 1);
  assert.equal(items[0].result?.content, "done");
});

test("beatsOf keeps notes out of action-kind pairing", () => {
  const steps = [
    step("note", { command: "mcp" }),
    step("observation", { content: "x" }),
  ];
  // The note shows on its own and the observation stays unmatched (and so
  // shows nowhere): a note is a one-line row, not an action with a result.
  const items = beatsOf(steps)[0].items;
  assert.equal(items.length, 1);
  assert.equal(items[0].result, null);
});

test("beatsOf breaks the tool run at words and at a plan", () => {
  const steps = [
    step("command"),
    step("assistant", { content: "Now the plan." }),
    step("plan"),
    step("edit"),
  ];
  const beats = beatsOf(steps);
  assert.deepEqual(beats.map((b) => b.kind), ["actions", "said", "plan", "actions"]);
});

test("streamTail cuts the segments already folded in as steps", () => {
  const stream = "First words. Second words. Still writing";
  const steps = [step("assistant", { content: "First words." }), step("assistant", { content: " Second words. " })];
  assert.equal(streamTail(stream, steps), "Still writing");
});

test("streamTail keeps text whose step is missing and ignores other kinds", () => {
  const stream = "Old words. New words";
  const steps = [step("command", { content: "Old words." }), step("assistant", { content: "Not in the stream." })];
  assert.equal(streamTail(stream, steps), stream);
});

test("streamTail empties when every segment became a step", () => {
  const stream = "All of it.";
  assert.equal(streamTail(stream, [step("assistant", { content: "All of it." })]), "");
});

test("groupNotes folds two or more notes in a row into one row", () => {
  const n1 = { step: step("note", { command: "mcp" }), result: null };
  const n2 = { step: step("note", { command: "skills" }), result: null };
  const action = { step: step("command"), result: null };
  const n3 = { step: step("note", { command: "resumed" }), result: null };
  const grouped = groupNotes([n1, n2, action, n3]);
  assert.equal(grouped.length, 3);
  assert.deepEqual(grouped[0], [n1, n2]);
  assert.equal(grouped[1], action);
  assert.equal(grouped[2], n3);
});

test("groupNotes leaves a thought row out of the fold", () => {
  const n1 = { step: step("note"), result: null };
  const thought = { step: step("assistant"), result: null, thought: 3 };
  const n2 = { step: step("note"), result: null };
  const grouped = groupNotes([n1, thought, n2]);
  assert.deepEqual(grouped, [n1, thought, n2]);
});

test("countWords counts the actions and skips the thought rows", () => {
  const items = [
    { step: step("command"), result: null },
    { step: step("command"), result: null },
    { step: step("read"), result: null },
    { step: step("assistant"), result: null, thought: 2.4 },
    { step: step("note"), result: null },
  ];
  assert.equal(countWords(items), "2 commands, 1 file read");
});

test("countWords falls back to notes when nothing was an action", () => {
  const items = [{ step: step("note"), result: null }, { step: step("note"), result: null }];
  assert.equal(countWords(items), "2 notes");
});

test("brief rounds to a compact span", () => {
  assert.equal(brief(3.2), "3s");
  assert.equal(brief(0.1), "1s");
  assert.equal(brief(80), "1m 20s");
  assert.equal(brief(120), "2m");
});
