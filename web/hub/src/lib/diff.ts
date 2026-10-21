// A unified diff split into files, for a review that reads one file at a
// time: the path, how many lines came and went, and the lines themselves
// tagged by what they are. The hub's `/chat/ID/diff` joins the committed
// diff and the uncommitted one with a `# not committed yet` line; that
// line becomes a divider the view can label.

export type DiffLineKind = "add" | "del" | "hunk" | "meta" | "context";

export interface DiffLine {
  kind: DiffLineKind;
  text: string;
}

export interface DiffFile {
  // The new path (the old one when the file was deleted).
  path: string;
  oldPath: string;
  status: "added" | "deleted" | "renamed" | "modified" | "binary";
  added: number;
  removed: number;
  // True for every file after the `# not committed yet` divider.
  uncommitted: boolean;
  lines: DiffLine[];
}

export const UNCOMMITTED_MARKER = "# not committed yet";

function pathFromHeader(line: string): { oldPath: string; path: string } | null {
  // `diff --git a/x b/y`; paths with spaces keep the a/ b/ split reliable
  // only when the two halves mirror, which git guarantees for the plain form.
  const m = /^diff --git a\/(.+) b\/(.+)$/.exec(line);
  if (!m) return null;
  return { oldPath: m[1], path: m[2] };
}

function lineKind(line: string): DiffLineKind {
  if (line.startsWith("+++") || line.startsWith("---")) return "meta";
  if (line.startsWith("@@")) return "hunk";
  if (line.startsWith("+")) return "add";
  if (line.startsWith("-")) return "del";
  if (/^(index |old mode|new mode|deleted file mode|new file mode|similarity index|rename from|rename to|copy from|copy to|Binary files|GIT binary patch)/.test(line)) return "meta";
  return "context";
}

export function splitDiff(text: string): DiffFile[] {
  const files: DiffFile[] = [];
  let cur: DiffFile | null = null;
  let uncommitted = false;
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  if (lines.length && lines[lines.length - 1] === "") lines.pop();
  for (const line of lines) {
    if (line.trim() === UNCOMMITTED_MARKER) {
      uncommitted = true;
      cur = null;
      continue;
    }
    const head = pathFromHeader(line);
    if (head) {
      cur = {
        path: head.path,
        oldPath: head.oldPath,
        status: head.path === head.oldPath ? "modified" : "renamed",
        added: 0,
        removed: 0,
        uncommitted,
        lines: [],
      };
      files.push(cur);
      continue;
    }
    if (!cur) {
      // Text before the first header (a `--stat` tail, a note) stands alone.
      if (!line.trim()) continue;
      cur = {
        path: "",
        oldPath: "",
        status: "modified",
        added: 0,
        removed: 0,
        uncommitted,
        lines: [],
      };
      files.push(cur);
    }
    const kind = lineKind(line);
    if (kind === "add") cur.added += 1;
    else if (kind === "del") cur.removed += 1;
    if (line.startsWith("new file mode")) cur.status = "added";
    else if (line.startsWith("deleted file mode")) cur.status = "deleted";
    else if (line.startsWith("Binary files") || line.startsWith("GIT binary patch")) cur.status = "binary";
    cur.lines.push({ kind, text: line });
  }
  return files;
}

export function diffTotals(files: DiffFile[]): {
  added: number;
  removed: number;
  files: number;
  uncommitted: number;
} {
  return files.reduce(
    (t, f) => ({
      added: t.added + f.added,
      removed: t.removed + f.removed,
      files: t.files + 1,
      uncommitted: t.uncommitted + (f.uncommitted ? 1 : 0),
    }),
    { added: 0, removed: 0, files: 0, uncommitted: 0 },
  );
}

// A short name for a path: the file, with one folder of context when the
// file name alone is common (index.ts, mod.rs, README.md).
export function shortPath(path: string): string {
  const parts = path.split("/");
  if (parts.length < 2) return path;
  const file = parts[parts.length - 1];
  const plain = /^(index|mod|lib|main|readme|page|layout|route)\.[a-z]+$/i.test(file);
  return plain ? parts.slice(-2).join("/") : file;
}

// The folder a path sits in, with a trailing slash; empty at the root.
export function dirOf(path: string): string {
  const i = path.lastIndexOf("/");
  return i < 0 ? "" : path.slice(0, i + 1);
}

export interface NumberedLine extends DiffLine {
  // Line numbers in the old and the new file, as a review gutter shows
  // them; null where a line has none (headers, hunk markers, the side a
  // change is not on, git's "No newline at end of file" note).
  old: number | null;
  new: number | null;
  // The line without its leading sign (+, -, space); headers and hunk
  // markers keep their whole text.
  body: string;
}

export function numberLines(lines: DiffLine[]): NumberedLine[] {
  let o = 0;
  let n = 0;
  return lines.map((l) => {
    if (l.kind === "hunk") {
      const m = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(l.text);
      if (m) {
        o = Number(m[1]);
        n = Number(m[2]);
      }
      return { ...l, old: null, new: null, body: l.text };
    }
    if (l.kind === "meta") return { ...l, old: null, new: null, body: l.text };
    if (l.text.startsWith("\\")) return { ...l, old: null, new: null, body: l.text };
    const body = l.text.slice(1);
    if (l.kind === "add") return { ...l, old: null, new: n++, body };
    if (l.kind === "del") return { ...l, old: o++, new: null, body };
    return { ...l, old: o++, new: n++, body };
  });
}
