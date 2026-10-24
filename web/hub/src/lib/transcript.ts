// What a turn did, in order - the pure folds behind the chat transcript.
// Plain TypeScript with no React so the node test suite can check the logic
// (node --test strips the types and imports this file directly). `Beat`,
// `ActionItem` and the helpers below move out of components/chat/chat.tsx;
// nothing about what the transcript shows changes here.

import type { HarnessStep } from "@/lib/api";

// ----- what it did, in order -------------------------------------------------
//
// A turn reads the way the Copilot app shows one: the words the agent wrote,
// then the tools it ran (one line each, the output folded under), then more
// words, in the order they happened. An observation is the result of the
// action before it and rides along with that row.

const ACTION_KINDS = new Set(["command", "read", "grep", "glob", "edit", "write", "mcp", "skill", "fetch", "process", "memory", "search", "subagent", "hook"]);

interface ActionItem {
  step: HarnessStep;
  result: HarnessStep | null;
  /** Seconds the model took before this (wordless) reply: a "Thought for" row. */
  thought?: number;
}

type Beat = { kind: "said"; step: HarnessStep } | { kind: "plan"; step: HarnessStep } | { kind: "actions"; items: ActionItem[] };

// How long the model took over a reply: the latency the harness measured,
// or - for steps recorded before it kept that - the gap since the step
// before. Under a second is not worth a row.
function thoughtSeconds(s: HarnessStep, prev: HarnessStep | null): number {
  let sec = s.duration_ms > 0 ? s.duration_ms / 1000 : 0;
  if (!sec && prev) {
    const a = Date.parse(prev.at);
    const b = Date.parse(s.at);
    if (Number.isFinite(a) && Number.isFinite(b) && b > a) sec = (b - a) / 1000;
  }
  return sec >= 1 ? sec : 0;
}

function beatsOf(steps: HarnessStep[]): Beat[] {
  const out: Beat[] = [];
  let run: ActionItem[] | null = null;
  let prev: HarnessStep | null = null;
  for (const s of steps) {
    const before = prev;
    prev = s;
    if (s.kind === "assistant") {
      // The time the model spent before answering rides in the run of tool
      // calls as its own row, the way the Copilot app shows "Thought for 3s".
      const sec = thoughtSeconds(s, before);
      if (sec) {
        if (!run) {
          run = [];
          out.push({ kind: "actions", items: run });
        }
        run.push({ step: s, result: null, thought: sec });
      }
      // A segment with no words (the model went straight to a tool) has
      // nothing more to show and does not break the run of tool calls.
      if (!s.content.trim()) continue;
      run = null;
      out.push({ kind: "said", step: s });
      continue;
    }
    if (s.kind === "plan") {
      run = null;
      out.push({ kind: "plan", step: s });
      continue;
    }
    if (s.kind === "observation") {
      // The result of the latest action at the same depth still waiting for
      // one (a helper's steps sit between a subagent call and its report).
      if (run) {
        for (let j = run.length - 1; j >= 0; j--) {
          const it = run[j];
          if (!it.result && !it.thought && ACTION_KINDS.has(it.step.kind) && (it.step.depth ?? 0) === (s.depth ?? 0)) {
            it.result = s;
            break;
          }
        }
      }
      continue;
    }
    if (!run) {
      run = [];
      out.push({ kind: "actions", items: run });
    }
    run.push({ step: s, result: null });
  }
  return out;
}

// The streamed text of the running turn holds every reply segment so far,
// glued together; the segments already recorded as steps show in their
// place, so only what is still being written stays in the live bubble.
function streamTail(stream: string, steps: HarnessStep[]): string {
  let rest = stream;
  for (const s of steps) {
    if (s.kind !== "assistant") continue;
    const t = s.content.trim();
    if (!t) continue;
    const i = rest.indexOf(t);
    if (i < 0) continue;
    rest = rest.slice(i + t.length);
  }
  return rest.replace(/^\s+/, "");
}

// PULSE's own notes (an MCP server connected, skills offered, an earlier
// session resumed, the context folded) are housekeeping, not the agent's
// work: two or more in a row take one line and open on a click.
function groupNotes(list: ActionItem[]): (ActionItem | ActionItem[])[] {
  const out: (ActionItem | ActionItem[])[] = [];
  let notes: ActionItem[] = [];
  const flush = () => {
    if (notes.length >= 2) out.push(notes);
    else out.push(...notes);
    notes = [];
  };
  for (const it of list) {
    if (it.step.kind === "note" && !it.thought) {
      notes.push(it);
      continue;
    }
    flush();
    out.push(it);
  }
  flush();
  return out;
}

// "3s", "1m 20s": the compact span a transcript row has room for.
function brief(sec: number): string {
  const s = Math.max(1, Math.round(sec));
  if (s < 60) return s + "s";
  const m = Math.floor(s / 60);
  return s % 60 ? m + "m " + (s % 60) + "s" : m + "m";
}

// "2 commands, 1 file read": the count line a folded run of steps opens to.
// (format.ts's plural with the num() locale string, kept local so this file
// stays importable by plain node.)
function countWords(items: ActionItem[]): string {
  const n: Record<string, number> = {};
  let rows = 0;
  for (const it of items) {
    if (it.thought) continue;
    rows++;
    if (!ACTION_KINDS.has(it.step.kind)) continue;
    const k = it.step.kind === "command" ? "command" : it.step.kind === "read" ? "file read" : it.step.kind === "edit" || it.step.kind === "write" ? "edit" : it.step.kind === "grep" || it.step.kind === "glob" || it.step.kind === "search" ? "search" : it.step.kind === "subagent" ? "helper" : "tool call";
    n[k] = (n[k] ?? 0) + 1;
  }
  const p = (v: number, word: string) => `${v.toLocaleString("en-US")} ${word}${v === 1 ? "" : "s"}`;
  const parts = Object.entries(n).map(([k, v]) => p(v, k));
  return parts.length ? parts.join(", ") : p(rows, "note");
}

export { ACTION_KINDS, beatsOf, brief, countWords, groupNotes, streamTail, thoughtSeconds };
export type { ActionItem, Beat };
