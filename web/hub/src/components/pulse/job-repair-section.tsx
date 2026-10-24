"use client";

import { useState } from "react";
import { plural } from "@/lib/format";
import { failedJobsPath, type FailedJobsPage } from "@/lib/job-repair";
import { useApi } from "@/lib/use-api";
import { FailedJobsPanel } from "./job-repair";
import { Disclosure } from "./panel";

// Failed jobs, folded: one line that says how many and that most need
// nothing, and the full list (with Retry) once the owner opens it.
export function FailedJobsSection({ project }: { project?: string }) {
  const [opened, setOpened] = useState(false);
  const summary = useApi<FailedJobsPage>(project === "commons" ? null : failedJobsPath(project, "", 1), { refresh: 30000 });
  const count = summary.error ? undefined : summary.data?.historical_total;
  if (project === "commons") return null;
  if (summary.data && count === 0) return null;

  return (
    <section id={project ? undefined : "failed-jobs"} aria-label="Jobs that failed" className="scroll-mt-32">
      <Disclosure
        variant="row"
        className="rounded-2xl border bg-card px-2.5 py-1.5 sm:px-3.5"
        contentClassName="-mx-2.5 -mb-1.5 mt-1.5 px-0 pb-0 sm:-mx-3.5"
        onToggle={(open) => { if (open) setOpened(true); }}
        summary={
          <span className="grid gap-0.5 py-0.5">
            <span className="text-[15px] font-semibold leading-snug">Jobs that failed</span>
            <span className="text-sm font-normal leading-relaxed text-muted-foreground">
              {count !== undefined ? plural(count, "job") + " failed in the past. " : summary.error ? "PULSE couldn't count them right now; open this to see why. " : ""}
              Most need nothing from you. Look here when a task seems stuck.
            </span>
          </span>
        }
      >
        {opened ? <FailedJobsPanel project={project} folded /> : null}
      </Disclosure>
    </section>
  );
}
