"use client";

import Link from "next/link";
import { Suspense, useCallback } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { Badge } from "@/components/ui/badge";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { PageTitle, EmptyState, SkeletonPage } from "@/components/pulse/panel";
import { Mono, ToneBadge } from "@/components/pulse/who";
import { PostSheet, ThreadSheet } from "@/components/board/sheets";
import { TaskSheet } from "@/components/tasks/task-sheet";
import { NewTaskButton } from "@/components/tasks/task-dialogs";
import { AmendmentsTab, ChatTab, EnvTab, EvolutionTab, NoticesTab, OverviewTab, ProposalsTab, SuggestionsTab, TasksTab, ThreadsTab, TimelineTab } from "@/components/project/tabs";
import { hrefs, isProjectTab, PROJECT_TABS, type ProjectTab } from "@/lib/nav";
import { allProjects, type Org } from "@/lib/api";
import { useApi } from "@/lib/use-api";

export default function ProjectPage() {
  return (
    <Suspense fallback={<SkeletonPage rows={4} />}>
      <ProjectScreen />
    </Suspense>
  );
}

function ProjectScreen() {
  const sp = useSearchParams();
  const router = useRouter();
  const pathname = usePathname();
  const slug = sp.get("slug") || "";
  const tabParam = sp.get("tab");
  const tab: ProjectTab = isProjectTab(tabParam) ? tabParam : "overview";
  const thread = sp.get("thread");
  const post = sp.get("post");
  const task = sp.get("task");
  const org = useApi<Org>("org");

  const setParams = useCallback(
    (patch: Record<string, string | null | undefined>) => {
      const q = new URLSearchParams(sp.toString());
      for (const [k, v] of Object.entries(patch)) {
        if (v === null || v === undefined || v === "") q.delete(k);
        else q.set(k, v);
      }
      const s = q.toString();
      router.push(pathname + (s ? "?" + s : ""), { scroll: false });
    },
    [sp, router, pathname],
  );

  if (!slug) {
    const projects = allProjects(org.data);
    return (
      <>
        <PageTitle>Pick a project</PageTitle>
        {!org.data && org.loading ? (
          <SkeletonPage rows={2} />
        ) : projects.length ? (
          <ul className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
            {projects.map((p) => (
              <li key={p.slug}>
                <Link href={hrefs.project(p.slug)} className="block rounded-lg border bg-card p-3 hover:bg-muted/40">
                  <span className="font-semibold">{p.slug}</span> {p.brain ? <Badge variant="secondary">framework</Badge> : null}
                  <div className="text-xs text-muted-foreground">{p.name}</div>
                </Link>
              </li>
            ))}
          </ul>
        ) : (
          <EmptyState>No project registered yet. Run <Mono>pulse init</Mono> in a checkout; the hub lists it here.</EmptyState>
        )}
      </>
    );
  }

  const card = allProjects(org.data).find((p) => p.slug === slug);
  const canDecide = org.data?.can_decide ?? true;
  const counts: Partial<Record<ProjectTab, number>> = card
    ? {
        proposals: card.proposals_awaiting,
        suggestions: card.suggestions_needing_votes + card.suggestions_eligible,
        amendments: card.amendments_open,
        threads: card.threads_open,
      }
    : {};

  return (
    <>
      <PageTitle
        crumb={
          <>
            <Link href={hrefs.home()} className="hover:underline">
              Hub
            </Link>{" "}
            / project
          </>
        }
        sub={
          card ? (
            <>
              {card.name && card.name !== card.slug ? card.name + " · " : null}
              <Mono>{card.branch}</Mono> <Mono className="text-muted-foreground">{card.head}</Mono>
              {card.dirty ? <span className="ml-1 text-warn">dirty</span> : null}
              {card.path ? <span className="ml-2 text-muted-foreground">{card.path}</span> : null}
            </>
          ) : (
            org.error || "…"
          )
        }
        actions={<NewTaskButton project={slug} onCreated={(t) => setParams({ tab: "tasks", task: t.id })} />}
      >
        <span className="inline-flex flex-wrap items-center gap-2">
          {slug}
          {card?.brain ? <Badge variant="secondary">framework</Badge> : null}
          {card?.kind === "commons" ? <Badge variant="outline">commons</Badge> : null}
          {card?.stop_present ? <ToneBadge tone="bad">STOP</ToneBadge> : null}
          {card && !card.present ? <Badge variant="destructive">missing on disk</Badge> : null}
        </span>
      </PageTitle>

      <Tabs value={tab} onValueChange={(v) => setParams({ tab: v === "overview" ? null : v, thread: null, post: null })} className="gap-4">
        <div className="-mx-1 overflow-x-auto px-1">
          <TabsList variant="line" className="w-max">
            {PROJECT_TABS.map((t) => (
              <TabsTrigger key={t.id} value={t.id} className="gap-1.5">
                {t.label}
                {counts[t.id] ? <span className="tnum rounded-full bg-muted px-1.5 text-[10.5px] text-muted-foreground">{counts[t.id]}</span> : null}
              </TabsTrigger>
            ))}
          </TabsList>
        </div>
        <div role="tabpanel" aria-label={tab}>
          {tab === "overview" ? <OverviewTab slug={slug} /> : null}
          {tab === "tasks" ? <TasksTab slug={slug} onOpen={(id) => setParams({ task: id })} /> : null}
          {tab === "chat" ? <ChatTab slug={slug} taskId={task} onPickTask={(id) => setParams({ task: id })} /> : null}
          {tab === "threads" ? <ThreadsTab slug={slug} onOpen={(k) => setParams({ thread: k })} /> : null}
          {tab === "proposals" ? <ProposalsTab slug={slug} canDecide={canDecide} /> : null}
          {tab === "suggestions" ? <SuggestionsTab slug={slug} /> : null}
          {tab === "amendments" ? <AmendmentsTab slug={slug} /> : null}
          {tab === "evolution" ? <EvolutionTab slug={slug} /> : null}
          {tab === "timeline" ? <TimelineTab slug={slug} /> : null}
          {tab === "notices" ? <NoticesTab slug={slug} /> : null}
          {tab === "env" ? <EnvTab slug={slug} /> : null}
        </div>
      </Tabs>

      <ThreadSheet slug={slug} threadKey={thread} onClose={() => setParams({ thread: null })} />
      <PostSheet slug={slug} postId={post} onClose={() => setParams({ post: null })} />
      <TaskSheet id={tab === "chat" ? null : task} slug={slug} onClose={() => setParams({ task: null })} />
    </>
  );
}
