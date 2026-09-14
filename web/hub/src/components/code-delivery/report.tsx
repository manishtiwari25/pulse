"use client";

import Link from "next/link";
import { useState } from "react";
import { AlertTriangleIcon, ArrowUpRightIcon, BotIcon, BriefcaseBusinessIcon, ChevronLeftIcon, ChevronRightIcon, FileDiffIcon, FileTextIcon, ListTodoIcon, MessageSquareIcon, RefreshCwIcon, VoteIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { CardGrid, Fact, ItemCard, ItemCardActions, ItemCardHeader, ItemCardMeta, ItemCardTitle } from "@/components/pulse/item-card";
import { EmptyState, ErrorNotice, Kpi, Notice, Panel, SkeletonRows, Stack } from "@/components/pulse/panel";
import { Mono, PostLink, ProjectLink, StatusBadge, ThreadLink, ToneBadge, When, Who } from "@/components/pulse/who";
import { codeDeliveryCommitURL, codeDeliveryCoverageText, codeDeliveryCoverageTone, codeDeliveryIncompleteWindow, codeDeliveryKpiState, codeDeliveryWindowLabel, shortCommit } from "@/lib/code-deliveries";
import { codeDeliveriesPath, type CodeDeliveriesPayload, type CodeDelivery, type CodeDeliveryJob, type CodeDeliverySuggestion, type CodeDeliveryTask, type CodeDeliveryWorker, type ProjectsPayload } from "@/lib/api";
import { hrefs } from "@/lib/nav";
import { num, plural, truncate } from "@/lib/format";
import { useApi } from "@/lib/use-api";

const LIMIT = 100;

export function CodeDeliveredKpi({ project }: { project?: string }) {
  const q = useApi<CodeDeliveriesPayload>(codeDeliveriesPath({ project: project || undefined, limit: LIMIT }), { refresh: 30000 });
  const state = codeDeliveryKpiState(q.data, q.loading, q.error);
  return <Kpi icon={FileDiffIcon} n={state.n} label="Code delivered" sub={state.sub} tone={state.tone} href={hrefs.codeDeliveries({ project })} />;
}

export function CodeDeliveriesReport({ project }: { project?: string }) {
  const [cursors, setCursors] = useState<(number | null)[]>([null]);
  const [jobDelivery, setJobDelivery] = useState<CodeDelivery | null>(null);
  const before = cursors[cursors.length - 1] ?? null;
  const q = useApi<CodeDeliveriesPayload>(codeDeliveriesPath({ project: project || undefined, limit: LIMIT, before }), { refresh: 30000 });
  const registry = useApi<ProjectsPayload>("projects", { refresh: 60000 });
  const remotes = new Map((registry.error ? [] : registry.data?.projects ?? []).map((p) => [p.slug, p.remote]));
  const d = q.data;
  const page = cursors.length;
  const incomplete = codeDeliveryIncompleteWindow(d);
  const coverage = d ? codeDeliveryCoverageText(d) : "";
  const coverageTone = incomplete ? "warn" : d ? codeDeliveryCoverageTone(d.summary) : "neutral";
  const canOlder = !!d?.scope.has_more && d.scope.next_before !== null;

  return (
    <section id="code-deliveries" aria-label="Code deliveries" className="scroll-mt-32">
      <Panel
        title="Code delivered"
        icon={FileDiffIcon}
        actions={
          <span className="flex flex-wrap items-center justify-end gap-2">
            <span>{codeDeliveryWindowLabel(d, page)}</span>
            <Button size="xs" variant="outline" onClick={() => void q.reload()} disabled={q.loading}>
              <RefreshCwIcon className={q.loading ? "animate-spin" : ""} /> Refresh
            </Button>
          </span>
        }
        footer="Counts come from recent land.landed evidence only. Mixed source/docs deliveries count once; docs-only and non-code-only landings are excluded. Votes are provenance/current votes, not approval or quality. Hosted commit links require the commit to have been pushed."
      >
        {q.error && !d ? <ErrorNotice error={q.error} onRetry={() => void q.reload()} /> : null}
        {!d ? q.error ? null : (
          <SkeletonRows n={4} />
        ) : (
          <Stack gap={4}>
            {q.error ? <ErrorNotice error={q.error} onRetry={() => void q.reload()}>Showing cached code-delivery data; refresh failed ({q.error}).</ErrorNotice> : null}
            {registry.error ? <ErrorNotice error={registry.error} onRetry={() => void registry.reload()}>Commit links unavailable: repository details could not be loaded ({registry.error}).</ErrorNotice> : null}
            <Notice tone={coverageTone}>{coverage}</Notice>
            <CardGrid cols={3}>
              <Fact n={num(d.summary.code_deliveries)} label="code deliveries" tone={d.summary.code_deliveries ? "ok" : undefined} />
              <Fact n={num(d.summary.code_files)} label="source-file changes" tone={d.summary.code_files ? "accent" : undefined} />
              <Fact n={num(d.summary.docs_only)} label="docs-only excluded" />
              <Fact n={num(d.summary.non_code)} label="non-code excluded" />
              <Fact n={num(d.summary.unknown)} label="unknown evidence" tone={d.summary.unknown ? "warn" : undefined} />
              <Fact n={num(d.summary.duplicates)} label="duplicates deduped" tone={d.summary.duplicates ? "warn" : undefined} />
            </CardGrid>
            {d.deliveries.length ? (
              <div className="grid gap-3">
                {d.deliveries.map((row) => (
                  <CodeDeliveryCard key={row.project + "/" + row.event_seq + "/" + row.commit} row={row} remote={remotes.get(row.project)} onOpenJob={() => setJobDelivery(row)} />
                ))}
              </div>
            ) : incomplete ? (
              <EmptyState title="Incomplete search window" icon={<AlertTriangleIcon className="size-[18px]" />}>
                The bounded history scan stopped before finding landing events for this scope. This is not a zero; use Older window to continue from the provided cursor.
              </EmptyState>
            ) : d.summary.unknown || d.unavailable.length ? (
              <EmptyState title="No proven code delivery in this window" icon={<AlertTriangleIcon className="size-[18px]" />}>
                The window has unavailable changed-file evidence, so this is not a proven zero. Review the unavailable rows below or load an older window.
              </EmptyState>
            ) : (
              <EmptyState title="No source-code delivery in this window" icon={<FileDiffIcon className="size-[18px]" />}>
                The scanned landing events had complete evidence and no source, test or UI styling file changes.
              </EmptyState>
            )}
            {d.unavailable.length ? <UnavailableRows rows={d.unavailable} /> : null}
            <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
              <span>{page === 1 ? "Recent window" : "Older window " + page}: {plural(d.deliveries.length, "shown delivery")}.</span>
              <span className="flex gap-2">
                <Button size="sm" variant="outline" disabled={page < 2} onClick={() => setCursors((cur) => cur.slice(0, -1))}>
                  <ChevronLeftIcon /> Newer
                </Button>
                <Button size="sm" variant="outline" disabled={!canOlder} onClick={() => { if (d.scope.next_before !== null) setCursors((cur) => [...cur, d.scope.next_before]); }}>
                  Older window <ChevronRightIcon />
                </Button>
              </span>
            </div>
          </Stack>
        )}
      </Panel>
      <JobEvidenceSheet delivery={jobDelivery} remote={jobDelivery ? remotes.get(jobDelivery.project) : undefined} onClose={() => setJobDelivery(null)} />
    </section>
  );
}

function CodeDeliveryCard({ row, remote, onOpenJob }: { row: CodeDelivery; remote?: string; onOpenJob: () => void }) {
  const title = row.job?.title || row.suggestion?.summary || row.tasks.map((t) => t.title).filter(Boolean).join(", ") || "Recorded source-code delivery";
  return (
    <ItemCard onOpen={row.job ? onOpenJob : undefined} ariaLabel={"Code delivery " + row.event_seq} className="min-w-0">
      <ItemCardHeader
        right={
          <>
            <When d={row.at} />
            <CommitLink commit={row.commit} remote={remote} />
          </>
        }
      >
        <ToneBadge tone="ok">source</ToneBadge>
        <ToneBadge tone={row.evidence === "git" ? "info" : "neutral"}>{row.evidence}</ToneBadge>
        <ProjectLink slug={row.project} className="text-xs" />
        <span className="tnum text-muted-foreground">event {row.event_seq}</span>
      </ItemCardHeader>
      <ItemCardTitle chevron={!!row.job}>{title}</ItemCardTitle>
      <ItemCardMeta>
        <span>{plural(row.code_files, "source-file change")} in {plural(row.files_changed, "changed file")}</span>
        {row.paths_truncated ? <ToneBadge tone="warn">paths truncated</ToneBadge> : null}
      </ItemCardMeta>
      <PathPreview paths={row.code_paths} truncated={row.paths_truncated} />
      <Lineage row={row} onOpenJob={onOpenJob} />
    </ItemCard>
  );
}

function CommitLink({ commit, remote }: { commit: string; remote?: string }) {
  const url = codeDeliveryCommitURL(remote, commit);
  if (!url) return <span title={commit + " (repository link unavailable)"}><Mono>{shortCommit(commit)}</Mono></span>;
  return (
    <a
      href={url}
      target="_blank"
      rel="noopener noreferrer"
      aria-label={"View commit " + commit + " (opens in a new tab)"}
      title={"View commit " + commit + " in the configured repository; available after push"}
      className="inline-flex items-center gap-1 rounded-sm text-primary underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
    >
      <Mono>{shortCommit(commit)}</Mono><ArrowUpRightIcon className="size-3" aria-hidden />
    </a>
  );
}

function PathPreview({ paths, truncated }: { paths: string[]; truncated: boolean }) {
  const shown = paths.slice(0, 5);
  return (
    <details className="rounded-lg border border-border/60 bg-muted/20 px-3 py-2 text-xs">
      <summary className="cursor-pointer font-medium text-foreground">{plural(paths.length, "source path")} recorded</summary>
      {shown.length ? (
        <ul className="mt-2 grid gap-1">
          {shown.map((p) => (
            <li key={p}>
              <span title={p} className="block max-w-full truncate">
                <Mono className="text-muted-foreground">{p}</Mono>
              </span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="mt-2 text-muted-foreground">No path names were included with this delivery.</p>
      )}
      {paths.length > shown.length || truncated ? <p className="mt-2 text-muted-foreground">{truncated ? "The server truncated a large path list." : plural(paths.length - shown.length, "more path") + " hidden in the preview."}</p> : null}
    </details>
  );
}

function Lineage({ row, onOpenJob }: { row: CodeDelivery; onOpenJob: () => void }) {
  return (
    <ItemCardActions className="gap-2">
      {row.job ? (
        <Button size="xs" variant="outline" onClick={onOpenJob}>
          <BriefcaseBusinessIcon /> Job <Mono>{row.job.id}</Mono>
        </Button>
      ) : (
        <Missing label="job" />
      )}
      {row.tasks.length ? row.tasks.map((task) => <TaskLink key={task.id} project={row.project} task={task} />) : <Missing label="task" />}
      {row.worker ? <WorkerLink project={row.project} worker={row.worker} /> : <Missing label="worker" />}
      {row.suggestion ? (
        <>
          <PostLink slug={row.project} id={row.suggestion.id} label="source suggestion" />
          <ThreadLink slug={row.project} k={row.suggestion.thread} />
          <VotesLine project={row.project} votes={row.suggestion.votes} />
        </>
      ) : (
        <Missing label="suggestion" />
      )}
    </ItemCardActions>
  );
}

function TaskLink({ project, task }: { project: string; task: CodeDeliveryTask }) {
  return (
    <Button asChild size="xs" variant="ghost" className="text-muted-foreground">
      <Link href={hrefs.task(project, task.id)} title={task.title}>
        <ListTodoIcon /> <Mono>{task.id}</Mono>
      </Link>
    </Button>
  );
}

function WorkerLink({ project, worker }: { project: string; worker: CodeDeliveryWorker }) {
  return (
    <Button asChild size="xs" variant="ghost" className="text-muted-foreground">
      <Link href={hrefs.harness({ project, agent: worker.name })} title={[worker.name, worker.runner, worker.model].filter(Boolean).join(" · ")}>
        <BotIcon /> {worker.name}
      </Link>
    </Button>
  );
}

function VotesLine({ project, votes }: { project: string; votes: CodeDeliverySuggestion["votes"] }) {
  if (!votes) return <ToneBadge tone="neutral">votes unavailable</ToneBadge>;
  return (
    <Link href={hrefs.votes({ project, kind: "suggestion" })} className="inline-flex items-center gap-1 text-xs text-muted-foreground hover:underline" title="Current suggestion votes, not approval or quality">
      <VoteIcon className="size-3.5" /> {votes.up + " up / " + votes.down + " down · score " + votes.score}
    </Link>
  );
}

function Missing({ label }: { label: string }) {
  return <ToneBadge tone="neutral" title={label + " relationship was not recorded"}>{label + " missing"}</ToneBadge>;
}

function UnavailableRows({ rows }: { rows: CodeDeliveriesPayload["unavailable"] }) {
  return (
    <Panel title="Unavailable delivery evidence" icon={AlertTriangleIcon} dense>
      <ul className="divide-y">
        {rows.map((row) => (
          <li key={row.project + "/" + row.event_seq} className="grid gap-1 px-4 py-2.5 text-xs">
            <div className="flex flex-wrap items-center gap-2">
              <ProjectLink slug={row.project} className="text-xs" />
              <Mono>event {row.event_seq}</Mono>
              <When d={row.at} />
            </div>
            <p className="text-muted-foreground anywhere">{row.reason}</p>
          </li>
        ))}
      </ul>
    </Panel>
  );
}

function JobEvidenceSheet({ delivery, remote, onClose }: { delivery: CodeDelivery | null; remote?: string; onClose: () => void }) {
  const job = delivery?.job ?? null;
  return (
    <Sheet open={!!delivery} onOpenChange={(open) => !open && onClose()}>
      <SheetContent side="right" className="flex flex-col gap-0 overflow-hidden rounded-l-xl p-0 data-[side=right]:w-full data-[side=right]:sm:max-w-2xl">
        {delivery && job ? <JobEvidenceBody delivery={delivery} job={job} remote={remote} /> : null}
      </SheetContent>
    </Sheet>
  );
}

function JobEvidenceBody({ delivery, job, remote }: { delivery: CodeDelivery; job: CodeDeliveryJob; remote?: string }) {
  return (
    <>
      <SheetHeader className="border-b bg-card px-4 py-3">
        <div className="flex flex-wrap items-center gap-1.5 pr-8 text-xs">
          <Mono className="font-semibold">{job.id}</Mono>
          <StatusBadge status={job.status} />
          <ToneBadge tone="neutral">{job.kind}</ToneBadge>
          <ProjectLink slug={delivery.project} className="text-xs" />
        </div>
        <SheetTitle className="text-lg font-semibold leading-snug anywhere">{job.title}</SheetTitle>
        <SheetDescription>Landing evidence and recorded work provenance for event {delivery.event_seq}.</SheetDescription>
      </SheetHeader>
      <div className="scroll-thin grid min-h-0 flex-1 gap-4 overflow-y-auto p-4">
        <Panel title="Landing evidence" icon={FileDiffIcon}>
          <div className="grid gap-3 text-sm">
            <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5">
              <dt className="text-muted-foreground">event</dt>
              <dd><Mono>{delivery.event_seq}</Mono></dd>
              <dt className="text-muted-foreground">commit</dt>
              <dd><CommitLink commit={delivery.commit} remote={remote} /></dd>
              <dt className="text-muted-foreground">landed</dt>
              <dd><When d={delivery.at} exact /></dd>
              <dt className="text-muted-foreground">evidence</dt>
              <dd><ToneBadge tone={delivery.evidence === "git" ? "info" : "neutral"}>{delivery.evidence}</ToneBadge></dd>
              <dt className="text-muted-foreground">changes</dt>
              <dd>{plural(delivery.code_files, "source-file change")} in {plural(delivery.files_changed, "changed file")}</dd>
            </dl>
            <PathPreview paths={delivery.code_paths} truncated={delivery.paths_truncated} />
          </div>
        </Panel>
        <Panel title="Work links" icon={BriefcaseBusinessIcon}>
          <Stack>
            <div className="flex flex-wrap items-center gap-2">
              {delivery.tasks.length ? delivery.tasks.map((task) => <TaskLink key={task.id} project={delivery.project} task={task} />) : <Missing label="task" />}
              {delivery.worker ? <WorkerLink project={delivery.project} worker={delivery.worker} /> : <Missing label="worker" />}
            </div>
            {delivery.tasks.length ? (
              <p className="text-xs text-muted-foreground">Task links open the existing task sheet; job rows are shown on the task&apos;s Jobs tab when recorded there.</p>
            ) : null}
            {delivery.worker ? (
              <div className="flex flex-wrap items-center gap-2 text-sm">
                <Who row={{ agent: delivery.worker.name, runner: delivery.worker.runner, model: delivery.worker.model }} />
                <Button asChild size="sm" variant="outline">
                  <Link href={hrefs.harness({ project: delivery.project, agent: delivery.worker.name })}>
                    Worker harness sessions <ArrowUpRightIcon />
                  </Link>
                </Button>
              </div>
            ) : null}
          </Stack>
        </Panel>
        <Panel title="Suggestion provenance" icon={MessageSquareIcon}>
          {delivery.suggestion ? (
            <Stack>
              <div className="flex flex-wrap items-center gap-2 text-sm">
                <PostLink slug={delivery.project} id={delivery.suggestion.id} label={truncate(delivery.suggestion.summary, 90)} />
                <ThreadLink slug={delivery.project} k={delivery.suggestion.thread} />
                {delivery.suggestion.area ? <ToneBadge tone="neutral">{delivery.suggestion.area}</ToneBadge> : null}
              </div>
              <p className="text-xs text-muted-foreground">
                {delivery.suggestion.votes ? "Current votes: " + delivery.suggestion.votes.up + " up / " + delivery.suggestion.votes.down + " down, score " + delivery.suggestion.votes.score + ". Votes are provenance only." : "Votes were not recorded for this source."}
              </p>
            </Stack>
          ) : (
            <EmptyState title="No source suggestion recorded" icon={<FileTextIcon className="size-[18px]" />}>
              This delivery has recorded code evidence, but no project-bound originating suggestion relationship.
            </EmptyState>
          )}
        </Panel>
      </div>
    </>
  );
}
