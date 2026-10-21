import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createCodeDeliveryMock } from "./code-deliveries.mjs";

const params = (q = "") => new URLSearchParams(q);
const fixture = JSON.parse(readFileSync(join(dirname(fileURLToPath(import.meta.url)), "api/code-deliveries.json"), "utf8"));

test("mock code delivery endpoint reports the frozen response shape", () => {
  const { status, body } = createCodeDeliveryMock(fixture).read(params("limit=10"));
  assert.equal(status, 200);
  assert.equal(body.scope.project, null);
  assert.equal(body.scope.scanned, 7);
  assert.equal(body.summary.code_deliveries, 3);
  assert.equal(body.summary.code_files, 5);
  assert.equal(body.summary.docs_only, 1);
  assert.equal(body.summary.non_code, 1);
  assert.equal(body.summary.unknown, 1);
  assert.equal(body.summary.duplicates, 1);
  assert.equal(body.unavailable.length, 1);
  assert.equal(body.deliveries[0].suggestion.votes.score, 3);
});

test("mock code delivery endpoint filters projects and pages by event cursor", () => {
  const mock = createCodeDeliveryMock(fixture);
  const { body: app } = mock.read(params("project=demo-app&limit=2"));
  assert.equal(app.scope.project, "demo-app");
  assert.equal(app.scope.has_more, true);
  assert.equal(app.scope.next_before, 229);
  assert.deepEqual(app.deliveries.map((d) => d.project), ["demo-app"]);
  assert.equal(app.summary.docs_only, 1);

  const { body: older } = mock.read(params("project=demo-app&before=229&limit=10"));
  assert.equal(older.summary.unknown, 1);
  assert.equal(older.summary.duplicates, 1);
  assert.equal(older.deliveries[0].job, null);

  const { body: lib } = mock.read(params("project=demo-lib&limit=10"));
  assert.equal(lib.scope.scanned, 2);
  assert.equal(lib.summary.code_deliveries, 1);
  assert.equal(lib.summary.non_code, 1);
});

test("mock code delivery endpoint returns sparse incomplete windows for registered projects", () => {
  const { status, body } = createCodeDeliveryMock(fixture, ["old-site"]).read(params("project=old-site"));
  assert.equal(status, 200);
  assert.deepEqual(body.scope, { project: "old-site", limit: 100, scanned: 0, has_more: true, next_before: 224 });
  assert.deepEqual(body.deliveries, []);
  assert.deepEqual(body.unavailable, []);
});

test("mock code delivery endpoint validates like the real route", () => {
  const mock = createCodeDeliveryMock(fixture, ["demo-app", "demo-lib", "old-site"]);
  for (const [query, why] of [
    ["limit=0", /greater than zero/],
    ["limit=201", /at most 200/],
    ["limit=NaN", /positive whole number/],
    ["before=0", /greater than zero/],
    ["before=abc", /positive whole number/],
  ]) {
    const { status, body } = mock.read(params(query));
    assert.equal(status, 400, query);
    assert.match(body.why, why);
    assert.ok(body.error);
    assert.ok(body.fix);
  }
  const missing = mock.read(params("project=missing-project"));
  assert.equal(missing.status, 404);
  assert.equal(missing.body.error, "no such project");
});
