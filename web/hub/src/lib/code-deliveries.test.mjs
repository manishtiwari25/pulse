import assert from "node:assert/strict";
import { test } from "node:test";
import { codeDeliveriesPath } from "./api.ts";
import { codeDeliveryCoverageText, codeDeliveryIncompleteWindow, codeDeliveryKpiState, codeDeliveryKpiSub, codeDeliveryWindowLabel, shortCommit } from "./code-deliveries.ts";

const payload = (overrides = {}) => ({
  scope: { project: null, limit: 100, scanned: 4, has_more: false, next_before: null, ...(overrides.scope || {}) },
  summary: { code_deliveries: 0, code_files: 0, docs_only: 0, non_code: 0, unknown: 0, duplicates: 0, ...(overrides.summary || {}) },
  deliveries: overrides.deliveries || [],
  unavailable: overrides.unavailable || [],
});

test("code delivery path uses the frozen query contract", () => {
  assert.equal(codeDeliveriesPath(), "code-deliveries");
  assert.equal(codeDeliveriesPath({ project: "demo app", limit: 50, before: 123 }), "code-deliveries?project=demo+app&limit=50&before=123");
  assert.equal(codeDeliveriesPath({ project: "", limit: undefined, before: null }), "code-deliveries");
});

test("coverage text does not collapse unknown evidence to zero", () => {
  const text = codeDeliveryCoverageText(payload({ summary: { unknown: 2, docs_only: 1, non_code: 1, duplicates: 1 } }));
  assert.match(text, /2 unknown evidence rows not counted as zero/);
  assert.match(text, /1 docs-only landing and 1 non-code landing excluded/);
  assert.match(text, /1 duplicate deduped/);
  assert.doesNotMatch(text, /complete evidence/);
});

test("zero state is explicit only when the window evidence is complete", () => {
  assert.match(codeDeliveryCoverageText(payload()), /zero is based on complete evidence/);
  assert.doesNotMatch(codeDeliveryCoverageText(payload({ unavailable: [{ event_seq: 3, project: "demo", at: "2026-09-01T00:00:00Z", reason: "legacy" }] })), /complete evidence/);
});

test("sparse bounded scan is an incomplete search window, not zero", () => {
  const d = payload({ scope: { scanned: 0, has_more: true, next_before: 42 } });
  assert.equal(codeDeliveryIncompleteWindow(d), true);
  assert.match(codeDeliveryCoverageText(d), /Incomplete search window/);
  assert.match(codeDeliveryCoverageText(d), /not a definitive zero/);
  assert.doesNotMatch(codeDeliveryCoverageText(d), /complete evidence/);
  assert.deepEqual(codeDeliveryKpiState(d, false, ""), { n: "—", sub: "incomplete search window · older evidence available", tone: "warn" });
});

test("KPI and window labels name bounded landing windows", () => {
  const d = payload({ scope: { project: "demo-lib", scanned: 8 }, summary: { code_deliveries: 2, code_files: 3 } });
  assert.equal(codeDeliveryWindowLabel(d, 2), "older landing window 2 for demo-lib · scanned 8 landing events · limit 100");
  assert.equal(codeDeliveryKpiSub(d, false, ""), "3 source-file changes · 8 landing events scanned");
  assert.equal(codeDeliveryKpiSub(undefined, true, ""), "loading recent landings");
  assert.equal(codeDeliveryKpiSub(undefined, false, "offline"), "unavailable");
});

test("KPI state marks cached refresh failures stale and warning", () => {
  const d = payload({ summary: { code_deliveries: 1, code_files: 2 } });
  assert.deepEqual(codeDeliveryKpiState(undefined, false, "offline"), { n: "—", sub: "unavailable", tone: "warn" });
  assert.deepEqual(codeDeliveryKpiState(d, false, ""), { n: 1, sub: "2 source-file changes · 4 landing events scanned", tone: "ok" });
  assert.deepEqual(codeDeliveryKpiState(d, false, "offline"), { n: 1, sub: "stale - refresh failed · 2 source-file changes · 4 landing events scanned", tone: "warn" });
});

test("commit display is bounded", () => {
  assert.equal(shortCommit("0123456789abcdef"), "0123456789ab");
  assert.equal(shortCommit(""), "unknown commit");
});
