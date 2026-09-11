"use client";

import Link from "next/link";
import { Mono, ToneBadge, When, Who } from "@/components/pulse/who";
import { taskProgress, type Task, type TaskWorker } from "@/lib/api";
import { truncate } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { cn } from "@/lib/utils";

export function isLeadPlanning(t: Pick<Task, "lead">, w: Pick<TaskWorker, "agent" | "kind">): boolean {
  return (!!t.lead && w.agent === t.lead) || w.kind === "lead";
}

export function TaskWorkerInline({ t, w, compact }: { t: Pick<Task, "lead">; w: TaskWorker; compact?: boolean }) {
  const planning = isLeadPlanning(t, w);
  return (
    <span className="inline-flex min-w-0 items-center gap-1.5">
      <Who row={{ agent: w.agent }} compact />
      <span className="text-muted-foreground">{planning ? "is planning" : "on"}</span>
      {!planning ? <ToneBadge tone="info">{w.kind}</ToneBadge> : null}
      <span className="min-w-0 truncate text-foreground">{truncate(w.title || w.job, compact ? 42 : 72)}</span>
      <Mono>{w.job}</Mono>
      {w.harness_session ? (
        <Link href={hrefs.harness({ id: w.harness_session })} className="font-medium text-foreground underline-offset-2 hover:underline">
          session
        </Link>
      ) : null}
      {w.since ? (
        <span className="text-muted-foreground">
          since <When d={w.since} />
        </span>
      ) : null}
      {w.executor ? <span className="text-muted-foreground">{"on " + w.executor}</span> : null}
    </span>
  );
}

export function TaskWorkingLine({ t, compact, className }: { t: Task; compact?: boolean; className?: string }) {
  const { working } = taskProgress(t);
  if (!working.length) return null;
  return (
    <span className={cn("flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground anywhere", compact && "line-clamp-1 flex-nowrap overflow-hidden", className)}>
      <span className="shrink-0 font-medium text-foreground">working now</span>
      {working.map((w, i) => (
        <span key={w.job + i} className="inline-flex min-w-0 items-center gap-2">
          {i ? <span className="text-muted-foreground">·</span> : null}
          <TaskWorkerInline t={t} w={w} compact={compact} />
        </span>
      ))}
    </span>
  );
}
