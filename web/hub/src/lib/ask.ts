// Ask (the console's quick answer box): the parts that need no browser.
// The panel in components/ask keeps one conversation per project in this
// tab's sessionStorage; the model only ever sees the last few turns.

import type { AskAnswer, AskSource, AskTurn } from "./api";

// Turns kept per project in this browser tab.
export const ASK_HISTORY_CAP = 20;
// Turns sent back with the next question so follow-ups make sense.
export const ASK_SEND_TURNS = 6;
// The server refuses longer questions (pulse_ask::MAX_QUESTION).
export const ASK_MAX_QUESTION = 2000;

export const ASK_STARTERS = ["What waits for me?", "What is blocked, and on whom?", "What changed in the last day?", "What are the agents working on right now?"];

export type AskMessage =
  | { role: "user"; content: string; at: string }
  | {
      role: "assistant";
      content: string;
      at: string;
      sources: AskSource[];
      facts_only: boolean;
      note: string;
      model: string;
      provider: string;
      elapsed_ms: number;
    };

export function askStorageKey(project: string): string {
  return "pulse.ask." + (project.trim() || "all");
}

export function trimHistory<T>(turns: T[], cap: number = ASK_HISTORY_CAP): T[] {
  return turns.length > cap ? turns.slice(turns.length - cap) : turns;
}

// What the model sees again: the last turns, words only.
export function historyForRequest(messages: AskMessage[], turns: number = ASK_SEND_TURNS): AskTurn[] {
  return trimHistory(messages, turns)
    .filter((m) => m.content.trim())
    .map((m) => ({ role: m.role, content: m.content }));
}

export function answerMessage(a: AskAnswer, at: string): AskMessage {
  return {
    role: "assistant",
    content: a.answer,
    at,
    sources: Array.isArray(a.sources) ? a.sources : [],
    facts_only: !!a.facts_only,
    note: a.note || "",
    model: a.model || "",
    provider: a.provider || "",
    elapsed_ms: a.elapsed_ms || 0,
  };
}

// A reason for a facts-only answer, in the owner's words, with the way out.
export function factsOnlyHint(note: string): string {
  const n = note.trim();
  if (!n) return "No model answered, so these are the matching facts.";
  if (n === "not signed in") return "No model is signed in, so these are the matching facts. Sign in on the Runs page for a worded answer.";
  if (n.startsWith("the model did not answer")) return "The model was too slow (" + n + "), so these are the matching facts. Ask again, or pick a faster Ask model in Settings.";
  if (n.startsWith("the model provider")) return "The model provider did not open (" + n + "), so these are the matching facts. Check the provider on the Runs page.";
  return "No model answered (" + n + "), so these are the matching facts.";
}

export function elapsedWord(ms: number): string {
  if (!ms || ms < 0) return "";
  return ms < 950 ? Math.round(ms) + " ms" : (ms / 1000).toFixed(1) + " s";
}

type StorageLike = Pick<Storage, "getItem" | "setItem" | "removeItem">;

export function readHistory(storage: StorageLike | null | undefined, project: string): AskMessage[] {
  if (!storage) return [];
  try {
    const raw = storage.getItem(askStorageKey(project));
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return trimHistory(parsed.filter(isMessage));
  } catch {
    return [];
  }
}

export function writeHistory(storage: StorageLike | null | undefined, project: string, messages: AskMessage[]): void {
  if (!storage) return;
  try {
    const key = askStorageKey(project);
    if (!messages.length) storage.removeItem(key);
    else storage.setItem(key, JSON.stringify(trimHistory(messages)));
  } catch {
    // A full or blocked storage loses the history, never the answer on screen.
  }
}

function isMessage(v: unknown): v is AskMessage {
  if (!v || typeof v !== "object") return false;
  const m = v as Record<string, unknown>;
  if (typeof m.content !== "string" || typeof m.at !== "string") return false;
  if (m.role === "user") return true;
  return m.role === "assistant" && Array.isArray(m.sources);
}
