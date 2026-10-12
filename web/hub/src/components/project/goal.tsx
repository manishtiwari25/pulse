"use client";

import Link from "next/link";
import { useEffect, useId, useRef, useState } from "react";
import { Loader2Icon, PencilIcon, RotateCcwIcon, TargetIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { ConfirmDialog, showApiError } from "@/components/pulse/confirm";
import { ErrorNotice, Panel, SkeletonRows } from "@/components/pulse/panel";
import { ToneBadge, When } from "@/components/pulse/who";
import { MAX_PROJECT_GOAL_CHARS, projectPath, writes, type ProjectGoal, type ProjectGoalPayload } from "@/lib/api";
import { num } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { invalidate, useApi } from "@/lib/use-api";

const goalViews = (path: string) => path === "org" || path === "projects" || path.startsWith("projects/") || path.startsWith("events");

// The id the home page's "generated goal" note links to (project/?slug=x#goal).
export const GOAL_ANCHOR = "goal";

export function ProjectGoalPanel({ slug }: { slug: string }) {
  const q = useApi<ProjectGoalPayload>(slug === "commons" ? null : projectPath(slug, "goal"));
  const [editing, setEditing] = useState(false);
  const [resetting, setResetting] = useState(false);
  const root = useRef<HTMLElement>(null);
  // The panel mounts after the overview has loaded, too late for the
  // browser's own jump to #goal, so it scrolls itself into view when asked.
  useEffect(() => {
    const jump = () => {
      if (window.location.hash === "#" + GOAL_ANCHOR) window.requestAnimationFrame(() => root.current?.scrollIntoView());
    };
    jump();
    window.addEventListener("hashchange", jump);
    return () => window.removeEventListener("hashchange", jump);
  }, []);
  if (slug === "commons") return null;
  const goal = q.data?.goal;
  const canEdit = q.data?.can_edit === true;

  return (
    <section ref={root} id={GOAL_ANCHOR} className="scroll-mt-32" aria-label="Project goal">
      <Panel
        title="Project goal"
        icon={TargetIcon}
        actions={
          goal ? (
            <>
              <ToneBadge tone="neutral" title={goal.source === "owner" ? "You wrote this goal" : "PULSE wrote this goal from the project; nobody has set one yet"}>{goal.source === "owner" ? "Set by you" : "Written by PULSE"}</ToneBadge>
              {canEdit ? (
                <Button size="sm" variant="outline" onClick={() => setEditing(true)}>
                  <PencilIcon /> Edit goal
                </Button>
              ) : null}
            </>
          ) : undefined
        }
        footer={
          <>
            Every agent reads this before it starts work. It never overrides a task&apos;s scope, your approvals, a stop, or the project&apos;s{" "}
            <Link href={hrefs.project(slug, "evolution")} className="font-medium text-foreground underline underline-offset-2">self-improvement limits</Link>.
          </>
        }
      >
        <div className="grid gap-3">
          {q.error ? <ErrorNotice error={q.error} onRetry={() => void q.reload()} /> : null}
          {goal ? (
            <>
              <p className="max-w-3xl whitespace-pre-wrap text-[15px] leading-relaxed anywhere">{goal.text}</p>
              <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[13px] text-muted-foreground">
                {goal.source === "generated" ? (
                  <span>PULSE wrote this from the project because you haven&apos;t set a goal. Edit it to point the agents at the outcome you want.</span>
                ) : (
                  <>
                    {goal.updated_at ? <span>Changed <When d={goal.updated_at} /></span> : <span>Your goal replaces the one PULSE wrote.</span>}
                    {canEdit ? (
                      <Button size="xs" variant="ghost" onClick={() => setResetting(true)}>
                        <RotateCcwIcon /> Go back to the PULSE goal
                      </Button>
                    ) : null}
                  </>
                )}
              </div>
            </>
          ) : q.loading ? <SkeletonRows n={2} /> : null}
        </div>
      </Panel>
      {editing && goal && canEdit ? (
        <GoalEditor slug={slug} goal={goal} onClose={() => setEditing(false)} />
      ) : null}
      <ConfirmDialog
        open={resetting}
        onOpenChange={setResetting}
        title="Go back to the goal PULSE wrote?"
        description="Your goal is replaced by the one PULSE writes from the project. Tasks and self-improvement settings stay as they are."
        action="Use the PULSE goal"
        dismiss="Keep my goal"
        onConfirm={() => writes.resetProjectGoal(slug)}
        success="Back to the PULSE goal. Agents read it from their next run."
        invalidates={goalViews}
      />
    </section>
  );
}

function GoalEditor({ slug, goal, onClose }: { slug: string; goal: ProjectGoal; onClose: () => void }) {
  const id = useId();
  const [text, setText] = useState(goal.text);
  const [busy, setBusy] = useState(false);
  const trimmed = text.trim();
  const count = Array.from(trimmed).length;
  const tooLong = count > MAX_PROJECT_GOAL_CHARS;

  const save = async () => {
    if (!trimmed || tooLong) {
      toast.error("Write a goal with 1 to " + num(MAX_PROJECT_GOAL_CHARS) + " characters.");
      return;
    }
    setBusy(true);
    const notice = toast.loading("Saving the goal…");
    try {
      await writes.setProjectGoal(slug, trimmed);
      invalidate(goalViews);
      toast.success("Goal saved. Agents read it from their next run.", { id: notice });
      onClose();
    } catch (error) {
      toast.dismiss(notice);
      showApiError(error, "The goal wasn't saved");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open onOpenChange={(open) => !open && !busy && onClose()}>
      <DialogContent className="sm:max-w-xl">
        <form onSubmit={(event) => { event.preventDefault(); if (!busy) void save(); }} className="grid gap-4">
          <DialogHeader>
            <DialogTitle>Edit the project goal</DialogTitle>
            <DialogDescription>
              One outcome every agent on {slug} works toward. Say what should get better and how you would see the progress. Leave out passwords, keys and private details.
            </DialogDescription>
          </DialogHeader>
          <div className="grid gap-2">
            <Label htmlFor={id}>Goal</Label>
            <Textarea
              id={id}
              value={text}
              onChange={(event) => setText(event.target.value)}
              className="max-h-64 resize-y"
              rows={6}
              autoFocus
              disabled={busy}
              aria-invalid={tooLong}
              aria-describedby={id + "-count"}
              placeholder="Improve the project's most important user outcome, while keeping existing behavior reliable."
            />
            <p id={id + "-count"} className={"text-xs " + (tooLong ? "text-bad" : "text-muted-foreground")}>
              {num(count)} of {num(MAX_PROJECT_GOAL_CHARS)} characters. Agents already at work finish with the old goal; their next run reads this one.
            </p>
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" disabled={busy} onClick={onClose}>Cancel</Button>
            <Button type="submit" disabled={busy || !trimmed || tooLong}>
              {busy ? <Loader2Icon className="animate-spin" /> : <TargetIcon />} Save goal
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
