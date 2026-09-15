import assert from "node:assert/strict";
import { test } from "node:test";
import { evolvePlanningView } from "./evolve-planning.ts";

test("planning labels distinguish eligibility, dispatch and finalization", () => {
  for (const [state, label] of [
    ["gated", "Gated"],
    ["blocked", "Blocked"],
    ["waiting", "Waiting for suggestions"],
    ["carding", "Writing an experiment card"],
    ["ready", "Ready, not dispatched"],
    ["queued", "Queued"],
    ["leased", "Experiment in flight"],
    ["finalizing", "Finalizing"],
  ]) {
    const view = evolvePlanningView({ state, reason: "The actual controller reason" });
    assert.equal(view.label, label);
    assert.equal(view.reason, "The actual controller reason");
    assert.ok(view.next);
    assert.notEqual(view.tone, "ok", "a planning state is not a successful measured result");
  }
});

test("Level D with no eligible candidate does not promise an active loop", () => {
  const view = evolvePlanningView({ state: "waiting", reason: "operating layer only; no eligible suggestion" });
  assert.match(view.next, /eligible, untried, in-scope/);
  assert.doesNotMatch(view.next, /nothing waits|every gate is open/i);
  assert.match(view.reason, /operating layer only/);
});

test("carding names the agent's card job, not an owner step", () => {
  const view = evolvePlanningView({ state: "carding", reason: "suggestion X has no experiment card; card job J (queued) writes one as a new suggestion on its thread", job: "J" });
  assert.equal(view.tone, "info");
  assert.match(view.next, /card job/);
  assert.doesNotMatch(view.next, /owner/i);
  assert.match(view.reason, /card job J/);
});

test("blocked planning retains the gate rather than claiming no owner step", () => {
  const view = evolvePlanningView({ state: "blocked", reason: "owner review needed" });
  assert.equal(view.tone, "warn");
  assert.match(view.next, /Resolve the planning issue/);
  assert.match(view.next, /not changed automatically/);
});

test("queued and finalizing are not completed cycles", () => {
  assert.match(evolvePlanningView({ state: "queued", reason: "queued" }).next, /STOP and runner caps/);
  assert.match(evolvePlanningView({ state: "finalizing", reason: "accepting artifacts" }).next, /not proof of a completed cycle/);
});

test("older hubs and unknown states remain explicitly unavailable", () => {
  for (const planning of [undefined, null]) {
    const view = evolvePlanningView(planning);
    assert.equal(view.label, "Planning unavailable");
    assert.match(view.next, /does not mean an experiment is running/);
  }
  for (const state of ["", "new-state", "__proto__", "constructor"]) {
    const view = evolvePlanningView({ state, reason: "Unrecognized state from the hub" });
    assert.equal(view.label, "Unknown planning state");
    assert.equal(view.tone, "warn");
    assert.equal(view.reason, "Unrecognized state from the hub");
  }
  assert.match(evolvePlanningView({ state: "ready", reason: "" }).reason, /did not report/);
});
