"use client";

import { useState } from "react";
import { ChevronLeftIcon, ChevronRightIcon, RefreshCwIcon, WrenchIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { errorText, registeredProjects, type Org } from "@/lib/api";
import { num } from "@/lib/format";
import {
  failedJobsPath, JOB_REPAIR_CATEGORIES, JOB_REPAIR_MAX_SELECTION, repairFailedJobs,
  type FailedJob, type FailedJobsPage, type JobRepairResponse,
} from "@/lib/job-repair";
import { useApi } from "@/lib/use-api";
import { ConfirmDialog } from "./confirm";
import { ItemCard, ItemCardHeader, ItemCardMeta, ItemCardTitle } from "./item-card";
import { EmptyState, ErrorNotice, Notice, Panel, SkeletonRows, Stack, Toolbar } from "./panel";
import { Mono, ProjectLink, ToneBadge, When } from "./who";

export function FailedJobsPanel({ project }: { project?: string }) {
  const [filter, setFilter] = useState("");
  const projects = useApi<Org>(project ? null : "org");
  const scope = project || filter;
  return (
    <FailedJobsContent
      key={scope}
      project={scope}
      projectFilter={!project ? (disabled) => (
        <div className="grid gap-1">
          <Select value={filter || "__all__"} onValueChange={(value) => setFilter(value === "__all__" ? "" : value)} disabled={disabled}>
            <SelectTrigger aria-label="Failed jobs project"><SelectValue placeholder="All projects" /></SelectTrigger>
            <SelectContent>
              <SelectItem value="__all__">All projects</SelectItem>
              {registeredProjects(projects.data).map((p) => <SelectItem key={p.slug} value={p.slug}>{p.name || p.slug}</SelectItem>)}
            </SelectContent>
          </Select>
          {projects.error ? <span role="alert" className="text-xs text-bad">Project filters unavailable: {projects.error}</span> : null}
        </div>
      ) : undefined}
    />
  );
}

function FailedJobsContent({ project, projectFilter }: { project: string; projectFilter?: (disabled: boolean) => React.ReactNode }) {
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
      throw new Error("The selection changed or could not be refreshed. Close this dialog and refresh before selecting jobs again.");
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

  return (
    <Panel
      title="Historical failed jobs"
      icon={WrenchIcon}
      actions={
        <Button size="sm" variant="outline" disabled={busy} onClick={() => void refresh()}>
          <RefreshCwIcon className={refreshing ? "animate-spin" : ""} /> {refreshing ? "Refreshing..." : "Refresh"}
        </Button>
      }
      footer="Repair queues work; it does not promise a successful run. No history is deleted, no budgets or gates are changed, and structural or owner-only holds are never overridden."
    >
      <Stack gap={4}>
        <p className="text-sm text-muted-foreground">
          {page ? <><strong className="text-foreground">{num(page.historical_total)}</strong> historical failed jobs{project ? " in " + project : " across projects"}. </> : null}
          This is all-age history of jobs still marked failed, not a count of work that needs retrying. A newer carrier may already have completed it.
          Transient retries reuse a job; their earlier runs and recovery events remain recorded.
        </p>
        <Toolbar>
          {projectFilter?.(busy)}
          <Select value={category} onValueChange={(value) => { setCategory(value); setSelected([]); }} disabled={busy}>
            <SelectTrigger aria-label="Current recovery category on this page"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All categories on this page</SelectItem>
              {Object.entries(JOB_REPAIR_CATEGORIES).map(([key, label]) => (
                <SelectItem key={key} value={key}>{label} ({num(page?.page_counts[key] ?? 0)})</SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button size="sm" disabled={busy || stale || !selectedJobs.length} onClick={() => { setReport(null); setPending(selectedJobs); }}>
            <WrenchIcon /> Repair selected ({selectedJobs.length}/{maxSelection})
          </Button>
        </Toolbar>
        {view.error ? <ErrorNotice error={view.error} onRetry={() => void refresh()} /> : null}
        {writeError ? <ErrorNotice>Repair request error: {writeError} Refresh before retrying; any completed work is protected by the recovery ledger.</ErrorNotice> : null}
        {refreshing || submitting ? <p role="status" className="text-sm text-muted-foreground">{submitting ? "Requesting bounded repair..." : "Refreshing current eligibility..."}</p> : null}
        {report ? (
          <div role="status" aria-live="polite" className="grid gap-3">
            <Notice tone={report.errors ? "bad" : report.skipped ? "warn" : "ok"}>
              Requested {num(report.requested)}: {num(report.queued)} queued, {num(report.skipped)} skipped, {num(report.errors)} errors. Queued does not mean fixed.
            </Notice>
            {report.results.map((item) => (
              <div key={item.job_id} className="rounded-lg border p-3 text-sm anywhere">
                <div className="flex flex-wrap items-center gap-2">
                  <Mono>{item.job_id}</Mono>
                  <ToneBadge tone={item.outcome === "error" ? "bad" : item.outcome === "skipped" ? "warn" : "ok"}>{item.outcome}</ToneBadge>
                  {item.carrier_id ? <span>Carrier: <Mono>{item.carrier_id}</Mono> ({item.carrier_status})</span> : null}
                </div>
                <p className="mt-2">{item.reason}</p>
                <p className="mt-1 text-xs text-muted-foreground">{item.fix}</p>
              </div>
            ))}
          </div>
        ) : null}
        {view.loading ? <SkeletonRows n={3} /> : (
          <>
            <p className="text-xs text-muted-foreground">
              {num(page?.page_counts.eligible ?? 0)} eligible on this page, before queue and lineage rechecks.
              Select up to {maxSelection} explicitly. Category counts and filtering apply only to this page, not the historical total.
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
                {jobs.length ? "No entries in this category on this page. Choose another category or page." : "No historical failed jobs on this page."}
              </EmptyState>
            ) : null}
          </>
        )}
        <div className="flex flex-wrap items-center justify-between gap-2">
          <p className="text-xs text-muted-foreground">Page {cursors.length}: {visible.length} shown, {jobs.length} fetched{page ? " (limit " + page.limit + ")" : ""}.</p>
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
        title={"Queue repair for " + (pending?.length ?? 0) + " selected job(s)?"}
        description="The server rechecks each current failure. Deterministic failures get one corrected copy with earlier evidence; transient failures may reuse the job. Holds, cancellations, STOP, backoff, queue limits and existing carriers can cause individual skips. No historical bulk retry or prerequisite override is authorized."
        action="Queue selected repairs"
        success="Repair request completed - see the per-job outcomes."
        onConfirm={repair}
        invalidates={(path) => /^(jobs\/failed|runner|tasks|org|events)/.test(path)}
      >
        <ul className="max-h-48 space-y-2 overflow-auto text-sm">
          {pending?.map((job) => <li key={job.id}><Mono>{job.id}</Mono> - {job.title} <span className="text-muted-foreground">({job.project}, {job.class})</span></li>)}
        </ul>
      </ConfirmDialog>
    </Panel>
  );
}

function FailedJobCard({ job, checked, disabled, onToggle }: { job: FailedJob; checked: boolean; disabled: boolean; onToggle: () => void }) {
  return (
    <ItemCard ariaLabel={"Historical failure " + job.id} active={checked}>
      <ItemCardHeader right={<When d={job.finished_at} />}>
        <ToneBadge tone={job.eligible ? "ok" : "neutral"}>{JOB_REPAIR_CATEGORIES[job.category]}</ToneBadge>
        <ToneBadge>{job.class}</ToneBadge>
        <span>{job.kind}, Tier {job.tier}</span>
      </ItemCardHeader>
      <ItemCardTitle>{job.title}</ItemCardTitle>
      <ItemCardMeta>
        <Mono>{job.id}</Mono><ProjectLink slug={job.project} />
        {job.carrier_id ? <span>Current carrier: <Mono>{job.carrier_id}</Mono> ({job.carrier_status})</span> : null}
      </ItemCardMeta>
      <p className="whitespace-pre-wrap text-sm anywhere">{job.reason}</p>
      <p className="text-xs text-muted-foreground anywhere">{job.fix}</p>
      {job.retry_at ? <p className="text-xs text-muted-foreground">Backoff due: <When d={job.retry_at} exact /></p> : null}
      <details className="text-xs">
        <summary className="cursor-pointer font-medium">Earlier failure and recovery context</summary>
        <div className="mt-2 grid gap-2">
          <p>Source: <Mono>{job.source || "(no source; deduped by lineage)"}</Mono></p>
          <p>Chain root: <Mono>{job.root}</Mono>. Recoveries: {job.recoveries}/{job.recovery_limit}.</p>
          <p>Recorded result:</p>
          <pre className="max-h-48 overflow-auto whitespace-pre-wrap rounded-md bg-muted/50 p-2 anywhere">{job.result || "(no result recorded)"}</pre>
          {job.last_run_id ? <p>Last run: <Mono>{job.last_run_id}</Mono> ({job.last_run_status})</p> : null}
          {job.last_run_summary ? <pre className="max-h-48 overflow-auto whitespace-pre-wrap rounded-md bg-muted/50 p-2 anywhere">{job.last_run_summary}</pre> : null}
        </div>
      </details>
      <label className="flex items-center gap-2 pt-1 text-sm">
        <input type="checkbox" className="size-4 accent-primary" checked={checked} disabled={disabled} onChange={onToggle} aria-label={"Select " + job.id + " for repair"} />
        {job.eligible ? "Select for bounded repair" : "Not eligible for repair"}
      </label>
    </ItemCard>
  );
}
