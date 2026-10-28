// Stamps the static export with when this build finished.
// `npm run build` runs this after `next build`; the stale-bundle test
// (web/hub/src/lib/bundle-freshness.test.mjs) compares this stamp against
// the newest file under src/.
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

writeFileSync(join(outDir, ".build-stamp"), new Date().toISOString() + "\n");
