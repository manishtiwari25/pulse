// Formatting helpers shared by every view. Pure functions, no DOM.

export const KINDS = ["claim", "handoff", "finding", "question", "answer", "suggestion", "opinion", "vote", "amendment", "result", "notice"];

export const num = (n: unknown): string => Number(n || 0).toLocaleString("en-US");
export const plural = (n: number, word: string): string => num(n) + " " + word + (n === 1 ? "" : "s");
// "a", "a and b", "a, b and c".
export const andList = (items: string[]): string => (items.length < 3 ? items.join(" and ") : items.slice(0, -1).join(", ") + " and " + items[items.length - 1]);
export const fmtDate = (d?: string | null): string => (d || "").replace("T", " ").replace("Z", " UTC");

export function parseDate(d?: string | null): Date | null {
  if (!d) return null;
  let s = String(d).trim();
  if (/^\d{4}-\d{2}-\d{2}$/.test(s)) s += "T00:00:00Z";
  if (/^\d{4}-\d{2}-\d{2}T\d{2}$/.test(s)) s += ":00:00Z";
  if (!/Z$/.test(s) && !/[+-]\d{2}:\d{2}$/.test(s)) s += "Z";
  const t = Date.parse(s);
  return isNaN(t) ? null : new Date(t);
}

export function span(sec: number): string {
  if (sec < 90) return Math.max(1, Math.round(sec)) + "s";
  if (sec < 3600) return Math.round(sec / 60) + "m";
  if (sec < 86400) return Math.round(sec / 3600) + "h";
  if (sec < 86400 * 30) return Math.round(sec / 86400) + "d";
  return Math.round(sec / (86400 * 30)) + "mo";
}

export function rel(d?: string | null, now: number = Date.now()): string {
  const t = parseDate(d);
  if (!t) return d || "";
  const diff = (now - t.getTime()) / 1000;
  if (diff < -60) return "in " + span(-diff);
  if (diff < 45) return "just now";
  return span(diff) + " ago";
}

export function untilText(t: Date | null, now: number = Date.now()): string {
  if (!t) return "";
  const diff = (t.getTime() - now) / 1000;
  return diff < 0 ? "expired " + span(-diff) + " ago" : "in " + span(diff);
}

export function expiresAt(s: { date: string; expiry_days?: number }): Date | null {
  const d = parseDate(s.date);
  return d && s.expiry_days ? new Date(d.getTime() + s.expiry_days * 86400000) : null;
}

export function hashHue(seed: string): number {
  let h = 0;
  for (const ch of seed) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return h % 360;
}

export function initials(seed: string): string {
  const parts = seed.split(/[\s/_\-.:]+/).filter(Boolean);
  const a = parts[0] || "?";
  const b = parts[1];
  return (parts.length > 1 && b ? a[0]! + b[0]! : a.slice(0, 2)).toUpperCase();
}

export function truncate(s: string, n: number): string {
  return s.length > n ? s.slice(0, n) + "\u2026" : s;
}

export type Tone = "neutral" | "ok" | "warn" | "bad" | "info" | "accent";

export function statusTone(s?: string | null): Tone {
  const v = (s || "").toLowerCase();
  if (/^(active|eligible|open|passed|ready|done|accepted|delivered|approved|proceeded|live|working)/.test(v)) return "ok";
  if (/^(needs|stale|short|no expiry|expired|closed|pending|review|planned|provisioning|proposed|silent|idle|recent|awaiting|partially)/.test(v)) return "warn";
  if (/^(blocked|malformed|ambiguous|unchecked|failed|denied|cancelled|declined|stopped|unreachable|pruned|retired|superseded|dormant)/.test(v)) return "bad";
  if (/^(in_progress|running|leased|queued|released|drained)/.test(v)) return "info";
  return "neutral";
}

export const KIND_TONE: Record<string, Tone> = {
  claim: "info",
  handoff: "info",
  finding: "accent",
  question: "warn",
  answer: "ok",
  suggestion: "accent",
  opinion: "neutral",
  vote: "neutral",
  amendment: "warn",
  result: "ok",
  notice: "bad",
};

export function shortId(id: string): string {
  // 20260906T182448Z-fable-go-planner -> 18:24 fable-go-planner
  const m = /^(\d{8})T(\d{2})(\d{2})\d{2}Z-(.+)$/.exec(id);
  return m ? m[2] + ":" + m[3] + " " + m[4] : id;
}

export function titleCase(s: string): string {
  return s.replace(/[_-]+/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
}

export function prioLabel(p: number): string {
  if (p >= 50) return "urgent";
  if (p >= 30) return "high";
  if (p >= 10) return "normal";
  return "low";
}

export const PRIORITIES: [number, string][] = [
  [0, "low - after everything else"],
  [10, "normal"],
  [30, "high - before suggestions"],
  [50, "urgent - before decisions"],
];
