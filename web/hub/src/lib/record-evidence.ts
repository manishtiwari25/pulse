// Evidence links for a record (cinder-3): the jobs and board posts a record's
// text names are the trail an owner otherwise follows by hand with
// `pulse record show <id>` plus a board search. Pure so the node test can pin
// the extraction; the records sheet renders the result as links.

export interface EvidenceLink {
  kind: "job" | "post";
  id: string;
  href: string;
}

const JOB_RE = /\bjob-\d{8}T\d{6}Z-[0-9a-f]{6}\b/g;
// A board post id: <stamp>-<agent>[-<suffix>]. Job and run ids share the stamp
// shape but carry a `job-`/`run-` prefix, which the lookbehind excludes.
const POST_RE = /(?<![\w-])\d{8}T\d{6}Z-[a-z][a-z0-9-]*[a-z0-9]\b/g;

function flatten(v: unknown): string {
  if (v === null || v === undefined) return "";
  if (typeof v === "string") return v;
  if (Array.isArray(v)) return v.map(flatten).join("\n");
  if (typeof v === "object") return Object.values(v as Record<string, unknown>).map(flatten).join("\n");
  return String(v);
}

// Jobs write their findings under `evolution/<job id lowercased>` (runner brief).
export const jobThreadKey = (jobId: string): string => "evolution/" + jobId.toLowerCase();

// Same shapes as hrefs.thread / hrefs.post in nav.ts (nav.test.mjs pins them);
// built here so the node test can import this module without a local import.
const boardHref = (slug: string, key: "thread" | "post", value: string): string => {
  const q = new URLSearchParams({ slug, tab: "board", sub: "threads" });
  q.set(key, value);
  return "/project/?" + q.toString();
};

export function recordEvidence(slug: string, body: string, frontmatter: Record<string, unknown> = {}): EvidenceLink[] {
  const text = body + "\n" + flatten(frontmatter);
  const out: EvidenceLink[] = [];
  const seen = new Set<string>();
  const push = (kind: EvidenceLink["kind"], id: string, href: string) => {
    const key = kind + ":" + id;
    if (seen.has(key)) return;
    seen.add(key);
    out.push({ kind, id, href });
  };
  for (const m of text.match(JOB_RE) || []) push("job", m, boardHref(slug, "thread", jobThreadKey(m)));
  for (const m of text.match(POST_RE) || []) push("post", m, boardHref(slug, "post", m));
  return out;
}
