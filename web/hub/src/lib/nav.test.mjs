import assert from "node:assert/strict";
import { test } from "node:test";
import { hrefs, resolveProjectTab } from "./nav.ts";

test("code delivery drill-down links preserve global and project scope", () => {
  assert.equal(hrefs.codeDeliveries(), "/code-deliveries/");
  assert.equal(hrefs.codeDeliveries({ project: "demo-app" }), "/code-deliveries/?project=demo-app");
});

test("harness links do not advertise an unsupported job filter", () => {
  assert.equal(hrefs.harness({ project: "demo-app", agent: "cinder" }), "/harness/?project=demo-app&agent=cinder");
  assert.equal(hrefs.harness({ project: "demo-app", agent: "cinder", job: "job-price-cache" }), "/harness/?project=demo-app&agent=cinder");
});

test("project tab links resolve old ids to human tabs", () => {
  assert.deepEqual(resolveProjectTab(null), { tab: "overview" });
  assert.deepEqual(resolveProjectTab("proposals"), { tab: "for-you" });
  assert.deepEqual(resolveProjectTab("tasks"), { tab: "work" });
  assert.deepEqual(resolveProjectTab("chat"), { tab: "work" });
  assert.deepEqual(resolveProjectTab("suggestions"), { tab: "board", sub: "suggestions" });
  assert.deepEqual(resolveProjectTab("env"), { tab: "history", sub: "env" });
  assert.deepEqual(resolveProjectTab("history"), { tab: "history" });
  assert.equal(hrefs.project("demo-app", "tasks"), "/project/?slug=demo-app&tab=work");
  assert.equal(hrefs.project("demo-app", "suggestions"), "/project/?slug=demo-app&tab=board&sub=suggestions");
  assert.equal(hrefs.task("demo-app", "TASK-001"), "/project/?slug=demo-app&tab=work&task=TASK-001");
});
