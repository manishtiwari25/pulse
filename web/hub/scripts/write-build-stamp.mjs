// Stamps the static export with the src/ tree this build was made from, so
// the stale-bundle test (web/hub/src/lib/bundle-freshness.test.mjs) proves
// the committed out/ was built from the committed src/ - provenance, not a
// clock: a verify hook can restamp a commit minutes after the build without
// falsifying the signal. When src/ has uncommitted changes the stamp says
// "dirty" and the test demands the commit time instead, so an in-flight edit
// is still caught.
//
// `npm run build` runs this after `next build`.
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const hubRoot = dirname(dirname(fileURLToPath(import.meta.url)));

// PULSE_HUB_OUT_DIR must mirror next.config.ts: set (as `npm run check`
// does) the build writes a scratch export that stays out of the commit, so
// the stamp goes there too; unset, the export lands in the committed out/.
// next.config.ts resolves the variable against the hub root, so do the same.
const outDir = process.env.PULSE_HUB_OUT_DIR
  ? join(hubRoot, process.env.PULSE_HUB_OUT_DIR)
  : join(hubRoot, "out");

const git = (args) => execFileSync("git", args, { cwd: hubRoot, encoding: "utf8" }).trim();

// The tree object hash of the committed src/ this build stands on. A clean
// src/ means the working tree matches HEAD, so the hash names exactly what
// was built; a dirty src/ cannot be named by a committed tree, so stamp
// "dirty" and let the test fall back to the commit time.
const clean = git(["status", "--porcelain", "--", "src/"]) === "";
const stamp = clean ? `src-tree ${git(["rev-parse", "HEAD^{tree}:./src"])}` : `dirty ${new Date().toISOString()}`;

writeFileSync(join(outDir, ".build-stamp"), stamp + "\n");
console.log(`build stamp: ${stamp}`);
