// The words and the order of the Chat page's one list. Every entry is a
// conversation: a plain one, one with sub-conversations under it, or a task
// - the owner's conversation with the lead agent, with one agent session per
// step the lead planned under it (`tasks` in GET /api/v1/chat). Kept apart
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
  if (t.needs_you) return { word: "Asked you", tone: "wait", pip: "waiting", dot: "bg-warn" };
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

// A task's section, on the same scale as a conversation's: working while
// the lead waits on you or an agent is at a step, open while it is new or
// planned, finished once done or cancelled (a task is never archived).
export function taskGroup(t: ChatTask): ListGroup {
  if (taskFinished(t)) return "finished";
  const tone = taskMood(t).tone;
  return tone === "wait" || tone === "run" ? "working" : "open";
}

// A step's state: the job's word, except while its agent's session runs.
export function sessionWords(s: ChatTaskSession): { word: string; tone: Tone } {
  if (s.status === "running") return { word: "Working", tone: "run" };
  switch (s.job_status) {
    case "queued":
      return { word: "Waiting", tone: "neutral" };
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

// ----- sub-conversations ----------------------------------------------------

// The list's sections: a conversation is working while its agent runs or
// waits on you, open while it is idle between turns, finished once the
// agent or the owner closed it (a message reopens it), and archived when
// it was put away.
export type ListGroup = "working" | "open" | "finished" | "archived";

export const GROUP_ORDER: ListGroup[] = ["working", "open", "finished", "archived"];

export interface Grouped {
  id: string;
  status: string;
  parent?: string;
  created_at?: string;
  live?: { status: string } | null;
}

export function groupOf(c: Grouped): ListGroup {
  const status = c.live?.status || c.status;
  if (status === "running" || status === "waiting") return "working";
  if (status === "closed") return "finished";
  if (status === "archived") return "archived";
  return "open";
}

// A conversation with its sub-conversations under it: the ones its agent
// (or the owner, from its page) started. A sub-conversation whose parent is
// not in the list (put away while it carries on, or outside the latest
// rows) stands on its own.
export interface Family<T extends Grouped> {
  c: T;
  children: T[];
}

export function families<T extends Grouped>(rows: T[]): Family<T>[] {
  const ids = new Set(rows.map((c) => c.id));
  const top = rows.filter((c) => !c.parent || !ids.has(c.parent) || c.parent === c.id);
  const byParent = new Map<string, T[]>();
  for (const c of rows) {
    if (!c.parent || c.parent === c.id || !ids.has(c.parent)) continue;
    const list = byParent.get(c.parent) ?? [];
    list.push(c);
    byParent.set(c.parent, list);
  }
  // Children in the order they were started, oldest first: the way the
  // parent's transcript names them.
  return top.map((c) => ({
    c,
    children: (byParent.get(c.id) ?? []).sort((a, b) => (a.created_at ?? "").localeCompare(b.created_at ?? "")),
  }));
}

// A family's section is its most pressing member's: a finished parent with
// a sub-conversation still working is listed under Working.
export function familyGroup<T extends Grouped>(f: Family<T>): ListGroup {
  let at = GROUP_ORDER.indexOf(groupOf(f.c));
  for (const child of f.children) at = Math.min(at, GROUP_ORDER.indexOf(groupOf(child)));
  return GROUP_ORDER[at];
}

// ----- the one list ---------------------------------------------------------

// An entry of the list: a conversation with its sub-conversations, or a
// task with its steps. `at` is its latest movement - a sub-conversation's
// reply counts for its parent - so the list orders by what moved last.
export type ListEntry<T extends Grouped> = { kind: "conversation"; family: Family<T>; at: string } | { kind: "task"; task: ChatTask; at: string };

interface Moved {
  created_at?: string;
  last_at?: string;
}

function movedAt(c: Moved): string {
  return c.last_at || c.created_at || "";
}

export function listEntries<T extends Grouped & Moved>(rows: T[], tasks: ChatTask[]): ListEntry<T>[] {
  const entries: ListEntry<T>[] = families(rows).map((family) => ({
    kind: "conversation",
    family,
    at: [family.c, ...family.children].map(movedAt).reduce((a, b) => (a > b ? a : b), ""),
  }));
  for (const task of tasks) entries.push({ kind: "task", task, at: movedAt(task) });
  return entries.sort((a, b) => b.at.localeCompare(a.at));
}

export function entryGroup<T extends Grouped>(e: ListEntry<T>): ListGroup {
  return e.kind === "task" ? taskGroup(e.task) : familyGroup(e.family);
}

export function entryId<T extends Grouped>(e: ListEntry<T>): string {
  return e.kind === "task" ? e.task.id : e.family.c.id;
}

// What a report note from a sub-conversation says it did, as the verb that
// follows its title: "Docs check · finished".
export function reportVerb(kind: string): string {
  switch (kind) {
    case "finished":
      return "finished";
    case "replied":
      return "replied";
    case "asks":
      return "asks you";
    case "blocked":
      return "is blocked";
    case "stopped":
      return "stopped";
    case "ended":
      return "ended its session";
  }
  return "reports";
}
