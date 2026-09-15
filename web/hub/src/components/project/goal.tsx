"use client";

import Link from "next/link";
import { useId, useState } from "react";
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

export function ProjectGoalPanel({ slug }: { slug: string }) {
  const q = useApi<ProjectGoalPayload>(slug === "commons" ? null : projectPath(slug, "goal"));
  const [editing, setEditing] = useState(false);
  const [resetting, setResetting] = useState(false);
  if (slug === "commons") return null;
  const goal = q.data?.goal;
  const canEdit = q.data?.can_edit === true;

  return (
    <>
      <Panel
        title="Project goal"
        icon={TargetIcon}
        actions={
          goal ? (
            <>
              <ToneBadge tone={goal.source === "owner" ? "info" : "neutral"}>{goal.source === "owner" ? "Owner set" : "Project default"}</ToneBadge>
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
            Every new agent run reads this shared direction. It does not override task scope, approvals, STOP or the project&apos;s{" "}
            <Link href={hrefs.project(slug, "evolution")} className="font-medium text-foreground underline underline-offset-2">self-improvement gates</Link>.
          </>
        }
      >
        <div className="grid gap-3">
          {q.error ? <ErrorNotice error={q.error} onRetry={() => void q.reload()} /> : null}
          {goal ? (
            <>
              <p className="whitespace-pre-wrap text-sm leading-relaxed anywhere">{goal.text}</p>
              <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
                {goal.source === "generated" ? (
                  <span>Generated from this project when no owner goal is set. Edit it to give the fleet a specific outcome.</span>
                ) : (
                  <>
                    <span>Your goal takes precedence over the generated default.</span>
                    {goal.updated_at ? <When d={goal.updated_at} /> : null}
                    {canEdit ? (
                      <Button size="xs" variant="ghost" onClick={() => setResetting(true)}>
                        <RotateCcwIcon /> Use project default
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
        title="Use the project default?"
        description="Replace your current goal with the generated project goal. Existing tasks and self-improvement permissions stay unchanged."
        action="Use project default"
        onConfirm={() => writes.resetProjectGoal(slug)}
        success="Project default restored. New agent runs will read it."
        invalidates={goalViews}
      />
    </>
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
    const notice = toast.loading("Saving project goal...");
    try {
      await writes.setProjectGoal(slug, trimmed);
      invalidate(goalViews);
      toast.success("Project goal saved. New agent runs will read it.", { id: notice });
      onClose();
    } catch (error) {
      toast.dismiss(notice);
      showApiError(error, "Could not save the project goal");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open onOpenChange={(open) => !open && !busy && onClose()}>
      <DialogContent className="sm:max-w-xl">
        <form onSubmit={(event) => { event.preventDefault(); if (!busy) void save(); }} className="grid gap-4">
          <DialogHeader>
            <DialogTitle>Edit project goal</DialogTitle>
            <DialogDescription>
              Give every agent on {slug} one outcome to work toward. Describe what should improve and how progress should be demonstrated. Do not include credentials or private information.
            </DialogDescription>
          </DialogHeader>
          <div className="grid gap-2">
            <Label htmlFor={id}>Shared goal</Label>
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
              {num(count)} / {num(MAX_PROJECT_GOAL_CHARS)} characters. Running agents keep their current brief; subsequent runs use this goal.
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
