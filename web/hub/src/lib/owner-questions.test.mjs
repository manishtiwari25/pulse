import assert from "node:assert/strict";
import { test } from "node:test";
import { askTitle, decisionLabels, sortOwnerQuestions } from "./owner-questions.ts";

test("stop questions lead, stale threads trail, newest first within a group", () => {
  const rows = [
    { id: "a", stop: false, stale: false, date: "2026-09-03T00:00:00Z" },
    { id: "b", stop: true, stale: true, date: "2026-09-04T00:00:00Z" },
    { id: "c", stop: true, stale: false, date: "2026-09-01T00:00:00Z" },
    { id: "d", stop: false, stale: false, date: "2026-09-05T00:00:00Z" },
    { id: "e", stop: true, stale: false, date: "2026-09-02T00:00:00Z" },
  ];
  assert.deepEqual(sortOwnerQuestions(rows).map((r) => r.id), ["e", "c", "b", "d", "a"]);
  assert.deepEqual(rows.map((r) => r.id), ["a", "b", "c", "d", "e"], "the input is left alone");
});

test("ties on date fall back to the id so the order is stable across reloads", () => {
  const rows = [
    { id: "y", stop: false, stale: false, date: "2026-09-03T00:00:00Z" },
    { id: "x", stop: false, stale: false, date: "2026-09-03T00:00:00Z" },
  ];
  assert.deepEqual(sortOwnerQuestions(rows).map((r) => r.id), ["x", "y"]);
});

test("a post labels the owner's buttons with its Yes: and No: lines", () => {
  const body = "Dependabot #20 in web/hub: a major upgrade.\n\nYes: Upgrade it\nNo: Leave it to me.\n\nYes queues a job that moves next to 17.x.";
  assert.deepEqual(decisionLabels(body), { yes: "Upgrade it", no: "Leave it to me" });
  assert.deepEqual(decisionLabels("Pick one."), { yes: "Yes", no: "No" });
  assert.deepEqual(decisionLabels(undefined, { yes: "Yes, carry on", no: "No, keep it paused" }), { yes: "Yes, carry on", no: "No, keep it paused" });
  // A sentence after the word is the explanation, not a label; bold marks are allowed.
  assert.deepEqual(decisionLabels("**Yes**: Do it\nNo: " + "x".repeat(41)), { yes: "Do it", no: "No" });
  assert.deepEqual(decisionLabels("Yes queues a job.\nNo leaves it."), { yes: "Yes", no: "No" });
});

test("the card names a post by its kind", () => {
  assert.equal(askTitle("question", "alpha"), "Question from alpha");
  assert.equal(askTitle(undefined, "alpha"), "Question from alpha");
  assert.equal(askTitle("finding", "daemon"), "Your call, from daemon");
  assert.equal(askTitle("handoff", "alpha"), "Handed to you by alpha");
  assert.equal(askTitle("suggestion", "alpha"), "Proposed to you by alpha");
});
