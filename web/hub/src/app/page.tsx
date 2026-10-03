"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useEffect, useState } from "react";
import { ActivityIcon, ArrowUpRightIcon, BotIcon, ChevronDownIcon, ChevronRightIcon, CpuIcon, FolderIcon, HomeIcon, InboxIcon, ListTodoIcon, MessageCircleQuestionIcon, MessageSquareIcon, PowerIcon, VoteIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { CodeDeliveredKpi } from "@/components/code-delivery/report";
import { ActivityChart, lastDays } from "@/components/pulse/charts";
import { HomeHero } from "@/components/home/hero";
import { CommonsLine, commonsLive, MachinePanel, ProjectCard, ProposalCard, QuestionCard, WorkingNow, workingRows, workingSummary } from "@/components/pulse/cards";
import { ComputeInboxCard } from "@/components/compute/compute";
import { DaemonControls } from "@/components/pulse/daemon-controls";
import { GateCard } from "@/components/pulse/evolve";
import { inboxBreakdown, useInbox } from "@/components/pulse/inbox";
import { CardGrid, ItemCard, ItemCardBody, ItemCardHeader, ItemCardMeta, ItemCardTitle, SectionHeading } from "@/components/pulse/item-card";
import { LiveFeed } from "@/components/pulse/live-feed";
import { FailedJobsSection } from "@/components/pulse/job-repair-section";
import { EmptyState, ErrorNotice, Kpi, KpiGrid, Kv, Notice, PageTitle, Panel, SkeletonPage, SkeletonRows } from "@/components/pulse/panel";
import { Mono, ProjectLink, ToneBadge } from "@/components/pulse/who";
import { AddProjectButton } from "@/components/project/project-dialogs";
import { ProposedRecordCard } from "@/components/records/decide-record";
import { DraftCard } from "@/components/records/draft-panel";
import { RecordSheet } from "@/components/records/records";
import { TaskCards } from "@/components/tasks/task-card";
import { NewTaskButton } from "@/components/tasks/task-dialogs";
import { hrefs } from "@/lib/nav";
import { num, plural } from "@/lib/format";
import { registeredProjects, type Compute, type Org, type RecordRow, type Runner, type ServicePayload, type SettingsPayload, type TasksPayload, type Timeline } from "@/lib/api";
import { useApi } from "@/lib/use-api";

export default function HubPage() {
  const router = useRouter();
  const org = useApi<Org>("org");
  const tl = useApi<Timeline>("timeline", { refresh: 60000 });
  const runner = useApi<Runner>("runner");
  const tasks = useApi<TasksPayload>("tasks");
  const compute = useApi<Compute>("compute");
  // Whether this machine keeps PULSE alive yet (ADR-038): the one step a shared copy asks for.
  const service = useApi<ServicePayload>("service", { refresh: 60000 });
  const settings = useApi<SettingsPayload>("settings");
  // Everything that waits for the owner's yes or no - the same count the sidebar and the project pages show.
  const forYou = useInbox();
  const [openRecord, setOpenRecord] = useState<RecordRow | null>(null);
  const [allQuestions, setAllQuestions] = useState(false);
  const [detailsOpen, setDetailsOpen] = useState(false);

  useEffect(() => {
    const openFromHash = () => {
      if (window.location.hash === "#daemon-controls") {
        setDetailsOpen(true);
        window.localStorage.setItem("pulse.home.details", "open");
        window.requestAnimationFrame(() => document.getElementById("daemon-controls")?.scrollIntoView());
      }
    };
    if (window.localStorage.getItem("pulse.home.details") === "open") window.requestAnimationFrame(() => setDetailsOpen(true));
    openFromHash();
    window.addEventListener("hashchange", openFromHash);
    return () => window.removeEventListener("hashchange", openFromHash);
  }, []);


  if (org.error && !org.data)
    return (
      <>
        <PageTitle>Home</PageTitle>
        <ErrorNotice error={org.error} onRetry={() => void org.reload()}>
          The hub API did not answer ({org.error}). Start it with <Mono>pulse hub</Mono> and reload.
        </ErrorNotice>
      </>
    );
  if (!org.data) return <SkeletonPage rows={4} />;

  const o = org.data;
  // The shared board is not a project: it gets one row under the grid, and
  // only while something is live on it.
  const projects = registeredProjects(o);
  const commons = o.commons;
  const { decisions: adrs, proposals: awaiting, questions, compute: pendingCompute, drafts, steps, machine: machineStep, total: inbox, daemon, checking, down, downWhy, gateOf } = forYou;
  // Boards can hold many old owner questions; the inbox shows the first few and unfolds on request.
  const QUESTIONS_SHOWN = 6;
  const shownQuestions = allQuestions ? questions : questions.slice(0, QUESTIONS_SHOWN);
  const openTasks = (tasks.data?.tasks ?? []).filter((t) => t.status !== "done" && t.status !== "cancelled");
  const working = workingRows(o.active_claims, openTasks);
  const recent = [...openTasks].sort((a, b) => (a.updated_at < b.updated_at ? 1 : -1)).slice(0, 6);
  const attention = projects.filter((p) => p.error || p.missing || p.stop_present || !p.board_present);
  const r = runner.data;
  const sparkFor = (slug: string): number[] | undefined => {
    const s = tl.data?.series?.find((x) => x.slug === slug);
    return s ? lastDays(s, 14) : undefined;
  };
  const runnerChecking = !runner.data && !runner.error;
  const fallbackWorkingAgents = new Set(o.active_claims.map((c) => c.agent || c.name || c.session || c.id).filter(Boolean)).size;
  const setDetails = (open: boolean) => {
    setDetailsOpen(open);
    window.localStorage.setItem("pulse.home.details", open ? "open" : "closed");
  };

  return (
    <>
      <HomeHero
        inbox={inbox}
        daemon={daemon}
        daemonDown={!!runner.error && !runner.data}
        runnerChecking={runnerChecking}
        inboxChecking={checking.length > 0}
        runningJobs={r?.jobs.running}
        fallbackWorkingAgents={fallbackWorkingAgents}
        activeAgents={o.totals.agents_active}
        projects={projects.length}
        ownerAway={o.owner_presence === "away"}
        actions={<NewTaskButton />}
      />

      {inbox ? (
        <section id="for-you" className="mb-8 scroll-mt-32 rounded-2xl surface-raised glossy p-4 shadow-card sm:p-5" aria-label="For you">
          <SectionHeading icon={InboxIcon} count={inbox} action={down.length ? <span className="text-warn">{down.join(" · ")}</span> : <span>Your yes or no — nothing happens without it.</span>}>
            For you
          </SectionHeading>
          <CardGrid cols={2}>
            {adrs.map((r) => (
              <ProposedRecordCard key={r.project + "/" + r.id} r={r} onOpen={setOpenRecord} />
            ))}
            {awaiting.map((p) => (
              <ProposalCard key={p.id} r={p} slug={p.slug || o.totals.brain} canDecide={o.can_decide} />
            ))}
            {shownQuestions.map((q) => (
              <QuestionCard key={(q.slug || "") + "/" + q.id} q={q} slug={q.slug || o.totals.brain} canDecide={o.can_decide} />
            ))}
            {questions.length > QUESTIONS_SHOWN ? (
              <button type="button" className="flex min-h-11 items-center justify-center rounded-xl border border-dashed border-border/80 px-4 text-sm text-muted-foreground transition-colors hover:border-border hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none" onClick={() => setAllQuestions((v) => !v)} aria-expanded={allQuestions}>
                {allQuestions ? "show fewer questions" : "show all " + plural(questions.length, "question") + " for you"}
              </button>
            ) : null}
            {pendingCompute.map((cr) => (
              <ComputeInboxCard key={cr.id} r={cr} caps={compute.data?.caps} />
            ))}
            {drafts.map(({ slug, r }) => (
              <DraftCard key={slug + "/" + r.id} slug={slug} r={r} />
            ))}
            {machineStep ? (
              <ItemCard href="#daemon-controls" highlight ariaLabel="Review daemon controls">
                <ItemCardHeader><ToneBadge tone={daemon.tone}>{daemon.label}</ToneBadge><ToneBadge tone="neutral">this machine</ToneBadge></ItemCardHeader>
                <ItemCardTitle chevron>{daemon.pause === "on" ? "Resume daemon" : daemon.setup === "start" ? "Start daemon" : "Keep PULSE running"}</ItemCardTitle>
                <ItemCardBody clamp={0}>Review the owner pause, heartbeat and service setup together. Preview and confirm any change in the daemon controls below.</ItemCardBody>
                <ItemCardMeta>Review daemon controls <ArrowUpRightIcon className="size-3.5" /></ItemCardMeta>
              </ItemCard>
            ) : null}
            {steps.map((s) => (
              <GateCard key={s.slug + "/" + s.kind} step={s} />
            ))}
          </CardGrid>
        </section>
      ) : (
        <p id="for-you" className="mb-8 flex scroll-mt-32 items-center gap-3 rounded-xl surface-glass px-4 py-3 text-sm leading-relaxed text-muted-foreground">
          <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground" aria-hidden>
            <InboxIcon className="size-4" />
          </span>
          <span className="min-w-0 anywhere">{downWhy.length ? "Nothing waits for you — " + downWhy.join("; ") + "." : checking.length ? "Nothing waits for you — checking " + checking.join(" and ") + "…" : "Nothing waits for you right now."}</span>
        </p>
      )}

      <section aria-label="Your projects" className="mb-8">
        <SectionHeading icon={FolderIcon} count={projects.length} action={<AddProjectButton size="sm" variant="outline" onRegistered={(r) => router.push(hrefs.project(r.project.slug))} />}>
          Your projects
        </SectionHeading>
        {projects.length ? (
          <CardGrid cols={2}>
            {projects.map((p) => (
              <ProjectCard key={p.slug} p={p} spark={sparkFor(p.slug)} gate={gateOf(p)} forYou={forYou.forProject(p.slug).total} />
            ))}
          </CardGrid>
        ) : (
          <EmptyState icon={<FolderIcon className="size-[18px]" />} title="A place for your projects">
            No project registered yet. Add one with the button above or run <Mono>pulse init</Mono> in a checkout.
          </EmptyState>
        )}
        {attention.length ? (
          <div className="mt-4 grid gap-2" aria-label="Needs attention">
            {attention.map((p) => (
              <Notice key={p.slug} tone="warn">
                <ProjectLink slug={p.slug} />{" "}
                <span className="text-muted-foreground">{p.stop_present ? "STOP is set - the evolution loop halts until it is removed." : p.missing || !p.present ? "is registered but missing on disk - re-clone it at " + p.path + " or unregister." : !p.board_present ? "has no board yet - run pulse init in its checkout." : p.error}</span>
              </Notice>
            ))}
          </div>
        ) : null}
        {commons && commonsLive(commons) ? <CommonsLine c={commons} className="mt-4" /> : null}
      </section>

      <section aria-label="What's happening" className="mb-8">
        <SectionHeading icon={ActivityIcon}>What&apos;s happening</SectionHeading>
        <div className="grid min-w-0 gap-5 xl:grid-cols-2">
          <Panel title="Working now" icon={ActivityIcon} actions={<span className="tnum">{workingSummary(working, o.active_claims.length)}</span>} className="surface-raised">
            <WorkingNow claims={o.active_claims} tasks={openTasks} />
          </Panel>
          <Panel
            title="Recent open tasks"
            icon={ListTodoIcon}
            actions={
              <Button asChild variant="ghost" size="sm">
                <Link href={hrefs.tasks()}>
                  View all <ArrowUpRightIcon />
                </Link>
              </Button>
            }
            className="surface-raised"
          >
            {tasks.data ? (
              <TaskCards tasks={recent} showProject compact cols={2} empty="No open tasks. Give the first one with New task - the project's lead breaks it into jobs and reports back in its chat." />
            ) : tasks.error ? (
              <ErrorNotice error={tasks.error} onRetry={() => void tasks.reload()} />
            ) : (
              <SkeletonRows n={2} />
            )}
          </Panel>
        </div>
        <div className="mt-5">{tl.data ? <ActivityChart tl={tl.data} bys={["total", "project", "kind"]} height={180} /> : null}</div>
      </section>

      <section aria-label="Details for this machine" className="mb-8">
        <div className="rounded-2xl surface-raised glossy p-4 shadow-card sm:p-5">
          <button
            type="button"
            className="flex w-full min-w-0 items-center justify-between gap-3 text-left text-sm font-semibold"
            aria-expanded={detailsOpen}
            aria-controls="pulse-home-details"
            onClick={() => setDetails(!detailsOpen)}
          >
            <span className="flex min-w-0 items-center gap-2">
              {detailsOpen ? <ChevronDownIcon className="size-4 text-muted-foreground" aria-hidden /> : <ChevronRightIcon className="size-4 text-muted-foreground" aria-hidden />}
              Details for this machine
            </span>
            <span className="text-xs font-normal text-muted-foreground">{detailsOpen ? "Hide details" : "Show details"}</span>
          </button>
          {detailsOpen ? (
            <div id="pulse-home-details" className="mt-5 grid gap-6">
              <section aria-label="Full status">
                <SectionHeading>At a glance</SectionHeading>
                <KpiGrid className="mb-0">
                  <Kpi icon={InboxIcon} n={inbox} label="For you" sub={inboxBreakdown(forYou)} tone={inbox ? "ok" : "neutral"} href="#for-you" />
                  <Kpi icon={ListTodoIcon} n={openTasks.length} label="Open tasks" sub={tasks.data ? `${num(openTasks.filter((t) => t.status === "in_progress").length)} in progress` : "loading"} href={hrefs.tasks()} />
                  <Kpi icon={CpuIcon} n={pendingCompute.length} label="Compute" sub={pendingCompute.length ? "requests to decide" : compute.data ? num(compute.data.caps.vms_now) + " VMs up" : "loading"} tone={pendingCompute.length ? "warn" : "neutral"} href={hrefs.compute()} />
                  <Kpi icon={BotIcon} n={o.totals.agents_active} label="Agents active" sub={num(o.totals.agents) + " known"} href={hrefs.agents()} />
                  <Kpi icon={VoteIcon} n={o.totals.votes_open} label="In vote" sub="suggestions + amendments" tone={o.totals.votes_open ? "warn" : "neutral"} href={hrefs.votes()} />
                  <CodeDeliveredKpi />
                  <Kpi icon={MessageCircleQuestionIcon} n={o.totals.questions_open} label="Open questions" sub="across the boards" />
                  <Kpi icon={MessageSquareIcon} n={o.totals.posts_24h} label="Posts / 24h" sub={num(o.totals.posts) + " all time"} spark={undefined} />
                  <Kpi icon={daemon.tone === "ok" ? PowerIcon : ActivityIcon} n={daemon.label} label="Daemon" sub={runner.error ? "status unavailable · controls below" : r ? (r.runner?.allowed === false ? r.runner.reason : num(r.jobs.running) + " running · " + num(r.jobs.queued) + " queued · controls below") : "checking heartbeat"} tone={daemon.tone} href="#daemon-controls" />
                </KpiGrid>
              </section>

              <section id="daemon-controls" aria-label="Daemon controls" className="scroll-mt-32">
                <DaemonControls runner={runner} service={service} settings={settings} />
              </section>

              <FailedJobsSection />

              <div className="grid min-w-0 gap-5 xl:grid-cols-2">
                <MachinePanel r={r} />
                <LiveFeed limit={40} height={320} />
              </div>

              <Panel title="About this hub" icon={HomeIcon}>
                <Kv
                  rows={[
                    ["projects", plural(o.totals.projects, "project")],
                    ["brain", o.totals.brain ? <Mono key="brain">{o.totals.brain}</Mono> : "not set"],
                    ["tally", <Mono key="tally">{o.tally_mode}</Mono>],
                    ["pulse", o.binary_version ? <Mono key="pulse">{o.binary_version}</Mono> : "unknown"],
                    ["store", <Mono key="store">{o.store || o.registry}</Mono>],
                  ]}
                />
              </Panel>
            </div>
          ) : null}
        </div>
      </section>

      {/* Read on an inbox card opens the record here; its header carries the same three answers. */}
      <RecordSheet id={openRecord?.id ?? null} project={openRecord?.project} onClose={() => setOpenRecord(null)} />
    </>
  );
}
