"use client";

import Link from "next/link";
import { useCallback, useEffect, useSyncExternalStore } from "react";
import { ArrowUpRightIcon } from "lucide-react";
import { Notice } from "@/components/pulse/panel";
import { ProjectLink } from "@/components/pulse/who";
import type { ProjectCardData } from "@/lib/api";
import { dismissGoalNudge, dismissedSet, forgetOwnerDismissals, GOAL_NUDGE_WHY, goalNudges, readDismissed, type NudgeStorage } from "@/lib/goal-nudge";
import { hrefs } from "@/lib/nav";

// The home page's note that a project still runs on the generated goal
// (ADR-040). The dismissals live in localStorage like the view toggles do:
// the server snapshot says "not read yet", so the static export hydrates
// cleanly and no note shows that would vanish a moment later.

const listeners = new Set<() => void>();
function subscribe(cb: () => void) {
  listeners.add(cb);
  window.addEventListener("storage", cb);
  return () => {
    listeners.delete(cb);
    window.removeEventListener("storage", cb);
  };
}
const notify = () => listeners.forEach((l) => l());

function storage(): NudgeStorage | null {
  try {
    return window.localStorage;
  } catch {
    return null; // private mode or storage disabled: a dismissal lasts for this page view only
  }
}

export function useGoalNudges(projects: readonly ProjectCardData[]): { rows: ProjectCardData[]; dismiss: (slug: string) => void } {
  const dismissed = useSyncExternalStore(
    subscribe,
    () => {
      const s = storage();
      return s ? readDismissed(s, projects) : "";
    },
    () => null,
  );
  // A goal the owner set since the last look drops its project's key, so a
  // later reset to the default shows the note again.
  useEffect(() => {
    const s = storage();
    if (s && forgetOwnerDismissals(s, projects)) notify();
  }, [projects]);
  const dismiss = useCallback((slug: string) => {
    const s = storage();
    if (s) dismissGoalNudge(s, slug);
    notify();
  }, []);
  return { rows: dismissed === null ? [] : goalNudges(projects, dismissedSet(dismissed)), dismiss };
}

export function GoalNudge({ p, onDismiss }: { p: ProjectCardData; onDismiss: () => void }) {
  return (
    <Notice tone="info" onDismiss={onDismiss} dismissLabel={"Dismiss the goal note for " + p.slug}>
      <ProjectLink slug={p.slug} /> <span className="text-muted-foreground">{GOAL_NUDGE_WHY}</span>{" "}
      <Link href={hrefs.project(p.slug) + "#goal"} className="inline-flex items-center gap-0.5 rounded-sm font-medium text-info underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
        Set the goal <ArrowUpRightIcon className="size-3.5" aria-hidden />
      </Link>
    </Notice>
  );
}
