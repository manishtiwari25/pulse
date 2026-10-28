// A stale committed bundle makes console work land invisible: a change to
// src/ without a rebuilt out/ ships nothing, because the pulse binary embeds
// out/ (rust-embed in crates/pulse-hub/src/server.rs). This test fails when
// the last commit touching src/ is newer than the last commit touching out/.
// `npm run build` also stamps the export it produces
// (scripts/write-build-stamp.mjs writes out/.build-stamp) and the test
// cross-checks that stamp against the last src/ change, so a rebuilt-but-
// never-committed bundle is caught too; the check's scratch build stamps
// the scratch dir, never the committed out/.

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

// .../web/hub/src/lib/bundle-freshness.test.mjs -> src/lib -> src -> web/hub
const hubRoot = dirname(dirname(dirname(fileURLToPath(import.meta.url))));

// Compare commit timestamps, not file mtimes: a fresh checkout rewrites
// every file's mtime, which would falsify the signal. `git log` answers the
// same question the embed does - is the committed src/ newer than the
// committed out/?
const lastCommitMs = (pathspec) => {
  const out = execFileSync("git", ["log", "-1", "--format=%cI", "--", pathspec], {
    cwd: hubRoot,
    encoding: "utf8",
  }).trim();
  return out ? Date.parse(out) : 0;
};

test("the committed out/ bundle is not older than src/", () => {
  const stampPath = join(hubRoot, "out", ".build-stamp");
  assert.ok(
    existsSync(stampPath),
    "out/.build-stamp is missing: run `npm run build` in web/hub and commit out/ so the hub binary serves this source",
  );
  const stamp = Date.parse(readFileSync(stampPath, "utf8").trim());
  assert.ok(Number.isFinite(stamp), `out/.build-stamp is not an ISO date: ${readFileSync(stampPath, "utf8")}`);
  const srcCommit = lastCommitMs("src/");
  const outCommit = lastCommitMs("out/");
  assert.ok(
    outCommit >= srcCommit,
    `web/hub/src last changed in a commit (${new Date(srcCommit).toISOString()}) newer than the last commit of web/hub/out (${new Date(outCommit).toISOString()}), so the committed bundle is stale: run \`npm run build\` in web/hub and commit out/`,
  );
  assert.ok(
    stamp >= srcCommit - 60_000,
    `out/.build-stamp (${new Date(stamp).toISOString()}) predates the last src/ change (${new Date(srcCommit).toISOString()}) by more than a minute: run \`npm run build\` in web/hub and commit out/`,
  );
});
