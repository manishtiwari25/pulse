// Formatting helpers shared by every view. Pure functions, no DOM.

export const KINDS = ["claim", "handoff", "finding", "question", "answer", "suggestion", "opinion", "vote", "amendment", "result", "notice"];

export const num = (n: unknown): string => Number(n || 0).toLocaleString("en-US");
export const plural = (n: number, word: string): string => num(n) + " " + word + (n === 1 ? "" : "s");
// "a", "a and b", "a, b and c".
export const andList = (items: string[]): string => (items.length < 3 ? items.join(" and ") : items.slice(0, -1).join(", ") + " and " + items[items.length - 1]);

// Bytes as a person reads them: "1.2 GB", "184 KB".
export function sizeText(n: number | undefined | null): string {
  const b = Number(n || 0);
  if (b >= 1 << 30) return (b / (1 << 30)).toFixed(1) + " GB";
  if (b >= 1 << 20) return (b / (1 << 20)).toFixed(1) + " MB";
  if (b >= 1 << 10) return Math.round(b / (1 << 10)) + " KB";
  return b + " B";
}
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

// A duration in words: "40 seconds", "9 minutes", "3 hours", "26 days", "1 month", "2 years".
export function span(sec: number): string {
  const unit = (n: number, word: string) => n + " " + word + (n === 1 ? "" : "s");
  if (sec < 60) return unit(Math.max(1, Math.round(sec)), "second");
  if (sec < 3600) return unit(Math.max(1, Math.round(sec / 60)), "minute");
  if (sec < 86400) return unit(Math.round(sec / 3600), "hour");
  if (sec < 86400 * 30) return unit(Math.round(sec / 86400), "day");
  if (sec < 86400 * 365) return unit(Math.round(sec / (86400 * 30)), "month");
  return unit(Math.round(sec / (86400 * 365)), "year");
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

export function sentence(s: string): string {
  const t = (s || "").replace(/\s+/g, " ").trim().replace(/[ \t]+([?.!])$/g, "$1");
  if (!t) return "";
  return t.charAt(0).toLocaleUpperCase("en-US") + t.slice(1);
}

export function firstSentence(s: string): string {
  const t = sentence(s);
  if (!t) return "";
  const m = /^([\s\S]+?[.!?])(?:\s|$)/.exec(t);
  return m ? m[1].trim() : t;
}

export function truncate(s: string, n: number): string {
  return s.length > n ? s.slice(0, n) + "\u2026" : s;
}

// `you` is marigold: the one tone for "waits for the owner's answer".
export type Tone = "neutral" | "ok" | "warn" | "bad" | "info" | "accent" | "you";

// A state's colour says who has to act and whether something is wrong:
// marigold when it waits for the owner, red only when it broke, amber when it
// needs a look, blue while work moves, green when it is good, grey when it is
// simply settled (including the owner's own "no").
export function statusTone(s?: string | null): Tone {
  const v = (s || "").toLowerCase();
  if (/^(awaiting|pending|review(?!ing)|needs[ _]owner|needs you|proposed|waiting for you|owner$)/.test(v)) return "you";
  if (/^(active|eligible|agreed|open|passed|ready|done|accepted|delivered|approved|proceeded|live|working|online|enabled|submitted|finished|landed|connected)/.test(v)) return "ok";
  if (/^(blocked|malformed|ambiguous|unchecked|failed|stopped|unreachable|provider_error|error)/.test(v) || /(^|_)error$/.test(v)) return "bad";
  if (/^(stale|short|no expiry|expired|silent|partially|step_limit|time_limit|format_errors|not_signed_in|lost)/.test(v)) return "warn";
  if (/^(in_progress|running|leased|queued|released|drained|provisioning|starting|cancelling|reviewing|held|resumed)/.test(v)) return "info";
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
