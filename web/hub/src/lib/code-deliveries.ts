import type { CodeDeliveriesPayload, CodeDeliverySummary } from "./api";

const n = (value: unknown) => Number(value || 0).toLocaleString("en-US");
const many = (value: number, word: string) => n(value) + " " + word + (value === 1 ? "" : "s");
export type CodeDeliveryKpiTone = "neutral" | "ok" | "warn";

export function codeDeliveryIncompleteWindow(d: CodeDeliveriesPayload | undefined): boolean {
  if (!d) return false;
  return d.scope.scanned === 0 && d.deliveries.length === 0 && d.scope.has_more && d.scope.next_before !== null;
}

// The words for one page of results: which merges it covers and how many were checked.
export function codeDeliveryWindowLabel(d: CodeDeliveriesPayload | undefined, page: number): string {
  if (!d) return "Loading the latest merges";
  const window = page > 1 ? "Older merges, page " + page : "Latest merges";
  const scope = d.scope.project ? " in " + d.scope.project : "";
  return window + scope + ": " + many(d.scope.scanned, "merge") + " checked, up to " + n(d.scope.limit) + " at a time";
}

// The line under the "Code delivered" number on Home and the project page.
export function codeDeliveryKpiSub(d: CodeDeliveriesPayload | undefined, loading: boolean, error: string): string {
  if (error && !d) return "Couldn't load";
  if (!d) return loading ? "Loading the latest merges" : "Not loaded yet";
  if (codeDeliveryIncompleteWindow(d)) return "The search stopped early; older merges are left to check";
  let line = many(d.summary.code_files, "source file") + " changed in " + many(d.scope.scanned, "merge") + " checked";
  const unknown = Math.max(d.summary.unknown, d.unavailable.length);
  if (unknown) line += "; " + n(unknown) + " couldn't be checked";
  return line;
}

const STALE = "Not refreshed, showing earlier numbers. ";

export function codeDeliveryKpiState(d: CodeDeliveriesPayload | undefined, loading: boolean, error: string): { n: number | string; sub: string; tone: CodeDeliveryKpiTone } {
  if (!d) return { n: error ? "—" : "…", sub: codeDeliveryKpiSub(d, loading, error), tone: error ? "warn" : "neutral" };
  if (codeDeliveryIncompleteWindow(d)) return { n: "—", sub: (error ? STALE : "") + codeDeliveryKpiSub(d, loading, ""), tone: "warn" };
  const sub = (error ? STALE : "") + codeDeliveryKpiSub(d, loading, "");
  return { n: d.summary.code_deliveries, sub, tone: error ? "warn" : codeDeliveryCoverageTone(d.summary) };
}

export function codeDeliveryCoverageTone(summary: CodeDeliverySummary): CodeDeliveryKpiTone {
  if (summary.unknown || summary.duplicates) return "warn";
  if (summary.code_deliveries) return "ok";
  return "neutral";
}

// How sure the count is, as plain sentences: what was checked, what was left
// out and why, and whether a zero is a real zero.
export function codeDeliveryCoverageText(d: CodeDeliveriesPayload): string {
  if (codeDeliveryIncompleteWindow(d)) return "The search stopped before it reached any merges, and there is older history left to check. This is not a real zero.";
  const s = d.summary;
  const bits = ["Checked " + many(d.scope.scanned, "recent merge") + "."];
  if (s.docs_only || s.non_code) {
    const left = [s.docs_only ? many(s.docs_only, "merge") + " that only changed docs" : "", s.non_code ? many(s.non_code, "merge") + " with no code in it" : ""].filter(Boolean);
    bits.push("Left out " + left.join(" and ") + ".");
  }
  if (s.duplicates) bits.push("Counted " + many(s.duplicates, "repeated merge") + " once.");
  if (s.unknown) bits.push(many(s.unknown, "merge") + " couldn't be checked, so " + (s.unknown === 1 ? "it isn't" : "they aren't") + " counted, not even as zero.");
  if (!s.code_deliveries && !s.unknown && d.unavailable.length === 0) bits.push("Every merge here was checked, so zero really means none.");
  return bits.join(" ");
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
