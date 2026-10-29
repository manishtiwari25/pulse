import assert from "node:assert/strict";
import { register } from "node:module";
import { test } from "node:test";
import React from "react";

// Render both question classes the For you tab shows: what still waits for the
// owner (QuestionCard with decision buttons) and what the agents settled
// themselves (the collapsed "Answered by the agents" audit). The point of the
// experiment: the owner can audit the agents' answers without those rows ever
// inflating the waiting count.
register("./tsx-test-loader.mjs", import.meta.url);

const { renderToStaticMarkup } = await import("react-dom/server");
const { AgentsDecidedAudit, AGENTS_DECIDED_SUMMARY } = await import("./tabs.tsx");
const { QuestionCard } = await import("../pulse/cards.tsx");
const { buildInbox } = await import("../pulse/inbox.ts");

const base = {
  project: "demo-app",
  slug: "demo-app",
  thread_key: "demo-app:governance/vote-reasons",
  area: "governance",
  thread: "vote-reasons",
  kind: "question",
  on_post: "",
  stop: false,
  stale: false,
  thread_last: "",
  agent: "agent-rose",
};

const OWNER_Q = {
  ...base,
  id: "q-owner",
  date: "2026-10-28T10:00:00Z",
  summary: "Ship the beta to everyone?",
  body: "The fleet asks whether the beta opens to all projects now.",
  decider: "owner",
};

const LEAD_Q = {
  ...base,
  id: "q-lead",
  date: "2026-10-28T11:00:00Z",
  summary: "Retry the flaky landing?",
  body: "Settled by the lead while the owner was away.",
  decider: "lead",
};

const SETTING_Q = {
  ...base,
  id: "q-setting",
  date: "2026-10-28T12:00:00Z",
  summary: "Raise the daily operating-layer cap?",
  body: "A configured default settled this one.",
  decider: "setting",
};

const e = React.createElement;

test("agents-decided audit lists the lead/setting questions collapsed under a toggle", () => {
  const html = renderToStaticMarkup(e(AgentsDecidedAudit, { questions: [LEAD_Q, SETTING_Q], slug: "demo-app" }));
  // Collapsed under a toggle: a closed <details> carries the summary line.
  assert.match(html, /<details(?![^>]*\bopen\b)/);
  assert.ok(html.includes(AGENTS_DECIDED_SUMMARY), "toggle names the agents-decided class");
  assert.ok(html.includes("· 2"), "toggle carries the count of settled questions");
  // Both agent-settled classes are auditable; the owner's row never leaks in.
  assert.ok(html.includes(LEAD_Q.summary), "lead-settled question renders");
  assert.ok(html.includes(SETTING_Q.summary), "setting-settled question renders");
  assert.ok(html.includes("Settled by the lead"), "the lead class is named");
  assert.ok(html.includes("Settled by a setting"), "the setting class is named");
  assert.ok(!html.includes(OWNER_Q.summary), "owner question never leaks into the audit");
});

test("agents-decided rows are read-only (nothing to decide) while the owner's card keeps its buttons", () => {
  const audit = renderToStaticMarkup(e(AgentsDecidedAudit, { questions: [LEAD_Q], slug: "demo-app" }));
  assert.ok(!audit.includes('aria-label="Yes to'), "settled questions offer no Yes button");
  assert.ok(!audit.includes('aria-label="No to'), "settled questions offer no No button");
  assert.ok(audit.includes("See it on the board"), "the audit links to the thread holding the answer");
  const waiting = renderToStaticMarkup(e(QuestionCard, { q: OWNER_Q, slug: "demo-app", canDecide: true }));
  assert.ok(waiting.includes('aria-label="Yes to'), "owner-decidable question still renders its decision buttons");
});

test("For you count stays accurate: agents-settled questions never enter the waiting total", () => {
  const loaded = (data) => ({ loading: false, error: "", data });
  const empty = { loading: false, error: "", data: undefined };
  const inbox = buildInbox({
    org: loaded({ name: "org", projects: [], totals: {}, proposals: [], questions: [OWNER_Q, LEAD_Q, SETTING_Q] }),
    proposed: empty,
    reg: empty,
    compute: empty,
    runner: empty,
    service: empty,
    settings: empty,
  });
  const mine = inbox.forProject("demo-app");
  assert.equal(mine.questions.length, 1, "only the owner-decidable question waits");
  assert.equal(mine.questions[0].id, OWNER_Q.id);
  assert.equal(mine.agentsDeciding, 2, "both agent-settled classes are counted separately");
  assert.equal(mine.total, 1, "the For you total excludes what the agents settled");
});
