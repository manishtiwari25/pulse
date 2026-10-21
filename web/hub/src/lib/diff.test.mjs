import assert from "node:assert/strict";
import { test } from "node:test";
import { diffTotals, dirOf, numberLines, shortPath, splitDiff, UNCOMMITTED_MARKER } from "./diff.ts";

const sample = [
  "diff --git a/src/a.ts b/src/a.ts",
  "index 1111111..2222222 100644",
  "--- a/src/a.ts",
  "+++ b/src/a.ts",
  "@@ -1,3 +1,4 @@",
  " const a = 1;",
  "-const b = 2;",
  "+const b = 3;",
  "+const c = 4;",
  "diff --git a/docs/new.md b/docs/new.md",
  "new file mode 100644",
  "index 0000000..3333333",
  "--- /dev/null",
  "+++ b/docs/new.md",
  "@@ -0,0 +1,2 @@",
  "+# New",
  "+words",
  UNCOMMITTED_MARKER,
  "diff --git a/old.txt b/renamed.txt",
  "similarity index 90%",
  "rename from old.txt",
  "rename to renamed.txt",
  "diff --git a/pic.png b/pic.png",
  "Binary files a/pic.png and b/pic.png differ",
  "",
].join("\n");

test("a unified diff splits into files with counts, status and the uncommitted divider", () => {
  const files = splitDiff(sample);
  assert.deepEqual(
    files.map((f) => [f.path, f.status, f.added, f.removed, f.uncommitted]),
    [
      ["src/a.ts", "modified", 2, 1, false],
      ["docs/new.md", "added", 2, 0, false],
      ["renamed.txt", "renamed", 0, 0, true],
      ["pic.png", "binary", 0, 0, true],
    ],
  );
  assert.equal(files[2].oldPath, "old.txt");
  // Line kinds: the +++/--- headers are meta, not adds and dels.
  assert.deepEqual(
    files[0].lines.map((l) => l.kind),
    ["meta", "meta", "meta", "hunk", "context", "del", "add", "add"],
  );
  assert.deepEqual(diffTotals(files), {
    added: 4,
    removed: 1,
    files: 4,
    uncommitted: 2,
  });
});

test("an empty diff is no files; stray text before a header stands alone; CRLF is fine", () => {
  assert.deepEqual(splitDiff(""), []);
  assert.deepEqual(splitDiff("\n\n"), []);
  const stray = splitDiff("note\r\ndiff --git a/x b/x\r\n+++ b/x\r\n+one\r\n");
  assert.equal(stray.length, 2);
  assert.equal(stray[0].path, "");
  assert.equal(stray[0].lines[0].text, "note");
  assert.equal(stray[1].added, 1);
});

test("short paths keep a folder only for common file names", () => {
  assert.equal(shortPath("README.md"), "README.md");
  assert.equal(shortPath("src/lib/diff.ts"), "diff.ts");
  assert.equal(shortPath("src/app/chat/page.tsx"), "chat/page.tsx");
  assert.equal(shortPath("crates/pulse-hub/src/lib.rs"), "src/lib.rs");
});

test("a review gutter numbers both sides from each hunk header and strips the sign", () => {
  const [file] = splitDiff(["diff --git a/x b/x", "--- a/x", "+++ b/x", "@@ -9,3 +10,4 @@ fn x()", " keep", "-gone", "+came", "+also", " end", "\\ No newline at end of file", "@@ -40 +42 @@", "+late"].join("\n"));
  const rows = numberLines(file.lines);
  assert.deepEqual(
    rows.map((l) => [l.kind, l.old, l.new, l.body]),
    [
      ["meta", null, null, "--- a/x"],
      ["meta", null, null, "+++ b/x"],
      ["hunk", null, null, "@@ -9,3 +10,4 @@ fn x()"],
      ["context", 9, 10, "keep"],
      ["del", 10, null, "gone"],
      ["add", null, 11, "came"],
      ["add", null, 12, "also"],
      ["context", 11, 13, "end"],
      ["context", null, null, "\\ No newline at end of file"],
      ["hunk", null, null, "@@ -40 +42 @@"],
      ["add", null, 42, "late"],
    ],
  );
  assert.equal(dirOf("src/lib/diff.ts"), "src/lib/");
  assert.equal(dirOf("README.md"), "");
});
