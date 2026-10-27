// The task sheet's Activity tab: how the flat lines of GET /tasks/{id}/activity
// become a chat-like feed. Lines of one run gather into a chapter under the
// worker's name; everything else (the owner's words, the lead's messages,
// what PULSE landed) stands on its own between chapters, in time order.
// Pure: no React and no DOM, so task-activity.test.mjs drives it.

import type { ActivityItem, ActivityRun, TaskActivity } from "./api";

export type ActivityBlock =
  | { kind: "line"; item: ActivityItem }
  // `first`: the chapter holds the run's start, so it carries the header; a
  // later chapter of the same run (a message came in between) says "continued".
  | { kind: "run"; run: ActivityRun; items: ActivityItem[]; first: boolean };

export function groupActivity(a: Pick<TaskActivity, "items" | "runs">): ActivityBlock[] {
  const runs = new Map(a.runs.map((r) => [r.id, r]));
  const out: ActivityBlock[] = [];
  const seen = new Set<string>();
  for (const item of a.items) {
    const run = item.run_id ? runs.get(item.run_id) : undefined;
    if (!run) {
      out.push({ kind: "line", item });
      continue;
    }
    const last = out[out.length - 1];
    if (last && last.kind === "run" && last.run.id === run.id) {
      last.items.push(item);
      continue;
    }
    out.push({ kind: "run", run, items: [item], first: !seen.has(run.id) });
    seen.add(run.id);
  }
  return out;
}

export const isOwnerName = (who: string): boolean => who === "human" || who === "owner";

export const whoWord = (who: string): string => (isOwnerName(who) ? "You" : who || "PULSE");

// How a run ended, as a person would say it.
export function runOutcome(status: string): { text: string; tone: "ok" | "warn" | "bad" | "neutral" | "live" } {
  switch (status) {
    case "done":
      return { text: "finished", tone: "ok" };
    case "running":
      return { text: "is working", tone: "live" };
    case "held":
      return { text: "paused for the runner's restart", tone: "neutral" };
    case "failed":
    case "error":
      return { text: "stopped with an error", tone: "bad" };
    case "timeout":
      return { text: "ran out of time", tone: "warn" };
    case "lost":
      return { text: "was lost", tone: "bad" };
    case "stale":
      return { text: "went quiet", tone: "warn" };
    case "killed":
    case "cancelled":
      return { text: "was stopped", tone: "warn" };
    case "dry-run":
      return { text: "ran as a dry run", tone: "neutral" };
    default:
      return { text: status ? status.replace(/[_-]+/g, " ") : "ended", tone: "neutral" };
  }
}

const unit = (n: number, word: string) => n + " " + word + (n === 1 ? "" : "s");

// "4 minutes" between two instants (or until now for a live run).
export function workedFor(run: Pick<ActivityRun, "started_at" | "finished_at">, nowMs: number): string {
  const start = Date.parse(run.started_at);
  const end = run.finished_at ? Date.parse(run.finished_at) : nowMs;
  if (!Number.isFinite(start) || !Number.isFinite(end)) return "";
  const sec = Math.max(0, (end - start) / 1000);
  if (sec < 60) return unit(Math.max(1, Math.round(sec)), "second");
  if (sec < 3600) return unit(Math.max(1, Math.round(sec / 60)), "minute");
  return unit(Math.round((sec / 3600) * 10) / 10, "hour");
}

// The one line a folded chapter shows: "cinder worked 4 minutes: 3 commands,
// 1 file changed - finished."
export function runSummaryLine(run: ActivityRun, nowMs: number): string {
  const parts: string[] = [];
  if (run.commands) parts.push(unit(run.commands, "command"));
  if (run.edits) parts.push(run.edits === 1 ? "1 file changed" : run.edits + " files changed");
  if (!parts.length && run.calls) parts.push(unit(run.calls, "model call"));
  const did = parts.length ? ": " + parts.join(", ") : "";
  const span = workedFor(run, nowMs);
  const outcome = runOutcome(run.status);
  if (run.status === "running") return run.agent + " has been on it " + (span || "a moment") + did + ".";
  return run.agent + " worked " + (span || "briefly") + did + " \u2014 " + outcome.text + ".";
}

// What a command's outcome badge says.
export function exitBadge(code: number | null | undefined): { text: string; bad: boolean } | null {
  if (code === null || code === undefined) return null;
  if (code === 0) return { text: "ok", bad: false };
  return { text: "exit " + code, bad: true };
}

export function durationText(ms: number | undefined): string {
  if (!ms || ms < 1000) return "";
  if (ms < 60_000) return (Math.round(ms / 100) / 10).toString().replace(/\.0$/, "") + "s";
  return Math.round(ms / 60_000) + " min";
}

// The sentence a system line says for a task, job, landing or post item.
export function systemLine(item: ActivityItem): string {
  const who = whoWord(item.who);
  switch (item.kind) {
    case "task":
      switch (item.what) {
        case "revised":
          return who + " changed the task" + (item.title ? ": " + item.title : "") + ".";
        case "status":
          return who + " moved it " + (item.title ? "from " + statusPlain(item.title) + " " : "") + "to " + statusPlain(item.status || "") + ".";
        case "lead":
          return (item.title || "An agent") + " became the lead agent for this task.";
        case "cancelled":
          return who + " cancelled the task" + (item.text ? ": " + item.text : "") + ".";
        case "followup":
          return "PULSE queued a follow-up" + (item.title ? " because the step " + item.title : "") + ".";
        case "nudged":
          return "PULSE nudged the lead" + (item.title ? " (" + item.title + ")" : "") + ".";
        default:
          return who + " " + item.what + ".";
      }
    case "job":
      return (isOwnerName(item.who) ? "You" : item.who === "daemon" ? "PULSE" : item.who || "The lead") + " added a step: " + (item.title || item.job_id || "a step") + ".";
    case "land":
      switch (item.what) {
        case "landed":
          return "PULSE landed the work on the main branch" + (item.status ? " (" + item.status.slice(0, 12) + ")" : "") + ".";
        case "pushed":
          return "PULSE pushed the main branch to the remote.";
        case "held":
          return "PULSE held the landing" + (item.text ? ": " + item.text : "") + ".";
        case "deferred":
          return "PULSE put the landing aside for now" + (item.text ? ": " + item.text : "") + ".";
        case "conflict":
          return "The landing hit a conflict" + (item.text ? ": " + item.text : "") + ".";
        case "resumed":
          return "PULSE resumed the landings.";
        default:
          return "Landing: " + item.what.replace(/[_-]+/g, " ") + (item.text ? " \u2014 " + item.text : "") + ".";
      }
    case "run":
      if (item.what === "started") return (item.who || "An agent") + " started on \u201c" + (item.title || "a step") + "\u201d.";
      return (item.who || "The agent") + " " + runOutcome(item.status || "").text + ".";
    default:
      return who + ": " + (item.title || item.text || item.what);
  }
}

const statusPlain = (s: string) => s.replace(/[_-]+/g, " ");

// Which lines a folded chapter still shows: none - the summary line stands
// for them. Which it counts: every line but the start and end markers.
export function chapterLineCount(items: ActivityItem[]): number {
  return items.filter((i) => i.kind !== "run").length;
}

// The last moment a chapter moved: its newest line.
export function lastAt(items: ActivityItem[]): string {
  let at = "";
  for (const i of items) if (i.at > at) at = i.at;
  return at;
}

// Live-stream events the feed must act on itself: harness steps carry no
// task id, and the hub's refresh rules leave them alone on purpose. A step
// of one of the task's jobs (or sessions) means the feed moved.
export function eventTouchesTask(e: { action: string; subject: string; detail: unknown }, jobIds: Set<string>, sessionIds: Set<string>): boolean {
  if (!e.action.startsWith("harness.")) return false;
  if (sessionIds.has(e.subject)) return true;
  const d = e.detail && typeof e.detail === "object" ? (e.detail as Record<string, unknown>) : null;
  const job = typeof d?.job === "string" ? d.job : "";
  return !!job && jobIds.has(job);
}
