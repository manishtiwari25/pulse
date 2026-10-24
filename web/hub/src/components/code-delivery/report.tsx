"use client";

import Link from "next/link";
import { useState } from "react";
import { AlertTriangleIcon, ChevronLeftIcon, ChevronRightIcon, FileDiffIcon, GitMergeIcon, ListTodoIcon, RefreshCwIcon, ThumbsUpIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { CardGrid, ItemCard, ItemCardActions, ItemCardHeader, ItemCardKind, ItemCardMeta, ItemCardTitle, SectionHeading } from "@/components/pulse/item-card";
import { Disclosure, EmptyState, ErrorNotice, Kpi, KpiGrid, Kv, Notice, Panel, SkeletonRows, Stack } from "@/components/pulse/panel";
import { Mono, PostLink, ProjectLink, StatusBadge, ThreadLink, When, Who } from "@/components/pulse/who";
import { codeDeliveryCommitURL, codeDeliveryCoverageText, codeDeliveryCoverageTone, codeDeliveryIncompleteWindow, codeDeliveryKpiState, codeDeliveryWindowLabel, shortCommit } from "@/lib/code-deliveries";
import { codeDeliveriesPath, type CodeDeliveriesPayload, type CodeDelivery, type CodeDeliveryJob, type CodeDeliverySuggestion, type CodeDeliveryTask, type CodeDeliveryWorker, type ProjectsPayload } from "@/lib/api";
import { hrefs } from "@/lib/nav";
import { plural, sentence, truncate } from "@/lib/format";
import { useApi } from "@/lib/use-api";

const LIMIT = 100;

// How a delivery's file list was known, in words.
const EVIDENCE: Record<string, string> = { git: "Read from git", recorded: "From the run's own record" };

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
  const projectCount = d ? new Set(d.deliveries.map((row) => row.project)).size : 0;
  const leftOut = d ? d.summary.docs_only + d.summary.non_code : 0;

  return (
    <section id="code-deliveries" aria-label="Code delivered" className="scroll-mt-32">
      {q.error && !d ? <ErrorNotice error={q.error} onRetry={() => void q.reload()} /> : null}
      {!d ? (
        q.error ? null : <SkeletonRows n={4} />
      ) : (
        <Stack gap={4}>
          {q.error ? <ErrorNotice error={q.error} onRetry={() => void q.reload()}>{"Couldn't refresh, so these are the earlier numbers (" + q.error + ")."}</ErrorNotice> : null}
          {registry.error ? <ErrorNotice error={registry.error} onRetry={() => void registry.reload()}>{"Commit links are missing: the project's repository details didn't load (" + registry.error + ")."}</ErrorNotice> : null}
          <div>
            <KpiGrid className="mb-3">
              <Kpi n={incomplete ? "—" : d.summary.code_deliveries} label="Changes delivered" sub={projectCount ? "Across " + plural(projectCount, "project") : "Merges that changed source files"} />
              <Kpi n={d.summary.code_files} label="Source files changed" sub="Code, tests and styles" />
              <Kpi n={leftOut} label="Left out" sub="Docs-only or no code" />
              <Kpi n={d.summary.unknown} label="Couldn't check" sub={d.summary.unknown ? "Left out of the count" : "Every merge was checked"} tone={d.summary.unknown ? "warn" : "neutral"} />
            </KpiGrid>
            {coverageTone === "warn" ? <Notice tone="warn">{coverage}</Notice> : <p className="max-w-3xl text-sm leading-relaxed text-muted-foreground">{coverage}</p>}
          </div>
          <div>
            <SectionHeading
              note={codeDeliveryWindowLabel(d, page) + "."}
              action={
                <Button size="sm" variant="outline" onClick={() => void q.reload()} disabled={q.loading}>
                  <RefreshCwIcon className={q.loading ? "animate-spin motion-reduce:animate-none" : ""} /> Refresh
                </Button>
              }
            >
              What was delivered
            </SectionHeading>
            {d.deliveries.length ? (
              <CardGrid cols={2}>
                {d.deliveries.map((row) => (
                  <CodeDeliveryCard key={row.project + "/" + row.event_seq + "/" + row.commit} row={row} remote={remotes.get(row.project)} onOpenJob={() => setJobDelivery(row)} />
                ))}
              </CardGrid>
            ) : incomplete ? (
              <EmptyState title="The search stopped early" icon={<AlertTriangleIcon className="size-[18px]" />}>
                It ran out of room before it reached any merges, so this is not a real zero. Use Older to keep looking further back.
              </EmptyState>
            ) : d.summary.unknown || d.unavailable.length ? (
              <EmptyState title="Nothing proven yet" icon={<AlertTriangleIcon className="size-[18px]" />}>
                Some merges here couldn&apos;t be checked, so this is not a real zero. See the list below, or look at older merges.
              </EmptyState>
            ) : (
              <EmptyState title="No code changes here" icon={<FileDiffIcon className="size-[18px]" />}>
                Every merge here was checked, and none of them changed source code, tests or styles. Finished work that changes code shows up here as it merges.
              </EmptyState>
            )}
          </div>
          {d.unavailable.length ? <UnavailableRows rows={d.unavailable} /> : null}
          <div className="flex flex-wrap items-center justify-between gap-2 text-[13px] text-muted-foreground">
            <span>{(page === 1 ? "Latest merges" : "Older merges, page " + page) + ": " + plural(d.deliveries.length, "change") + " shown."}</span>
            <span className="flex gap-2">
              <Button size="sm" variant="outline" disabled={page < 2} onClick={() => setCursors((cur) => cur.slice(0, -1))}>
                <ChevronLeftIcon /> Newer
              </Button>
              <Button
                size="sm"
                variant="outline"
                disabled={!canOlder}
                onClick={() => {
                  if (d.scope.next_before !== null) setCursors((cur) => [...cur, d.scope.next_before]);
                }}
              >
                Older <ChevronRightIcon />
              </Button>
            </span>
          </div>
          <Disclosure summary="How this is counted" className="rounded-xl border bg-card px-4 py-3" contentClassName="text-[13px] leading-relaxed text-muted-foreground">
            <ul className="grid list-disc gap-1 pl-5">
              <li>Only recent merges of finished work count. A merge that changes both code and docs counts once.</li>
              <li>Merges that only change docs, or change no code at all, are left out.</li>
              <li>Votes show what the agents thought of the idea, not whether the change is good.</li>
              <li>Commit links work once the commit has been pushed to the project&apos;s repository.</li>
            </ul>
          </Disclosure>
        </Stack>
      )}
      <JobEvidenceSheet delivery={jobDelivery} remote={jobDelivery ? remotes.get(jobDelivery.project) : undefined} onClose={() => setJobDelivery(null)} />
    </section>
  );
}

// The job's or suggestion's words; task titles are the owner's own and stay as typed.
const deliveryTitle = (row: CodeDelivery) => (row.job?.title || row.suggestion?.summary ? sentence(row.job?.title || row.suggestion?.summary || "") : row.tasks.map((t) => t.title).filter(Boolean).join(", ") || "A change to the source code");

// The quiet look every secondary link in a card's action row shares.
const ROW_LINK = "inline-flex items-center gap-1 rounded-sm text-[13px] font-medium text-primary underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";

function CodeDeliveryCard({ row, remote, onOpenJob }: { row: CodeDelivery; remote?: string; onOpenJob: () => void }) {
  const title = deliveryTitle(row);
  return (
    <ItemCard onOpen={row.job ? onOpenJob : undefined} ariaLabel={"Change delivered to " + row.project + ": " + title} className="min-w-0">
      <ItemCardHeader right={<When d={row.at} />}>
        <span title={"Event " + row.event_seq}>
          <ItemCardKind icon={GitMergeIcon}>Merged into</ItemCardKind>
        </span>
        <ProjectLink slug={row.project} className="-ml-0.5" />
      </ItemCardHeader>
      <ItemCardTitle chevron={!!row.job}>{title}</ItemCardTitle>
      <ItemCardMeta>
        <span title={plural(row.files_changed, "file") + " changed in all"}>{plural(row.code_files, "source file") + " changed"}</span>
        {row.worker ? <WorkerLink project={row.project} worker={row.worker} /> : null}
        {row.tasks.map((task) => (
          <TaskLink key={task.id} project={row.project} task={task} />
        ))}
        {row.suggestion?.votes ? <VotesLine project={row.project} votes={row.suggestion.votes} /> : null}
        {!row.job && row.suggestion?.thread ? (
          // Without a job there is no sheet, so the board thread stays on the card.
          <span className="inline-flex min-w-0 items-center gap-1">
            {"In "}
            <ThreadLink slug={row.project} k={row.suggestion.thread} />
          </span>
        ) : null}
      </ItemCardMeta>
      <PathPreview paths={row.code_paths} truncated={row.paths_truncated} />
      <ItemCardActions className="gap-x-4">
        {row.job ? (
          <Button size="sm" variant="outline" onClick={onOpenJob} title={"Job " + row.job.id}>
            See the details
          </Button>
        ) : null}
        <CommitLink commit={row.commit} remote={remote} evidence={row.evidence} />
        {row.suggestion ? <PostLink slug={row.project} id={row.suggestion.id} label="The suggestion behind it" className={ROW_LINK} /> : null}
      </ItemCardActions>
    </ItemCard>
  );
}

// A link to the commit on the project's host; the bare short hash when there is no link to make.
// `hash` prints the short hash as the link text too (the sheet's fact list).
function CommitLink({ commit, remote, hash, evidence }: { commit: string; remote?: string; hash?: boolean; evidence?: string }) {
  const url = codeDeliveryCommitURL(remote, commit);
  const how = evidence ? " " + (EVIDENCE[evidence] || sentence(evidence)) + "." : "";
  if (!url)
    return (
      <span className="text-[13px] text-muted-foreground" title={"Commit " + commit + ". There's no link: the repository isn't on a known host, or its details didn't load." + how}>
        {hash ? null : "Commit "}
        <Mono>{shortCommit(commit)}</Mono>
      </span>
    );
  return (
    <a
      href={url}
      target="_blank"
      rel="noopener noreferrer"
      aria-label={"View commit " + commit + " (opens in a new tab)"}
      title={"View commit " + commit + " in the project's repository. It works once the commit is pushed." + how}
      className={ROW_LINK}
    >
      {hash ? <Mono>{shortCommit(commit)}</Mono> : "View commit"}
    </a>
  );
}

function PathPreview({ paths, truncated }: { paths: string[]; truncated: boolean }) {
  const shown = paths.slice(0, 5);
  return (
    <Disclosure summary={paths.length === 1 ? "Show the file" : paths.length ? "Show the " + plural(paths.length, "file") : "Which files?"} className="rounded-xl bg-muted/50 px-3 py-2" contentClassName="text-[13px]">
      {shown.length ? (
        <ul className="grid gap-1">
          {shown.map((p) => (
            <li key={p} className="min-w-0">
              <span title={p} className="block max-w-full truncate">
                <Mono className="text-muted-foreground">{p}</Mono>
              </span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-muted-foreground">No file names were recorded for this change.</p>
      )}
      {paths.length > shown.length || truncated ? <p className="mt-2 text-muted-foreground">{truncated ? "The list was cut short because it was long." : "And " + plural(paths.length - shown.length, "more file") + "."}</p> : null}
    </Disclosure>
  );
}

function TaskLink({ project, task }: { project: string; task: CodeDeliveryTask }) {
  return (
    <Link href={hrefs.task(project, task.id)} title={task.title} className="inline-flex items-center gap-1 underline-offset-2 hover:text-foreground hover:underline">
      <ListTodoIcon className="size-3.5" aria-hidden /> {task.id}
    </Link>
  );
}

function WorkerLink({ project, worker }: { project: string; worker: CodeDeliveryWorker }) {
  return (
    <Link href={hrefs.harness({ project, agent: worker.name })} title={[worker.name, worker.runner, worker.model].filter(Boolean).join(", ")} className="underline-offset-2 hover:text-foreground hover:underline">
      {"By " + worker.name}
    </Link>
  );
}

function votesWords(v: NonNullable<CodeDeliverySuggestion["votes"]>): string {
  return v.up + " for, " + v.down + " against";
}

function VotesLine({ project, votes }: { project: string; votes: CodeDeliverySuggestion["votes"] }) {
  if (!votes) return <span>No votes recorded</span>;
  return (
    <Link href={hrefs.votes({ project, kind: "suggestion" })} className="inline-flex items-center gap-1 underline-offset-2 hover:text-foreground hover:underline" title={"Agents' votes on the suggestion (score " + votes.score + "), not a review of the change"}>
      <ThumbsUpIcon className="size-3.5" aria-hidden /> {votesWords(votes)}
    </Link>
  );
}

function UnavailableRows({ rows }: { rows: CodeDeliveriesPayload["unavailable"] }) {
  return (
    <Panel title="Merges that couldn't be checked" icon={AlertTriangleIcon} dense>
      <ul className="divide-y">
        {rows.map((row) => (
          <li key={row.project + "/" + row.event_seq} className="grid gap-1 px-4 py-3 text-sm sm:px-5">
            <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[13px] text-muted-foreground">
              <ProjectLink slug={row.project} />
              <When d={row.at} />
              <span className="text-xs" title="Event number in the project's history">{"Event " + row.event_seq}</span>
            </div>
            <p className="text-muted-foreground anywhere">{sentence(row.reason)}</p>
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
      <SheetHeader className="gap-2 border-b bg-card px-5 pb-4 pt-5">
        <SheetTitle className="pr-8 text-xl font-semibold leading-snug tracking-tight anywhere">{sentence(job.title)}</SheetTitle>
        <SheetDescription asChild>
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 text-[13px] text-muted-foreground">
            <StatusBadge status={job.status} />
            <ProjectLink slug={delivery.project} className="font-normal" />
            <span>
              {"Merged "}
              <When d={delivery.at} />
            </span>
          </div>
        </SheetDescription>
      </SheetHeader>
      <div className="scroll-thin grid min-h-0 flex-1 content-start gap-4 overflow-y-auto p-5">
        <Panel title="What changed" icon={FileDiffIcon}>
          <div className="grid gap-3">
            <Kv
              rows={[
                ["Merged", <When key="at" d={delivery.at} exact />],
                ["Files", plural(delivery.code_files, "source file") + " changed, of " + plural(delivery.files_changed, "file") + " in all"],
                ["Commit", <CommitLink key="c" commit={delivery.commit} remote={remote} hash />],
                ["How we know", <span key="e" title={delivery.evidence}>{EVIDENCE[delivery.evidence] || sentence(delivery.evidence)}</span>],
              ]}
            />
            <PathPreview paths={delivery.code_paths} truncated={delivery.paths_truncated} />
          </div>
        </Panel>
        <Panel title="The work behind it" icon={ListTodoIcon}>
          <Stack>
            {delivery.tasks.length ? (
              <ul className="grid gap-1.5 text-sm">
                {delivery.tasks.map((task) => (
                  <li key={task.id} className="flex min-w-0 flex-wrap items-baseline gap-x-2">
                    <Link href={hrefs.task(delivery.project, task.id)} className="font-medium underline-offset-2 hover:underline anywhere">
                      {task.title || task.id}
                    </Link>
                    <span className="text-xs text-muted-foreground">{task.id}</span>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="text-sm text-muted-foreground">No task was recorded for this change.</p>
            )}
            {delivery.worker ? (
              <div className="flex flex-wrap items-center justify-between gap-2 text-sm">
                <Who row={{ agent: delivery.worker.name, runner: delivery.worker.runner, model: delivery.worker.model }} />
                <Button asChild size="sm" variant="outline">
                  <Link href={hrefs.harness({ project: delivery.project, agent: delivery.worker.name })}>
                    {"See " + delivery.worker.name + "'s runs"}
                  </Link>
                </Button>
              </div>
            ) : (
              <p className="text-sm text-muted-foreground">No agent was recorded for this change.</p>
            )}
          </Stack>
        </Panel>
        <Panel title="Where the idea came from">
          {delivery.suggestion ? (
            <Stack>
              <PostLink slug={delivery.project} id={delivery.suggestion.id} label={truncate(sentence(delivery.suggestion.summary), 120)} className="text-sm font-medium" />
              <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-[13px] text-muted-foreground">
                <span className="inline-flex items-center gap-1">
                  On the board: <ThreadLink slug={delivery.project} k={delivery.suggestion.thread} />
                </span>
                {delivery.suggestion.area ? <span>{"About " + delivery.suggestion.area}</span> : null}
                <VotesLine project={delivery.project} votes={delivery.suggestion.votes} />
              </div>
              <p className="text-xs text-muted-foreground">Votes show what the agents thought of the idea, not a review of the change.</p>
            </Stack>
          ) : (
            <p className="text-sm text-muted-foreground">No suggestion was recorded for this change. It came from a task or a job directly.</p>
          )}
        </Panel>
        <Disclosure summary="Technical details" className="rounded-xl border bg-card px-4 py-3" contentClassName="text-[13px]">
          <Kv
            rows={[
              ["Job", <Mono key="j">{job.id}</Mono>],
              ["Job kind", job.kind],
              ["Event", <Mono key="e">{String(delivery.event_seq)}</Mono>],
              ["Commit", <Mono key="c">{delivery.commit}</Mono>],
            ]}
          />
        </Disclosure>
      </div>
    </>
  );
}
