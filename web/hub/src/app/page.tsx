"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useState } from "react";
import { ActivityIcon, BotIcon, CheckIcon, CpuIcon, FolderIcon, HomeIcon, InboxIcon, ListTodoIcon, MessageCircleQuestionIcon, MessageSquareIcon, PowerIcon, UsersIcon, VoteIcon, ExternalLinkIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ActivityChart, lastDays } from "@/components/pulse/charts";
import { CommonsCard, MachinePanel, ProjectCard, ProposalCard, WorkingNow, workingRows, workingSummary } from "@/components/pulse/cards";
import { ComputeDecide } from "@/components/compute/compute";
import { GateCard, foldSteps, gateStep, type GateStep } from "@/components/pulse/evolve";
import { CardGrid, ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardMeta, ItemCardTitle, SectionHeading } from "@/components/pulse/item-card";
import { LiveFeed } from "@/components/pulse/live-feed";
import { EmptyState, ErrorNotice, Kpi, KpiGrid, Notice, PageTitle, Panel, SkeletonPage, SkeletonRows } from "@/components/pulse/panel";
import { Mono, ProjectLink, StatusBadge, ToneBadge, When, Who } from "@/components/pulse/who";
import { AddProjectButton } from "@/components/project/project-dialogs";
import { awaitsDecision, ProposedRecordCard } from "@/components/records/decide-record";
import { RecordSheet } from "@/components/records/records";
import { TaskCards } from "@/components/tasks/task-card";
import { NewTaskButton } from "@/components/tasks/task-dialogs";
import { hrefs } from "@/lib/nav";
import { num, plural } from "@/lib/format";
import { evolveGate, registeredProjects, type Compute, type ComputeCaps, type ComputeRequest, type Org, type ProjectCardData, type ProjectsPayload, type RecordRow, type RecordsPayload, type Runner, type TasksPayload, type Timeline } from "@/lib/api";
import { useApi } from "@/lib/use-api";

export default function HubPage() {
  const router = useRouter();
  const org = useApi<Org>("org");
  const tl = useApi<Timeline>("timeline", { refresh: 60000 });
  const runner = useApi<Runner>("runner");
  const tasks = useApi<TasksPayload>("tasks");
  const compute = useApi<Compute>("compute", { refresh: 30000 });
  // Proposed ADRs across every project: the decisions that wait for the owner (ADR-028).
  const proposed = useApi<RecordsPayload>("records?kind=adr&status=Proposed", { refresh: 60000 });
  // The registry rows carry the self-improvement gates when the org rows do not (ADR-029).
  const reg = useApi<ProjectsPayload>("projects", { refresh: 60000 });
  const [openRecord, setOpenRecord] = useState<RecordRow | null>(null);

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
  // The commons is not a project: it gets its own card below the grid.
  const projects = registeredProjects(o);
  const commons = o.commons;
  const awaiting = o.proposals.filter((p) => p.status === "awaiting");
  const openTasks = (tasks.data?.tasks ?? []).filter((t) => t.status !== "done" && t.status !== "cancelled");
  const working = workingRows(o.active_claims, openTasks);
  const recent = [...openTasks].sort((a, b) => (a.updated_at < b.updated_at ? 1 : -1)).slice(0, 6);
  const pendingCompute = (compute.data?.requests ?? []).filter((r) => r.status === "pending");
  const adrs = (proposed.data?.records ?? []).filter(awaitsDecision);
  // The gate a project waits at (records, fixtures, the owner's held-out fixtures) is a human step too - it sits in the inbox with a command to copy (ADR-029). The baseline is the daemon's (ADR-031) and never a card.
  const gateOf = (p: ProjectCardData) => evolveGate(p, (reg.data?.projects ?? []).find((r) => r.slug === p.slug));
  const steps = foldSteps(projects.map((p) => gateStep(p.slug, gateOf(p))).filter((s): s is GateStep => s !== null));
  const decisions = adrs.length + awaiting.length + pendingCompute.length;
  const inbox = decisions + steps.length;
  const computeDown = !!compute.error && !compute.data;
  const decisionsDown = !!proposed.error && !proposed.data;
  const down = [decisionsDown ? "decisions could not be loaded" : "", computeDown ? "compute requests could not be loaded" : ""].filter(Boolean);
  const downWhy = [decisionsDown ? "decisions could not be loaded (" + proposed.error + ")" : "", computeDown ? "compute requests could not be loaded (" + compute.error + ")" : ""].filter(Boolean);
  const checking = [!proposed.data && !decisionsDown ? "decisions" : "", !compute.data && !computeDown ? "compute requests" : ""].filter(Boolean);
  const attention = projects.filter((p) => p.error || p.missing || p.stop_present || !p.board_present);
  const r = runner.data;
  const daemonOn = !!r && (r.daemon.state === "running" || r.daemon.state === "alive");
  const sparkFor = (slug: string): number[] | undefined => {
    const s = tl.data?.series?.find((x) => x.slug === slug);
    return s ? lastDays(s, 14) : undefined;
  };
  const metaChipBase = "items-center gap-1 whitespace-nowrap rounded-full border bg-card px-2 py-0.5 text-xs text-muted-foreground";
  const metaChip = "inline-flex " + metaChipBase;

  return (
    <>
      <PageTitle
        icon={HomeIcon}
        badge={
          inbox > 0 ? (
            <a href="#for-you" className="inline-flex items-center rounded-full bg-primary px-2 py-0.5 text-[11.5px] font-medium text-primary-foreground shadow-card hover:bg-primary/90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
              {num(inbox) + " for you"}
            </a>
          ) : null
        }
        sub={
          <span className="flex flex-wrap items-center gap-1.5">
            <span className={metaChip}>{plural(o.totals.projects, "project")}</span>
            {o.totals.brain ? (
              <span className={metaChip}>
                brain <Mono>{o.totals.brain}</Mono>
              </span>
            ) : null}
            <span className={metaChip}>
              tally <Mono>{o.tally_mode}</Mono>
            </span>
            {o.binary_version ? (
              <span className={metaChip}>
                pulse <Mono>{o.binary_version}</Mono>
              </span>
            ) : null}
            <span title={o.store || o.registry} className={"hidden min-w-0 max-w-[min(42vw,28rem)] sm:inline-flex " + metaChipBase}>
              store <Mono className="min-w-0 truncate text-muted-foreground">{o.store || o.registry}</Mono>
            </span>
          </span>
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
      {o.owner_presence === "away" ? (
        <Notice tone="warn" className="mb-3">
          You&apos;re marked away — each project&apos;s lead decides proposals on your behalf. Hard stops, ADRs and compute requests still wait for you.{" "}
          <Link href={hrefs.settings("owner")} className="font-medium underline underline-offset-2">
            Settings
          </Link>
        </Notice>
      ) : null}

      {/* The inbox: everything that waits for the owner's yes or no, buttons in reach, before anything else. Decisions first, then proposals, then compute, then the steps only the owner can take. */}
      {inbox ? (
        <section id="for-you" className="mb-4" aria-label="For you">
          <SectionHeading icon={InboxIcon} count={inbox} action={down.length ? <span className="text-warn">{down.join(" · ")}</span> : <span className="text-right">{decisions && steps.length ? "your yes or no, and steps only you can take" : steps.length ? "the steps only you can take" : "waiting for your yes or no"}</span>}>
            For you
          </SectionHeading>
          <div className="rounded-xl border border-primary/25 surface-tint p-3 shadow-card sm:p-4">
            <CardGrid cols={2}>
              {adrs.map((r) => (
                <ProposedRecordCard key={r.project + "/" + r.id} r={r} onOpen={setOpenRecord} />
              ))}
              {awaiting.map((p) => (
                <ProposalCard key={p.id} r={p} slug={p.slug || o.totals.brain} canDecide={o.can_decide} />
              ))}
              {pendingCompute.map((cr) => (
                <ComputeInboxCard key={cr.id} r={cr} caps={compute.data?.caps} />
              ))}
              {steps.map((s) => (
                <GateCard key={s.slug + "/" + s.kind} step={s} />
              ))}
            </CardGrid>
          </div>
        </section>
      ) : (
        <p id="for-you" className="mb-4 flex items-center gap-2 rounded-lg border bg-card px-3.5 py-2.5 text-sm text-muted-foreground">
          <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-ok/10 text-ok" aria-hidden>
            <CheckIcon className="size-3.5" />
          </span>
          <span>{downWhy.length ? "No proposal waits for you - " + downWhy.join("; ") + "." : checking.length ? "No proposal waits for you - checking " + checking.join(" and ") + "…" : "Nothing waits for you right now."}</span>
        </p>
      )}

      <KpiGrid>
        <Kpi icon={InboxIcon} n={inbox} label="for you" sub={inbox ? [adrs.length ? plural(adrs.length, "decision") : "", awaiting.length ? plural(awaiting.length, "proposal") : "", pendingCompute.length ? plural(pendingCompute.length, "request") : "", steps.length ? plural(steps.length, "step") : ""].filter(Boolean).join(" · ") : "nothing waits"} tone={inbox ? "ok" : "neutral"} href="#for-you" />
        <Kpi icon={ListTodoIcon} n={openTasks.length} label="open tasks" sub={tasks.data ? `${num(openTasks.filter((t) => t.status === "in_progress").length)} in progress` : "loading"} href={hrefs.tasks()} />
        <Kpi icon={CpuIcon} n={pendingCompute.length} label="compute" sub={pendingCompute.length ? "requests to decide" : compute.data ? num(compute.data.caps.vms_now) + " VMs up" : "loading"} tone={pendingCompute.length ? "warn" : "neutral"} href={hrefs.compute()} />
        <Kpi icon={BotIcon} n={o.totals.agents_active} label="agents active" sub={num(o.totals.agents) + " known"} href={hrefs.agents()} />
        <Kpi icon={VoteIcon} n={o.totals.votes_open} label="in vote" sub="suggestions + amendments" tone={o.totals.votes_open ? "warn" : "neutral"} href={hrefs.votes()} />
        <Kpi icon={MessageCircleQuestionIcon} n={o.totals.questions_open} label="open questions" sub="across the boards" />
        <Kpi icon={MessageSquareIcon} n={o.totals.posts_24h} label="posts / 24h" sub={num(o.totals.posts) + " all time"} spark={undefined} />
        <Kpi icon={daemonOn ? PowerIcon : ActivityIcon} n={r ? (daemonOn ? "on" : "off") : "…"} label="daemon" sub={r ? (r.runner?.allowed === false ? r.runner.reason : (r.daemon.state || "off") + " · " + num(r.jobs.running) + " running · " + num(r.jobs.queued) + " queued") : "loading"} tone={r ? (daemonOn ? "ok" : "warn") : "neutral"} href={hrefs.agents()} />
      </KpiGrid>

      <div className="grid gap-4 xl:grid-cols-[minmax(0,2fr)_minmax(320px,1fr)]">
        <div className="grid min-w-0 grid-cols-1 content-start gap-6">
          <section aria-label="Projects">
            <SectionHeading icon={FolderIcon} count={projects.length} action={<AddProjectButton size="sm" variant="outline" onRegistered={(r) => router.push(hrefs.project(r.project.slug))} />}>
              Projects
            </SectionHeading>
            {projects.length ? (
              <CardGrid cols={2}>
                {projects.map((p) => (
                  <ProjectCard key={p.slug} p={p} spark={sparkFor(p.slug)} gate={gateOf(p)} />
                ))}
              </CardGrid>
            ) : (
              <EmptyState>
                No project registered yet. Add one with the button above or run <Mono>pulse init</Mono> in a checkout.
              </EmptyState>
            )}
          </section>
          {commons ? (
            <section aria-label="Commons">
              <SectionHeading icon={UsersIcon} action={<span>not a project - the board every project shares</span>}>Commons</SectionHeading>
              <CommonsCard c={commons} spark={sparkFor(commons.slug)} />
            </section>
          ) : null}
          <section aria-label="Open tasks">
            <SectionHeading
              icon={ListTodoIcon}
              count={tasks.data ? openTasks.length : undefined}
              action={
                <Link href={hrefs.tasks()} className="hover:underline">
                  all
                </Link>
              }
            >
              Open tasks
            </SectionHeading>
            {tasks.data ? (
              // onOpen (not href): the card carries a project link and quick actions, which must not sit inside an anchor.
              <TaskCards tasks={recent} showProject compact cols={2} empty="No open tasks. Give the first one with New task - the project's lead breaks it into jobs and reports back in its chat." />
            ) : tasks.error ? (
              <ErrorNotice error={tasks.error} onRetry={() => void tasks.reload()} />
            ) : (
              <SkeletonRows n={2} />
            )}
          </section>
          {tl.data ? <ActivityChart tl={tl.data} bys={["total", "project", "kind"]} height={180} /> : null}
        </div>
        <div className="grid min-w-0 grid-cols-1 content-start gap-4">
          <LiveFeed limit={40} height={320} />
          <MachinePanel r={r} />
          <Panel title="Working now" icon={ActivityIcon} actions={<span className="tnum">{workingSummary(working, o.active_claims.length)}</span>}>
            <WorkingNow claims={o.active_claims} tasks={openTasks} />
          </Panel>
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
      {/* Read on an inbox card opens the record here; its header carries the same three answers. */}
      <RecordSheet id={openRecord?.id ?? null} project={openRecord?.project} onClose={() => setOpenRecord(null)} />
    </>
  );
}

// One pending compute request as an inbox card: who asks for what, for how
// long and why, with Approve / Deny in reach (ComputeDecide confirms each).
function ComputeInboxCard({ r, caps }: { r: ComputeRequest; caps?: ComputeCaps }) {
  return (
    <ItemCard highlight ariaLabel={"Compute request " + r.id}>
      <ItemCardHeader
        right={
          <>
            <Who row={{ agent: r.requested_by }} compact />
            <When d={r.at} />
          </>
        }
      >
        <Mono className="font-semibold">{r.id}</Mono>
        <StatusBadge status={r.status} />
        <ToneBadge tone="info">compute</ToneBadge>
        <ProjectLink slug={r.project} className="text-xs" />
      </ItemCardHeader>
      <ItemCardTitle>
        {r.requested_by + " asks for "}
        <Mono>{r.size}</Mono>
        {" for " + plural(r.hours, "hour")}
      </ItemCardTitle>
      <ItemCardBody clamp={3}>{r.reason}</ItemCardBody>
      {caps ? (
        <ItemCardMeta>
          <span className="tnum">{num(caps.vms_now) + " of " + num(caps.max_vms) + " VMs up"}</span>
          <span className="tnum">{num(caps.hours_today) + " of " + num(caps.max_hours_per_day) + " h today"}</span>
          <span className="tnum">{"budget " + num(caps.budget_credits) + " credits"}</span>
          {caps.provider ? <Mono>{caps.provider}</Mono> : null}
        </ItemCardMeta>
      ) : null}
      <ItemCardActions>
        <Button asChild variant="ghost" size="xs" className="text-muted-foreground">
          <Link href={hrefs.compute()}>
            all requests <ExternalLinkIcon />
          </Link>
        </Button>
        <span className="ml-auto shrink-0">
          <ComputeDecide r={r} />
        </span>
      </ItemCardActions>
    </ItemCard>
  );
}
