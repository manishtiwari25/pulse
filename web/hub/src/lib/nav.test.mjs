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

test("record evidence links agree with the board hrefs", async () => {
  const { recordEvidence } = await import("./record-evidence.ts");
  const [job, post] = recordEvidence("demo-app", "job-20261004T221802Z-28402f 20261004T221729Z-cinder-3");
  assert.equal(job.href, hrefs.thread("demo-app", "evolution/job-20261004t221802z-28402f"));
  assert.equal(post.href, hrefs.post("demo-app", "20261004T221729Z-cinder-3"));
});
