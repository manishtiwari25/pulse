"use client";

import { useState } from "react";
import { ChevronLeftIcon, ChevronRightIcon, CircleXIcon, RefreshCwIcon, RotateCcwIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { errorText, registeredProjects, type Org } from "@/lib/api";
import { num, plural, sentence } from "@/lib/format";
import {
  failedJobsPath, JOB_FAILURE_CLASS, JOB_REPAIR_CATEGORIES, JOB_REPAIR_MAX_SELECTION, JOB_REPAIR_SENTENCE, repairFailedJobs,
  type FailedJob, type FailedJobsPage, type JobRepairResponse, type JobRepairResult,
} from "@/lib/job-repair";
import { useApi } from "@/lib/use-api";
import { ConfirmDialog } from "./confirm";
import { ItemCard, ItemCardBody, ItemCardHeader, ItemCardKind, ItemCardMeta, ItemCardTitle } from "./item-card";
import { Disclosure, EmptyState, ErrorNotice, Notice, Panel, SkeletonRows, Stack, Toolbar } from "./panel";
import { Mono, ProjectLink, ToneBadge, When } from "./who";

const OUTCOME_WORD: Record<JobRepairResult["outcome"], string> = { requeued: "Queued again", replanned: "Queued with a fix", skipped: "Skipped", error: "Didn't work" };

// `folded`: shown under FailedJobsSection's own heading and count, so the
// panel drops its title and opening sentence instead of repeating them.
export function FailedJobsPanel({ project, folded }: { project?: string; folded?: boolean }) {
  const [filter, setFilter] = useState("");
  const projects = useApi<Org>(project ? null : "org");
  const scope = project || filter;
  return (
    <FailedJobsContent
      key={scope}
      project={scope}
      folded={folded}
      projectFilter={!project ? (disabled) => (
        <div className="grid gap-1">
          <Select value={filter || "__all__"} onValueChange={(value) => setFilter(value === "__all__" ? "" : value)} disabled={disabled}>
            <SelectTrigger aria-label="Failed jobs project"><SelectValue placeholder="All projects" /></SelectTrigger>
            <SelectContent>
              <SelectItem value="__all__">All projects</SelectItem>
              {registeredProjects(projects.data).map((p) => <SelectItem key={p.slug} value={p.slug}>{p.name || p.slug}</SelectItem>)}
            </SelectContent>
          </Select>
          {projects.error ? <span role="alert" className="text-xs text-bad">Couldn&apos;t load the project list: {projects.error}</span> : null}
        </div>
      ) : undefined}
    />
  );
}

function FailedJobsContent({ project, projectFilter, folded }: { project: string; projectFilter?: (disabled: boolean) => React.ReactNode; folded?: boolean }) {
  const [cursors, setCursors] = useState<string[]>([""]);
  const [category, setCategory] = useState("all");
  const [selected, setSelected] = useState<string[]>([]);
  const [pending, setPending] = useState<FailedJob[] | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [report, setReport] = useState<JobRepairResponse | null>(null);
  const [writeError, setWriteError] = useState("");
  const view = useApi<FailedJobsPage>(failedJobsPath(project, cursors[cursors.length - 1]));
  const page = view.data;
  const maxSelection = Math.min(JOB_REPAIR_MAX_SELECTION, page?.max_selection ?? JOB_REPAIR_MAX_SELECTION);
  const jobs = page?.jobs ?? [];
  const visible = jobs.filter((job) => category === "all" || job.category === category);
  const selectedJobs = visible.filter((job) => job.eligible && selected.includes(job.id));
  const busy = refreshing || submitting || !!pending;
  const stale = view.loading || !!view.error;
  const retryable = page?.page_counts.eligible ?? 0;
  const titles = new Map(jobs.map((j) => [j.id, j.title]));

  const refresh = async () => {
    setRefreshing(true);
    setSelected([]);
    try {
      await view.reload();
    } finally {
      setRefreshing(false);
    }
  };

  const repair = async () => {
    if (!pending?.length || stale || pending.some((job) => !jobs.some((current) => current.id === job.id && current.eligible))) {
      throw new Error("The list changed or couldn't be refreshed. Close this, refresh, and pick the jobs again.");
    }
    setSubmitting(true);
    setWriteError("");
    try {
      const result = await repairFailedJobs(pending.map((job) => job.id), project);
      setReport(result);
      setSelected([]);
      await view.reload();
    } catch (e) {
      const detail = errorText(e);
      setWriteError([detail.error, detail.why, detail.fix].filter(Boolean).join(" "));
      throw e;
    } finally {
      setSubmitting(false);
    }
  };

  const toggle = (job: FailedJob) => {
    if (!job.eligible || stale || busy) return;
    setSelected((current) => current.includes(job.id)
      ? current.filter((id) => id !== job.id)
      : [...selectedJobs.map((item) => item.id), job.id].slice(0, maxSelection));
  };

  const refreshButton = (
    <Button size="xs" variant="outline" disabled={busy} onClick={() => void refresh()}>
      <RefreshCwIcon className={refreshing ? "animate-spin" : ""} /> {refreshing ? "Refreshing…" : "Refresh"}
    </Button>
  );
  return (
    <Panel
      title={folded ? undefined : "Jobs that failed"}
      icon={CircleXIcon}
      actions={folded ? undefined : refreshButton}
      className={folded ? "rounded-none rounded-b-2xl border-0 border-t bg-transparent shadow-none ring-0" : undefined}
      footer="Retrying puts a job back in line; it doesn't guarantee it works this time. Nothing is deleted, no limits change, and holds that only you can lift stay in place."
    >
      <Stack gap={4}>
        {folded ? (
          <div className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2">
            <p className="max-w-3xl text-sm leading-relaxed text-muted-foreground">PULSE retries temporary problems on its own. Pick the ones that still matter and retry them.</p>
            {refreshButton}
          </div>
        ) : <p className="max-w-3xl text-sm leading-relaxed text-muted-foreground">
          {page ? <><strong className="font-semibold text-foreground">{plural(page.historical_total, "job")}</strong>{" failed in the past" + (project ? " in " + project : "") + ". "}</> : null}
          Most need nothing from you: PULSE retries temporary problems on its own. Look here when a task seems stuck, and retry the ones that still matter.
        </p>}
        <Toolbar className="mb-0">
          {projectFilter?.(busy)}
          <Select value={category} onValueChange={(value) => { setCategory(value); setSelected([]); }} disabled={busy}>
            <SelectTrigger aria-label="Filter by reason on this page"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All reasons</SelectItem>
              {Object.entries(JOB_REPAIR_CATEGORIES).map(([key, label]) => (
                <SelectItem key={key} value={key}>{label} ({num(page?.page_counts[key] ?? 0)})</SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button className="sm:ml-auto" disabled={busy || stale || !selectedJobs.length} onClick={() => { setReport(null); setPending(selectedJobs); }}>
            <RotateCcwIcon /> {selectedJobs.length ? "Retry " + plural(selectedJobs.length, "job") : "Retry selected"}
          </Button>
        </Toolbar>
        {view.error ? <ErrorNotice error={view.error} onRetry={() => void refresh()}>Couldn&apos;t load the failed jobs: {view.error}</ErrorNotice> : null}
        {writeError ? <ErrorNotice>The retry didn&apos;t go through: {writeError} Refresh before trying again; anything that already went through is kept.</ErrorNotice> : null}
        {refreshing || submitting ? <p role="status" className="text-sm text-muted-foreground">{submitting ? "Sending the retry…" : "Checking which jobs can be retried…"}</p> : null}
        {report ? (
          <div role="status" aria-live="polite" className="grid gap-2">
            <Notice tone={report.errors ? "bad" : report.skipped ? "warn" : "ok"}>
              {"Asked to retry " + plural(report.requested, "job") + ": " + num(report.queued) + " queued, " + num(report.skipped) + " skipped" + (report.errors ? ", " + num(report.errors) + " didn't work" : "") + ". Queued means it runs again, not that it's fixed."}
            </Notice>
            {report.results.map((item) => (
              <div key={item.job_id} className="rounded-xl border bg-card px-4 py-3 text-sm anywhere">
                <div className="flex flex-wrap items-center gap-2">
                  <ToneBadge tone={item.outcome === "error" ? "bad" : item.outcome === "skipped" ? "warn" : "ok"} title={item.outcome}>{OUTCOME_WORD[item.outcome] ?? sentence(item.outcome)}</ToneBadge>
                  <span className="min-w-0 font-medium anywhere" title={item.job_id}>{titles.get(item.job_id) || "A failed job"}</span>
                </div>
                <p className="mt-1.5 leading-relaxed">{item.reason}</p>
                {item.fix ? <p className="mt-1 text-[13px] leading-relaxed text-muted-foreground">{item.fix}</p> : null}
                {item.carrier_id ? <p className="mt-1 text-xs text-muted-foreground">Now carried by <Mono>{item.carrier_id}</Mono>{item.carrier_status ? " (" + item.carrier_status + ")" : ""}</p> : null}
              </div>
            ))}
          </div>
        ) : null}
        {view.loading ? <SkeletonRows n={3} /> : (
          <>
            <p className="text-[13px] leading-relaxed text-muted-foreground">
              {retryable ? plural(retryable, "job") + " on this page can be retried. " : "None on this page can be retried right now. "}
              {"Pick up to " + num(maxSelection) + " at a time; PULSE checks each one again before it runs. The reason filter covers this page only."}
            </p>
            {visible.length ? (
              <div className="grid gap-3" aria-busy={refreshing || submitting}>
                {visible.map((job) => (
                  <FailedJobCard
                    key={job.id}
                    job={job}
                    checked={selectedJobs.some((item) => item.id === job.id)}
                    disabled={busy || stale || !job.eligible || selectedJobs.length >= maxSelection && !selectedJobs.some((item) => item.id === job.id)}
                    onToggle={() => toggle(job)}
                  />
                ))}
              </div>
            ) : !view.error ? (
              <EmptyState>
                {jobs.length ? "No job on this page fails for that reason. Pick another reason or page." : "No failed jobs here."}
              </EmptyState>
            ) : null}
          </>
        )}
        <div className="flex flex-wrap items-center justify-between gap-2">
          <p className="text-[13px] text-muted-foreground">{"Page " + num(cursors.length) + ": showing " + num(visible.length) + " of " + num(jobs.length)}</p>
          <div className="flex gap-2">
            <Button size="sm" variant="outline" disabled={busy || view.loading || cursors.length < 2} onClick={() => { setCursors((current) => current.slice(0, -1)); setSelected([]); }}>
              <ChevronLeftIcon /> Newer
            </Button>
            <Button size="sm" variant="outline" disabled={busy || stale || !page?.next_before} onClick={() => { if (page?.next_before) { setCursors((current) => [...current, page.next_before]); setSelected([]); } }}>
              Older <ChevronRightIcon />
            </Button>
          </div>
        </div>
      </Stack>
      <ConfirmDialog
        open={!!pending}
        onOpenChange={(open) => { if (!open) setPending(null); }}
        title={"Retry " + plural(pending?.length ?? 0, "job") + "?"}
        description="PULSE checks each one again first. A job that failed because of its work gets one corrected copy that carries what went wrong; a temporary failure simply runs again. Some may be skipped, for example when work is stopped, a job is waiting before its next retry, or another job already carries the work. Nothing is deleted and no limits are raised."
        action="Retry"
        success="Sent. See what happened to each job below."
        onConfirm={repair}
        invalidates={(path) => /^(jobs\/failed|runner|tasks|org|events)/.test(path)}
      >
        <ul className="max-h-48 space-y-1.5 overflow-auto text-sm">
          {pending?.map((job) => <li key={job.id} title={job.id}>{job.title} <span className="text-muted-foreground">({job.project})</span></li>)}
        </ul>
      </ConfirmDialog>
    </Panel>
  );
}

function FailedJobCard({ job, checked, disabled, onToggle }: { job: FailedJob; checked: boolean; disabled: boolean; onToggle: () => void }) {
  const why = JOB_REPAIR_CATEGORIES[job.category] ?? sentence(job.category);
  return (
    <ItemCard ariaLabel={"Failed job: " + job.title} active={checked}>
      <ItemCardHeader right={<When d={job.finished_at} />}>
        <ItemCardKind icon={CircleXIcon} tone="bad">{JOB_FAILURE_CLASS[job.class] ?? "Failed"}</ItemCardKind>
        <ProjectLink slug={job.project} className="font-normal text-muted-foreground" />
      </ItemCardHeader>
      <ItemCardTitle>{job.title}</ItemCardTitle>
      <ItemCardBody clamp={0}>
        <p className="text-foreground/85">{JOB_REPAIR_SENTENCE[job.category] ?? job.reason}</p>
        {job.reason && JOB_REPAIR_SENTENCE[job.category] ? <p className="mt-1 whitespace-pre-wrap text-[13px] anywhere" title="What PULSE recorded">{"What PULSE recorded: " + job.reason}</p> : null}
      </ItemCardBody>
      <ItemCardMeta>
        <ToneBadge tone={job.eligible ? "ok" : "neutral"} dot>{why}</ToneBadge>
        {job.retry_at && job.category === "backoff" ? <span>Next automatic retry <When d={job.retry_at} /></span> : null}
        {job.recoveries ? <span>{"Retried " + num(job.recoveries) + " of " + num(job.recovery_limit) + " times"}</span> : null}
      </ItemCardMeta>
      {/* rotate-0 while closed: an open Disclosure around this one would otherwise turn its chevron too. */}
      <Disclosure summary="Technical details" className="text-[13px] [&:not([open])>summary>svg]:rotate-0" contentClassName="grid gap-2 text-muted-foreground">
          {job.fix ? <p>Suggested fix: {job.fix}</p> : null}
          <p>Job <Mono>{job.id}</Mono>, {job.kind}, Tier {job.tier}, {job.class}</p>
          <p>Source: <Mono>{job.source || "(none; matched by its chain)"}</Mono></p>
          <p>Chain root: <Mono>{job.root}</Mono>. Recoveries: {job.recoveries} of {job.recovery_limit}.</p>
          {job.carrier_id ? <p>Now carried by <Mono>{job.carrier_id}</Mono> ({job.carrier_status})</p> : null}
          {job.retry_at ? <p>Backoff due: <When d={job.retry_at} exact /></p> : null}
          <p>Recorded result:</p>
          <pre className="max-h-48 overflow-auto whitespace-pre-wrap rounded-lg bg-muted/60 p-2 font-mono text-xs text-foreground/85 anywhere">{job.result || "(no result recorded)"}</pre>
          {job.last_run_id ? <p>Last run: <Mono>{job.last_run_id}</Mono> ({job.last_run_status})</p> : null}
          {job.last_run_summary ? <pre className="max-h-48 overflow-auto whitespace-pre-wrap rounded-lg bg-muted/60 p-2 font-mono text-xs text-foreground/85 anywhere">{job.last_run_summary}</pre> : null}
      </Disclosure>
      <label className={"flex w-fit items-center gap-2 pt-1 text-sm " + (job.eligible ? "cursor-pointer font-medium" : "text-muted-foreground")}>
        <input type="checkbox" className="size-4 accent-primary" checked={checked} disabled={disabled} onChange={onToggle} aria-label={"Retry " + job.title} />
        {job.eligible ? "Retry this job" : "Can't be retried: " + why.charAt(0).toLowerCase() + why.slice(1)}
      </label>
    </ItemCard>
  );
}
