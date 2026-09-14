"use client";

import { useState } from "react";
import { ChevronRightIcon, WrenchIcon } from "lucide-react";
import { num } from "@/lib/format";
import { failedJobsPath, type FailedJobsPage } from "@/lib/job-repair";
import { useApi } from "@/lib/use-api";
import { FailedJobsPanel } from "./job-repair";

export function FailedJobsSection({ project }: { project?: string }) {
  const [opened, setOpened] = useState(false);
  const summary = useApi<FailedJobsPage>(project === "commons" ? null : failedJobsPath(project, "", 1), { refresh: 30000 });
  const count = summary.error ? undefined : summary.data?.historical_total;
  if (project === "commons") return null;

  return (
    <section id={project ? undefined : "failed-jobs"} aria-label="Failed job repair" className="scroll-mt-32">
      <details className="group" onToggle={(event) => { if (event.currentTarget.open) setOpened(true); }}>
        <summary className="flex cursor-pointer list-none flex-wrap items-center gap-2 rounded-xl border bg-card px-4 py-3 text-sm shadow-card [&::-webkit-details-marker]:hidden">
          <ChevronRightIcon className="size-4 shrink-0 transition-transform group-open:rotate-90" aria-hidden />
          <WrenchIcon className="size-4 shrink-0 text-muted-foreground" aria-hidden />
          <span className="font-medium">Failed job repair</span>
          {count !== undefined ? <span className="text-xs text-muted-foreground">{num(count)} historical failures</span> : null}
          {summary.error ? <span className="text-xs text-warn">Count unavailable - expand for details</span> : null}
          <span className="ml-auto text-xs text-muted-foreground">Review causes and queue selected repairs</span>
        </summary>
        {opened ? <div className="mt-3"><FailedJobsPanel project={project} /></div> : null}
      </details>
    </section>
  );
}
