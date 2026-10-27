// What the task chat says under the owner's latest message: has the lead
// picked it up? Read from the message (when it was sent, which agents have
// read it), what came after it, and where the project's lead is (the API's
// `lead_work`). The daemon marks a message read by the lead the moment it
// queues the lead's sitting that holds it, so "read by the lead" plus the
// sitting's state is the whole story: in line, reading, seen, answered.
// Pure: no React and no DOM, so task-message-status.test.mjs drives it.

export interface LeadWork {
  state: "idle" | "queued" | "running" | string;
  job?: string;
  title?: string;
  agent?: string;
  since?: string;
  read_at?: string;
}

export interface TaskStatusMessage {
  author: string;
  at: string;
  read_by?: string[];
}

export type TaskMessageState =
  // An agent wrote after it: the reply is the status.
  | { kind: "answered" }
  // The lead's sitting holds the message and is running.
  | { kind: "reading"; lead: string; since: string }
  // The lead's sitting holds the message and waits for a slot.
  | { kind: "in_line"; lead: string }
  // The lead sat with it and wrote nothing here (an answer may have gone
  // elsewhere, or none was due).
  | { kind: "seen"; lead: string; at: string }
  // Nobody holds it yet. `busy`: the lead is in another sitting and takes it
  // next; `stale`: it has waited longer than the daemon's own tick explains.
  | { kind: "sent"; lead: string; stale: boolean; busy: boolean };

// How long an unpicked message may sit before the line says so: the daemon
// wakes on the message and ticks every 15 seconds besides.
export const STALE_AFTER_MS = 2 * 60 * 1000;

export const isOwner = (author: string): boolean => author === "human" || author === "owner";

export function taskMessageState(m: TaskStatusMessage, later: TaskStatusMessage[], lead: string | undefined, work: LeadWork | null | undefined, nowMs: number): TaskMessageState {
  if (later.some((x) => !isOwner(x.author))) return { kind: "answered" };
  const state = work?.state ?? "idle";
  const who = lead || work?.agent || "the lead agent";
  const readByLead = !!lead && (m.read_by ?? []).includes(lead);
  // A sitting queued after the message holds it: the daemon gathers every
  // unread message when it queues one.
  const sittingHoldsIt = (state === "queued" || state === "running") && !!work?.since && work.since >= m.at;
  if (readByLead || sittingHoldsIt) {
    if (state === "running") return { kind: "reading", lead: who, since: work?.since || m.at };
    if (state === "queued") return { kind: "in_line", lead: who };
    return { kind: "seen", lead: who, at: work?.read_at && work.read_at >= m.at ? work.read_at : m.at };
  }
  if (work?.read_at && work.read_at >= m.at) return { kind: "seen", lead: who, at: work.read_at };
  const sentMs = Date.parse(m.at);
  const stale = Number.isFinite(sentMs) && nowMs - sentMs > STALE_AFTER_MS;
  return { kind: "sent", lead: who, stale, busy: state === "running" || state === "queued" };
}

// The line itself, for the chat and for the tests: one short sentence.
export function taskMessageLine(s: TaskMessageState): string {
  switch (s.kind) {
    case "answered":
      return "";
    case "reading":
      return s.lead + " is reading it…";
    case "in_line":
      return "Sent · " + s.lead + " reads it next";
    case "seen":
      return "Seen by " + s.lead;
    case "sent":
      if (s.stale) return "Sent · nobody has picked it up yet";
      return s.busy ? "Sent · " + s.lead + " is in another sitting and reads it next" : "Sent";
  }
}
