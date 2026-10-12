"use client";

import Link from "next/link";
import { Avatar } from "@/components/pulse/who";
import { taskProgress, type Task, type TaskWorker } from "@/lib/api";
import { fmtDate, rel, sentence, truncate } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { useNow } from "@/lib/use-api";
import { cn } from "@/lib/utils";

export function isLeadPlanning(t: Pick<Task, "lead">, w: Pick<TaskWorker, "agent" | "kind">): boolean {
  return (!!t.lead && w.agent === t.lead) || w.kind === "lead";
}

// A relative time inside a sentence, exact time in the tooltip.
export function Ago({ d, className }: { d?: string | null; className?: string }) {
  const now = useNow();
  if (!d) return null;
  return (
    <time dateTime={d} title={fmtDate(d)} className={className}>
      {rel(d, now)}
    </time>
  );
}

// Why an active task has nobody on it right now, in the runner's own terms
// (tendTasks): a queued job waits for a free worker, a failed job goes back
// to the lead, done jobs wait for their branch to land.
export function idleLine(t: Task): string | null {
  const p = taskProgress(t);
  if (p.working.length || p.running || (t.status !== "in_progress" && t.status !== "planned")) return null;
  if (p.queued) return "The next step is waiting for a free agent.";
  if (p.blocked) return "A step is blocked. The lead agent will sort it out or ask you.";
  if (!p.total) return t.lead ? "The lead agent is planning the steps." : "Waiting for the lead agent to plan the steps.";
  if (p.failed) return "A step failed. The lead agent is planning another way.";
  return "Every step is done. Waiting to merge the work.";
}

// One readable sentence: "cinder is working on Await the price lookup,
// started 9 minutes ago." It flows as text (never a flex row), so a narrow
// card wraps it like a sentence instead of squeezing each word into a column.
// The job id, its kind and the machine stay in the tooltip.
// `bare` drops the step's title where the row already shows it.
export function TaskWorkerInline({ t, w, compact, bare }: { t: Pick<Task, "lead">; w: TaskWorker; compact?: boolean; bare?: boolean }) {
  const planning = isLeadPlanning(t, w);
  const what = sentence(w.title || "") || "a step";
  const tip = ["Job " + w.job, w.kind ? "kind: " + w.kind : "", w.executor ? "on machine " + w.executor : ""].filter(Boolean).join(", ");
  return (
    <span className="min-w-0 anywhere" title={tip}>
      <Avatar seed={w.agent} size={18} className="mr-1.5 -mt-0.5 align-middle" />
      <span className="font-medium text-foreground">{w.agent || "An agent"}</span>
      {planning ? " is planning the steps" : bare ? " is working on it" : " is working on "}
      {planning || bare ? null : <span className="text-foreground">{truncate(what, compact ? 56 : 96)}</span>}
      {w.since ? (
        <>
          {", started "}
          <Ago d={w.since} />
        </>
      ) : null}
      {"."}
      {w.harness_session && !compact ? (
        <>
          {" "}
          <Link href={hrefs.harness({ id: w.harness_session })} title={"Run " + w.harness_session} className="font-medium text-primary underline-offset-2 hover:underline">
            Watch the run
          </Link>
        </>
      ) : null}
    </span>
  );
}

// Who is on the task right now, one sentence per agent.
export function TaskWorkingLine({ t, compact, className }: { t: Task; compact?: boolean; className?: string }) {
  const { working } = taskProgress(t);
  if (!working.length) return null;
  const shown = compact ? working.slice(0, 1) : working;
  return (
    <div className={cn("grid min-w-0 gap-1 text-[13px] leading-relaxed text-muted-foreground", className)}>
      {shown.map((w, i) => (
        <p key={w.job + i} className={cn("min-w-0", compact && "line-clamp-2")}>
          <TaskWorkerInline t={t} w={w} compact={compact} />
        </p>
      ))}
      {compact && working.length > 1 ? <p className="pl-6">{working.length === 2 ? "and 1 more agent" : "and " + (working.length - 1) + " more agents"}</p> : null}
    </div>
  );
}
