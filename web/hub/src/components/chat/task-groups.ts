// The words the Chat page uses for a task and its steps. A task is a group of
// conversations: the owner's thread with the lead agent and one agent
// session per step the lead planned (`tasks` in GET /api/v1/chat). Kept apart
// from the page so the rules can be tested without a browser.

import type { ChatTask, ChatTaskSession } from "@/lib/api";
import type { PipMood } from "@/components/pulse/mascots";

export type Tone = "run" | "wait" | "ok" | "neutral" | "bad";

export interface Mood {
  word: string;
  tone: Tone;
  pip: PipMood;
  dot: string;
}

// A task's state in the list's words, on the same scale as a conversation's:
// the owner's turn first, then what the agents are doing.
export function taskMood(t: ChatTask): Mood {
  if (t.needs_you) return { word: "Needs your reply", tone: "wait", pip: "waiting", dot: "bg-warn" };
  if (t.status === "done") return { word: "Done", tone: "neutral", pip: "happy", dot: "bg-ok/50" };
  if (t.status === "cancelled") return { word: "Cancelled", tone: "neutral", pip: "asleep", dot: "bg-muted-foreground/40" };
  if (t.sessions.some((s) => s.status === "running" || s.job_status === "leased")) return { word: "Working", tone: "run", pip: "busy", dot: "bg-primary" };
  if (t.status === "open") return { word: t.lead ? "Waiting for the lead agent" : "New", tone: "ok", pip: "curious", dot: "bg-ok" };
  if (t.status === "planned") return { word: "Planned", tone: "ok", pip: "curious", dot: "bg-ok" };
  return { word: "In progress", tone: "ok", pip: "happy", dot: "bg-ok" };
}

export function taskFinished(t: ChatTask): boolean {
  return t.status === "done" || t.status === "cancelled";
}

// A step's state: the job's word, except while its agent's session runs.
export function sessionWords(s: ChatTaskSession): { word: string; tone: Tone } {
  if (s.status === "running") return { word: "Working", tone: "run" };
  switch (s.job_status) {
    case "queued":
      return { word: "Waiting for a free agent", tone: "neutral" };
    case "blocked":
      return { word: "Blocked", tone: "wait" };
    case "leased":
      return { word: "Starting", tone: "run" };
    case "done":
      return { word: "Done", tone: "ok" };
    case "failed":
      return { word: "Failed", tone: "bad" };
    case "cancelled":
      return { word: "Cancelled", tone: "neutral" };
  }
  // A state this build does not know yet: shown as words, not an identifier.
  const spaced = (s.status || s.job_status).trim().replace(/[_-]+/g, " ");
  return { word: spaced.charAt(0).toUpperCase() + spaced.slice(1), tone: "neutral" };
}

// The task's name from what the owner typed: its first line without list or
// heading marks, cut at a word when it runs long. The whole text is the
// brief, so nothing is lost.
export const TASK_TITLE_MAX = 120;

export function taskTitleOf(text: string): string {
  const first =
    text
      .split("\n")
      .map((l) => l.replace(/^(?:\s*(?:[#>*-]+|\d+[.)]))*\s*/, "").trim())
      .find((l) => l.length > 0) || "";
  if (first.length <= TASK_TITLE_MAX) return first;
  const cut = first.slice(0, TASK_TITLE_MAX);
  const at = cut.lastIndexOf(" ");
  return (at > TASK_TITLE_MAX / 2 ? cut.slice(0, at) : cut).replace(/[\s,;:.!?-]+$/, "") + "\u2026";
}
