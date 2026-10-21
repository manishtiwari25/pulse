import assert from "node:assert/strict";
import { test } from "node:test";
import { sortOwnerQuestions } from "./owner-questions.ts";

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
