// A stale committed bundle makes console work land invisible: a change to
// src/ without a rebuilt out/ ships nothing, because the pulse binary embeds
// the committed out/ (crates/pulse-hub/src/server.rs) and cargo never runs
// Node. The bundle is a build output, so a file's mtime is whenever the
// checkout wrote it and proves nothing. What proves the bundle is fresh is
// that it was built from the src/ it ships beside: the build stamps out/
// with the src/ tree hash (scripts/write-build-stamp.mjs), and this test
// demands the committed stamp name the committed src/ tree. The commits
// themselves agree (the newest src/ commit and the newest out/ commit are
// one). A stamp from a dirty src/ names no tree, so it falls back to being
// at most a minute older than its commit.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";

const root = join(import.meta.dirname, "..", "..");
const git = (args) => execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();

const commitTime = (path) => Date.parse(git(["log", "-1", "--format=%cI", "--", path]));

test("the committed out/ bundle is built from the committed src/", () => {
  const stamp = readFileSync(join(root, "out", ".build-stamp"), "utf8").trim();
  const srcCommit = commitTime("src");
  const outCommit = commitTime("out");

  if (stamp.startsWith("src-tree ")) {
    // The build recorded the src/ tree it was made from; the commit must be
    // carrying exactly that src/.
    assert.equal(
      stamp.slice("src-tree ".length),
      git(["rev-parse", "HEAD^{tree}:./src"]),
      "the committed out/ was built from a different src/ than the commit carries - rebuild with `npm run build` and commit out/",
    );
  } else {
    // A build of a dirty src/ names no tree: fall back to the stamp being
    // fresh, so a just-built bundle waiting on a verify hook still passes.
    assert.ok(
      commitTime("out/.build-stamp") >= srcCommit - 60_000,
      `out/.build-stamp (${stamp}) is older than the newest src/ commit (${new Date(srcCommit).toISOString()}): rebuild with \`npm run build\` and commit out/`,
    );
  }

  // The bundle and the source land together: no commit touches src/ without
  // touching out/ in the same or a later commit.
  assert.ok(
    outCommit >= srcCommit,
    `newest out/ commit (${new Date(outCommit).toISOString()}) predates newest src/ commit (${new Date(srcCommit).toISOString()}): rebuild with \`npm run build\` and commit out/`,
  );
});
