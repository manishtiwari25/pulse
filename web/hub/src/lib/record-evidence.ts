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

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

// "20261004T221802Z" -> "4 Oct 2026, 22:18 UTC".
function stampWords(stamp: string): string {
  const m = /^(\d{4})(\d{2})(\d{2})T(\d{2})(\d{2})\d{2}Z$/.exec(stamp);
  if (!m) return "";
  const month = MONTHS[Number(m[2]) - 1];
  if (!month) return "";
  return Number(m[3]) + " " + month + " " + m[1] + ", " + m[4] + ":" + m[5] + " UTC";
}

// How the sheet names one evidence link in plain words; the id stays in the
// link's tooltip. A post's agent is the id after its stamp, without the
// long numeric suffix some agents add.
export function evidenceLabel(e: EvidenceLink): { label: string; when: string } {
  const m = /^(?:job-)?(\d{8}T\d{6}Z)-(.+)$/.exec(e.id);
  const when = m ? stampWords(m[1]) : "";
  if (e.kind === "job") return { label: "What the job found, on the board", when };
  const agent = m ? m[2].replace(/-\d{6,}$/, "") : "";
  return { label: agent ? "Board post by " + agent : "Board post", when };
}
