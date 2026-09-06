"use client";

import Link from "next/link";
import { Button } from "@/components/ui/button";
import { ActivityChart, lastDays } from "@/components/pulse/charts";
import { MachinePanel, ProjectCard, ProposalCard, WorkingNow } from "@/components/pulse/cards";
import { LiveFeed } from "@/components/pulse/live-feed";
import { EmptyState, ErrorNotice, Kpi, KpiGrid, PageTitle, Panel, SkeletonPage } from "@/components/pulse/panel";
import { Mono, ProjectLink, StatusBadge, When } from "@/components/pulse/who";
import { NewTaskButton } from "@/components/tasks/task-dialogs";
import { hrefs } from "@/lib/nav";
import { num, plural } from "@/lib/format";
import { allProjects, type Compute, type Org, type Runner, type Task, type TasksPayload, type Timeline } from "@/lib/api";
import { useApi } from "@/lib/use-api";

export default function HubPage() {
  const org = useApi<Org>("org");
  const tl = useApi<Timeline>("timeline", { refresh: 60000 });
  const runner = useApi<Runner>("runner");
  const tasks = useApi<TasksPayload>("tasks");
  const compute = useApi<Compute>("compute", { refresh: 30000 });

  if (org.error && !org.data)
    return (
      <>
        <PageTitle>Hub</PageTitle>
        <ErrorNotice error={org.error} onRetry={() => void org.reload()}>
          The hub API did not answer ({org.error}). Start it with <Mono>pulse hub</Mono> and reload.
        </ErrorNotice>
      </>
    );
  if (!org.data) return <SkeletonPage rows={4} />;

  const o = org.data;
  const projects = allProjects(o);
  const awaiting = o.proposals.filter((p) => p.status === "awaiting");
  const openTasks = (tasks.data?.tasks ?? []).filter((t) => t.status !== "done" && t.status !== "cancelled");
  const pendingCompute = (compute.data?.requests ?? []).filter((r) => r.status === "pending");
  const attention = projects.filter((p) => p.error || p.missing || p.stop_present || !p.board_present);
  const r = runner.data;
  const sparkFor = (slug: string): number[] | undefined => {
    const s = tl.data?.series?.find((x) => x.slug === slug);
    return s ? lastDays(s, 14) : undefined;
  };

  return (
    <>
      <PageTitle
        sub={
          <>
            {plural(o.totals.projects, "project")} {o.totals.brain ? <>· framework brain <Mono>{o.totals.brain}</Mono></> : null} · tally <Mono>{o.tally_mode}</Mono> · store <Mono>{o.store || o.registry}</Mono>
            {o.binary_version ? <> · pulse <Mono>{o.binary_version}</Mono></> : null}
          </>
        }
        actions={
          <>
            <NewTaskButton />
            <Button asChild variant="outline" size="sm">
              <Link href={hrefs.tasks()}>All tasks</Link>
            </Button>
          </>
        }
      >
        Hub
      </PageTitle>

      <KpiGrid>
        <Kpi n={awaiting.length} label="for you" sub="proposals waiting" tone={awaiting.length ? "ok" : "neutral"} href={awaiting.length ? "#for-you" : undefined} />
        <Kpi n={openTasks.length} label="open tasks" sub={tasks.data ? plural(openTasks.filter((t) => t.status === "in_progress").length, "in progress") : "loading"} href={hrefs.tasks()} />
        <Kpi n={pendingCompute.length} label="compute" sub={pendingCompute.length ? "requests to decide" : compute.data ? num(compute.data.caps.vms_now) + " VMs up" : "loading"} tone={pendingCompute.length ? "warn" : "neutral"} href={hrefs.compute()} />
        <Kpi n={o.totals.agents_active} label="agents active" sub={num(o.totals.agents) + " known"} href={hrefs.agents()} />
        <Kpi n={o.totals.votes_open} label="in vote" sub="suggestions + amendments" tone={o.totals.votes_open ? "warn" : "neutral"} />
        <Kpi n={o.totals.questions_open} label="open questions" sub="across the boards" />
        <Kpi n={o.totals.posts_24h} label="posts / 24h" sub={num(o.totals.posts) + " all time"} spark={undefined} />
        <Kpi n={r ? (r.daemon.state || "off") : "…"} label="daemon" sub={r ? (r.runner?.allowed === false ? r.runner.reason : num(r.jobs.running) + " running · " + num(r.jobs.queued) + " queued") : "loading"} tone={r ? (r.daemon.state === "running" || r.daemon.state === "alive" ? "ok" : "warn") : "neutral"} href={hrefs.agents()} />
      </KpiGrid>

      {awaiting.length ? (
        <section id="for-you" className="mb-4">
          <h2 className="mb-2 text-sm font-semibold">For you - {plural(awaiting.length, "proposal")} waiting for a yes or no</h2>
          <div className="grid gap-3 lg:grid-cols-2">
            {awaiting.map((p) => (
              <ProposalCard key={p.id} r={p} slug={p.slug || o.totals.brain} canDecide={o.can_decide} />
            ))}
          </div>
        </section>
      ) : null}

      <div className="grid gap-4 xl:grid-cols-[minmax(0,2fr)_minmax(320px,1fr)]">
        <div className="grid gap-4">
          <section>
            <h2 className="mb-2 text-sm font-semibold">Projects</h2>
            <div className="grid gap-3 md:grid-cols-2">
              {projects.map((p) => (
                <ProjectCard key={p.slug} p={p} spark={sparkFor(p.slug)} />
              ))}
            </div>
          </section>
          {tl.data ? <ActivityChart tl={tl.data} bys={["total", "project", "kind"]} /> : null}
          <Panel title="Recent tasks" actions={<Link href={hrefs.tasks()} className="hover:underline">all</Link>} dense>
            {tasks.data ? (
              openTasks.length ? (
                <ul className="divide-y">
                  {openTasks
                    .sort((a, b) => (a.updated_at < b.updated_at ? 1 : -1))
                    .slice(0, 8)
                    .map((t: Task) => (
                      <li key={t.id} className="flex flex-wrap items-center gap-2 px-3 py-2 text-[13px]">
                        <Link href={hrefs.task(t.project, t.id)} className="font-mono text-xs hover:underline">
                          {t.id}
                        </Link>
                        <Link href={hrefs.task(t.project, t.id)} className="min-w-0 flex-1 truncate font-medium hover:underline">
                          {t.title}
                        </Link>
                        <StatusBadge status={t.status} />
                        <ProjectLink slug={t.project} className="text-xs" />
                        <When d={t.updated_at} className="text-xs" />
                      </li>
                    ))}
                </ul>
              ) : (
                <EmptyState className="m-3">No open tasks. Give the first one with New task - the project&apos;s lead breaks it into jobs and reports back in its chat.</EmptyState>
              )
            ) : tasks.error ? (
              <div className="p-3">
                <ErrorNotice error={tasks.error} onRetry={() => void tasks.reload()} />
              </div>
            ) : (
              <div className="p-3 text-xs text-muted-foreground">Loading…</div>
            )}
          </Panel>
        </div>
        <div className="grid gap-4 content-start">
          <LiveFeed limit={40} height={420} />
          <MachinePanel r={r} />
          <WorkingNow claims={o.active_claims} />
          {attention.length ? (
            <Panel title="Needs attention">
              <ul className="grid gap-1.5 text-[13px]">
                {attention.map((p) => (
                  <li key={p.slug} className="anywhere">
                    <ProjectLink slug={p.slug} />{" "}
                    <span className="text-muted-foreground">{p.stop_present ? "STOP is set - the evolution loop halts until it is removed." : p.missing || !p.present ? "is registered but missing on disk - re-clone it at " + p.path + " or unregister." : !p.board_present ? "has no board yet - run pulse init in its checkout." : p.error}</span>
                  </li>
                ))}
              </ul>
            </Panel>
          ) : null}
        </div>
      </div>
    </>
  );
}
