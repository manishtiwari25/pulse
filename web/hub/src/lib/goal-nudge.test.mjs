import assert from "node:assert/strict";
import { test } from "node:test";
import { dismissGoalNudge, dismissedSet, forgetOwnerDismissals, goalNudgeKey, goalNudges, readDismissed, wantsGoalNudge } from "./goal-nudge.ts";

const memory = (entries = {}) => {
  const m = new Map(Object.entries(entries));
  return { getItem: (k) => (m.has(k) ? m.get(k) : null), setItem: (k, v) => void m.set(k, v), removeItem: (k) => void m.delete(k), keys: () => [...m.keys()] };
};

const row = (slug, extra = {}) => ({ slug, present: true, missing: false, error: "", goal_source: "generated", ...extra });

test("only present projects on the generated goal want the note", () => {
  assert.equal(wantsGoalNudge(row("a")), true);
  assert.equal(wantsGoalNudge(row("b", { goal_source: "owner" })), false);
  assert.equal(wantsGoalNudge(row("c", { goal_source: undefined })), false, "an older hub says nothing about goals");
  assert.equal(wantsGoalNudge(row("d", { present: false })), false);
  assert.equal(wantsGoalNudge(row("e", { missing: true })), false);
  assert.equal(wantsGoalNudge(row("f", { error: "snapshot failed" })), false);
});

test("the notes keep the grid's order and skip dismissed projects", () => {
  const rows = [row("one"), row("two", { goal_source: "owner" }), row("three"), row("four", { present: false })];
  assert.deepEqual(goalNudges(rows, new Set()).map((p) => p.slug), ["one", "three"]);
  assert.deepEqual(goalNudges(rows, new Set(["one"])).map((p) => p.slug), ["three"]);
});

test("a dismissal is one key per project and reads back as one sorted line", () => {
  const storage = memory();
  dismissGoalNudge(storage, "two");
  dismissGoalNudge(storage, "one");
  assert.equal(storage.getItem(goalNudgeKey("one")), "dismissed");
  assert.equal(goalNudgeKey("one"), "pulse.goal-nudge.one");
  const rows = [row("one"), row("two"), row("three")];
  assert.equal(readDismissed(storage, rows), "one\ntwo");
  assert.deepEqual([...dismissedSet(readDismissed(storage, rows))], ["one", "two"]);
  assert.deepEqual([...dismissedSet("")], [], "nothing dismissed reads as the empty line");
  assert.deepEqual(goalNudges(rows, dismissedSet(readDismissed(storage, rows))).map((p) => p.slug), ["three"]);
});

test("a dismissal holds until the goal is the owner's, then the key goes so a later reset shows the note again", () => {
  const storage = memory();
  dismissGoalNudge(storage, "one");
  dismissGoalNudge(storage, "two");
  assert.equal(forgetOwnerDismissals(storage, [row("one"), row("two")]), false, "generated goals keep their dismissal");
  assert.equal(forgetOwnerDismissals(storage, [row("one", { goal_source: "owner" }), row("two")]), true);
  assert.deepEqual(storage.keys(), ["pulse.goal-nudge.two"]);
  assert.equal(forgetOwnerDismissals(storage, [row("one", { goal_source: "owner" })]), false, "nothing left to forget");
  const back = [row("one"), row("two")];
  assert.deepEqual(goalNudges(back, dismissedSet(readDismissed(storage, back))).map((p) => p.slug), ["one"]);
});

test("other values and other projects' keys are left alone", () => {
  const storage = memory({ "pulse.goal-nudge.one": "later", "pulse.goal-nudge.gone": "dismissed", "pulse.home.details": "open" });
  assert.equal(readDismissed(storage, [row("one"), row("two")]), "", "only the exact value counts");
  assert.equal(forgetOwnerDismissals(storage, [row("one", { goal_source: "owner" })]), true, "any stale value for an owner goal goes");
  assert.deepEqual(storage.keys().sort(), ["pulse.goal-nudge.gone", "pulse.home.details"]);
});
