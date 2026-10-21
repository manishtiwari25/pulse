import type { EvolvePlanning } from "@/lib/api";

interface PlanningView {
  label: string;
  tone: "neutral" | "info" | "warn";
  reason: string;
  next: string;
}

// What the self-improvement planner is doing, in the owner's words. `label`
// is the short state, `next` one plain sentence on what happens next; the
// planner's own reason (technical) is shown as-is under "Show details".
const STATES = new Map<string, Omit<PlanningView, "reason">>([
  ["gated", {
    label: "Waiting on a setup step",
    tone: "warn",
    next: "A setup step comes first. PULSE's planning note says which one.",
  }],
  ["blocked", {
    label: "Blocked",
    tone: "warn",
    next: "Something blocks the next experiment. PULSE's planning note says what. Steps only you can take are never skipped automatically.",
  }],
  ["waiting", {
    label: "Waiting for an agreed suggestion",
    tone: "neutral",
    next: "Agents need a suggestion the board agreed on, that hasn't been tried yet and that fits what they may change.",
  }],
  ["carding", {
    label: "Writing up the next experiment",
    tone: "info",
    next: "The top suggestion doesn't name one file and one setting to change, so an agent is writing it up as an experiment card on its thread. The card needs its own votes before it runs.",
  }],
  ["ready", {
    label: "Ready to start",
    tone: "info",
    next: "Nothing more is needed from you. The runner starts the experiment when it has room.",
  }],
  ["queued", {
    label: "Waiting to start",
    tone: "info",
    next: "An experiment is waiting to start. It starts once the runner is on, nothing is stopped and a slot is free.",
  }],
  ["leased", {
    label: "Experiment running",
    tone: "info",
    next: "An experiment is running; its score isn't in yet.",
  }],
  ["finalizing", {
    label: "Scoring an experiment",
    tone: "info",
    next: "An experiment finished and is being scored. An agent saying it's done doesn't mean the change was kept.",
  }],
]);

export function evolvePlanningView(planning: EvolvePlanning | null | undefined): PlanningView {
  const known = STATES.get(planning?.state ?? "");
  return {
    label: known?.label ?? (planning ? "Unknown planning state" : "Planning unavailable"),
    tone: known?.tone ?? "warn",
    reason: planning?.reason || "PULSE did not report why.",
    next: known?.next ?? "Planning details are unavailable. Being allowed to improve doesn't mean an experiment is running.",
  };
}
