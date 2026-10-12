import type { ProjectCardData } from "./api";

// The home page's note that a project still runs on the generated goal
// (ADR-040): the lead plans, votes and researches toward a direction nobody
// chose until the owner sets one. Pure, so goal-nudge.test.mjs can drive it;
// the page owns React state and the real localStorage.

// One key per project. "dismissed" hides the note until the storage is
// cleared or the goal becomes the owner's, when the key is removed again so
// a later reset to the default shows the note once more.
export const GOAL_NUDGE_PREFIX = "pulse.goal-nudge.";
export const GOAL_NUDGE_DISMISSED = "dismissed";

export const goalNudgeKey = (slug: string) => GOAL_NUDGE_PREFIX + slug;

// The subset of Storage the nudge uses, so tests can pass a Map-backed one.
export interface NudgeStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

type NudgeRow = Pick<ProjectCardData, "slug" | "present" | "missing" | "error" | "goal_source">;

// Whether the note applies to a row at all: a project on this machine whose
// lead works toward the generated default. Rows from a hub that predates
// goal_source say nothing, so they never nag.
export function wantsGoalNudge(p: NudgeRow): boolean {
  return p.goal_source === "generated" && p.present && !p.missing && !p.error;
}

// The rows to show a note for, in the grid's order, minus the dismissed ones.
export function goalNudges<T extends NudgeRow>(projects: readonly T[], dismissed: ReadonlySet<string>): T[] {
  return projects.filter((p) => wantsGoalNudge(p) && !dismissed.has(p.slug));
}

// The dismissed slugs among these rows as one sorted line, so a render can
// compare two reads by value (useSyncExternalStore wants that) - "" when none.
export function readDismissed<T extends NudgeRow>(storage: NudgeStorage, projects: readonly T[]): string {
  return projects
    .filter((p) => storage.getItem(goalNudgeKey(p.slug)) === GOAL_NUDGE_DISMISSED)
    .map((p) => p.slug)
    .sort()
    .join("\n");
}

export const dismissedSet = (line: string): ReadonlySet<string> => new Set(line ? line.split("\n") : []);

// A project whose goal is now the owner's forgets its dismissal, so the note
// returns if the goal is reset to the default later. True when a key went.
export function forgetOwnerDismissals<T extends NudgeRow>(storage: NudgeStorage, projects: readonly T[]): boolean {
  let changed = false;
  for (const p of projects) {
    const key = goalNudgeKey(p.slug);
    if (p.goal_source !== "owner" || storage.getItem(key) === null) continue;
    storage.removeItem(key);
    changed = true;
  }
  return changed;
}

export function dismissGoalNudge(storage: NudgeStorage, slug: string): void {
  storage.setItem(goalNudgeKey(slug), GOAL_NUDGE_DISMISSED);
}

// What follows the project's name in the note: the project runs on a
// generated goal, and why setting one matters.
export const GOAL_NUDGE_WHY = "runs on a generated goal, one PULSE wrote for it. Set your own, so its agents plan, vote and research toward what you want.";
