import type { CodeDeliveriesPayload, CodeDeliverySummary } from "./api";

const n = (value: unknown) => Number(value || 0).toLocaleString("en-US");
const many = (value: number, word: string) => n(value) + " " + word + (value === 1 ? "" : "s");
export type CodeDeliveryKpiTone = "neutral" | "ok" | "warn";

export function codeDeliveryIncompleteWindow(d: CodeDeliveriesPayload | undefined): boolean {
  if (!d) return false;
  return d.scope.scanned === 0 && d.deliveries.length === 0 && d.scope.has_more && d.scope.next_before !== null;
}

export function codeDeliveryWindowLabel(d: CodeDeliveriesPayload | undefined, page: number): string {
  if (!d) return "loading recent landing window";
  const window = page > 1 ? "older landing window " + page : "recent landing window";
  const scope = d.scope.project ? " for " + d.scope.project : "";
  return window + scope + " · scanned " + many(d.scope.scanned, "landing event") + " · limit " + n(d.scope.limit);
}

export function codeDeliveryKpiSub(d: CodeDeliveriesPayload | undefined, loading: boolean, error: string): string {
  if (error && !d) return "unavailable";
  if (!d) return loading ? "loading recent landings" : "not loaded";
  if (codeDeliveryIncompleteWindow(d)) return "incomplete search window · older evidence available";
  const parts = [many(d.summary.code_files, "source-file change"), many(d.scope.scanned, "landing event") + " scanned"];
  if (d.summary.unknown || d.unavailable.length) parts.push(many(Math.max(d.summary.unknown, d.unavailable.length), "unknown"));
  return parts.join(" · ");
}

export function codeDeliveryKpiState(d: CodeDeliveriesPayload | undefined, loading: boolean, error: string): { n: number | string; sub: string; tone: CodeDeliveryKpiTone } {
  if (!d) return { n: error ? "—" : "…", sub: codeDeliveryKpiSub(d, loading, error), tone: error ? "warn" : "neutral" };
  if (codeDeliveryIncompleteWindow(d)) return { n: "—", sub: (error ? "stale - refresh failed · " : "") + codeDeliveryKpiSub(d, loading, ""), tone: "warn" };
  const sub = (error ? "stale - refresh failed · " : "") + codeDeliveryKpiSub(d, loading, "");
  return { n: d.summary.code_deliveries, sub, tone: error ? "warn" : codeDeliveryCoverageTone(d.summary) };
}

export function codeDeliveryCoverageTone(summary: CodeDeliverySummary): CodeDeliveryKpiTone {
  if (summary.unknown || summary.duplicates) return "warn";
  if (summary.code_deliveries) return "ok";
  return "neutral";
}

export function codeDeliveryCoverageText(d: CodeDeliveriesPayload): string {
  if (codeDeliveryIncompleteWindow(d)) return "Incomplete search window: no landing events were scanned before the bounded scan stopped, and older evidence is available. This is not a definitive zero.";
  const s = d.summary;
  const excluded = s.docs_only + s.non_code;
  const bits = [
    many(d.scope.scanned, "landing event") + " scanned in this bounded window",
    many(s.code_files, "source-file change"),
  ];
  if (excluded) bits.push(many(s.docs_only, "docs-only landing") + " and " + many(s.non_code, "non-code landing") + " excluded");
  if (s.duplicates) bits.push(many(s.duplicates, "duplicate") + " deduped");
  if (s.unknown) bits.push(many(s.unknown, "unknown evidence row") + " not counted as zero");
  if (!s.code_deliveries && !s.unknown && d.unavailable.length === 0) bits.push("zero is based on complete evidence for this page");
  return bits.join(" · ");
}

export function shortCommit(sha: string): string {
  return sha ? sha.slice(0, 12) : "unknown commit";
}

export function codeDeliveryCommitURL(remote: string | null | undefined, sha: string): string | null {
  if (!remote || !/^[a-f0-9]{40}(?:[a-f0-9]{24})?$/i.test(sha)) return null;
  let source = remote.trim();
  if (/[\s\\]/.test(source)) return null;
  const scp = /^(?:[^@/:]+@)?(github\.com|gitlab\.com|bitbucket\.org):(.+)$/i.exec(source);
  if (scp) source = "ssh://" + scp[1] + "/" + scp[2];
  let repo: URL;
  try {
    repo = new URL(source);
  } catch (error) {
    if (error instanceof TypeError) return null;
    throw error;
  }
  if (!["https:", "http:", "ssh:", "git:"].includes(repo.protocol) || repo.port) return null;
  const parts = repo.pathname.replace(/\/+$/, "").replace(/\.git$/i, "").slice(1).split("/");
  if (parts.some((part) => !/^[a-z0-9_.-]+$/i.test(part) || /^\.+$/.test(part))) return null;
  let route: string;
  switch (repo.hostname.toLowerCase()) {
    case "github.com":
      if (parts.length !== 2) return null;
      route = "commit";
      break;
    case "gitlab.com":
      if (parts.length < 2) return null;
      route = "-/commit";
      break;
    case "bitbucket.org":
      if (parts.length !== 2) return null;
      route = "commits";
      break;
    default:
      return null;
  }
  return "https://" + repo.hostname.toLowerCase() + "/" + parts.join("/") + "/" + route + "/" + sha.toLowerCase();
}
