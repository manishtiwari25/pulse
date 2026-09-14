import assert from "node:assert/strict";
import { test } from "node:test";
import { hrefs } from "./nav.ts";

test("code delivery drill-down links preserve global and project scope", () => {
  assert.equal(hrefs.codeDeliveries(), "/code-deliveries/");
  assert.equal(hrefs.codeDeliveries({ project: "demo-app" }), "/code-deliveries/?project=demo-app");
});

test("harness links do not advertise an unsupported job filter", () => {
  assert.equal(hrefs.harness({ project: "demo-app", agent: "cinder" }), "/harness/?project=demo-app&agent=cinder");
  assert.equal(hrefs.harness({ project: "demo-app", agent: "cinder", job: "job-price-cache" }), "/harness/?project=demo-app&agent=cinder");
});
