import type { EvolvePlanning } from "@/lib/api";

interface PlanningView {
  label: string;
  tone: "neutral" | "info" | "warn";
  reason: string;
  next: string;
}

const STATES = new Map<string, Omit<PlanningView, "reason">>([
  ["gated", {
    label: "Gated",
    tone: "warn",
    next: "Follow the reported gate before starting an experiment.",
  }],
  ["blocked", {
    label: "Blocked",
    tone: "warn",
    next: "Resolve the planning issue above before another experiment can be queued. Owner-only gates are not changed automatically.",
  }],
  ["waiting", {
    label: "Waiting for suggestions",
    tone: "neutral",
    next: "An eligible, untried, in-scope suggestion is still needed; see the planning reason above.",
  }],
  ["ready", {
    label: "Ready, not dispatched",
    tone: "info",
    next: "No additional owner step is listed. The eligible card still needs daemon dispatch.",
  }],
  ["queued", {
    label: "Queued",
    tone: "info",
    next: "An experiment is queued. Dispatch still depends on the daemon, STOP and runner caps.",
  }],
  ["leased", {
    label: "Experiment in flight",
    tone: "info",
    next: "An experiment is in flight; its final measured result is not available yet.",
  }],
  ["finalizing", {
    label: "Finalizing",
    tone: "info",
    next: "The controller is finalizing the experiment. A worker reporting done is not proof of a completed cycle.",
  }],
]);

export function evolvePlanningView(planning: EvolvePlanning | null | undefined): PlanningView {
  const known = STATES.get(planning?.state ?? "");
  return {
    label: known?.label ?? (planning ? "Unknown planning state" : "Planning unavailable"),
    tone: known?.tone ?? "warn",
    reason: planning?.reason || "The hub did not report an experiment planning reason.",
    next: known?.next ?? "Planning details are unavailable. Level D alone does not mean an experiment is running.",
  };
}
