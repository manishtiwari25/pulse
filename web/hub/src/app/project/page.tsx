"use client";

import Link from "next/link";
import { Suspense, useCallback } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { Badge } from "@/components/ui/badge";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { PageTitle, EmptyState, SkeletonPage } from "@/components/pulse/panel";
import { CardGrid, ItemCard, ItemCardBody, ItemCardHeader, ItemCardMeta, ItemCardTitle, SectionHeading } from "@/components/pulse/item-card";
import { KindBadge, Mono, ToneBadge, When } from "@/components/pulse/who";
import { CommonsCard } from "@/components/pulse/cards";
import { PostSheet, ThreadSheet } from "@/components/board/sheets";
import { TaskSheet } from "@/components/tasks/task-sheet";
import { NewTaskButton } from "@/components/tasks/task-dialogs";
import { AmendmentsTab, ChatTab, EnvTab, EvolutionTab, NoticesTab, OverviewTab, ProposalsTab, SuggestionsTab, TasksTab, ThreadsTab, TimelineTab } from "@/components/project/tabs";
import { hrefs, isProjectTab, PROJECT_TABS, type ProjectTab } from "@/lib/nav";
import { allProjects, COMMONS_HINT, registeredProjects, type Org, type ProjectCardData } from "@/lib/api";
import { useApi } from "@/lib/use-api";
import { num, plural } from "@/lib/format";

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
    const projects = registeredProjects(org.data);
    const commons = org.data?.commons;
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
          sub="Every checkout the hub knows about. Open one to see its board, tasks and the conversation with its lead."
        >
          Pick a project
        </PageTitle>
        {!org.data && org.loading ? (
          <SkeletonPage rows={2} />
        ) : (
          <>
            {projects.length ? (
              <CardGrid cols={3}>
                {projects.map((p) => (
                  <ProjectPick key={p.slug} p={p} />
                ))}
              </CardGrid>
            ) : (
              <EmptyState>No project registered yet. Run <Mono>pulse init</Mono> in a checkout; the hub lists it here.</EmptyState>
            )}
            {commons ? (
              <section aria-label="Commons" className="mt-4">
                <SectionHeading action={<span>not a project - the board every project shares</span>}>Commons</SectionHeading>
                <CommonsCard c={commons} />
              </section>
            ) : null}
          </>
        )}
      </>
    );
  }

  const card = allProjects(org.data).find((p) => p.slug === slug);
  const isCommons = card?.kind === "commons";
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
    // The Tabs root wraps the sticky title so the tab strip can live in its
    // `bar` row: tabs and New task stay in reach however far the owner scrolls.
    <Tabs value={tab} onValueChange={(v) => setParams({ tab: v === "overview" ? null : v, thread: null, post: null })} className="gap-0">
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
            isCommons ? (
              COMMONS_HINT
            ) : (
              <>
                {card.name && card.name !== card.slug ? card.name + " · " : null}
                <Mono>{card.branch}</Mono> <Mono className="text-muted-foreground">{card.head}</Mono>
                {card.dirty ? <span className="ml-1 text-warn">dirty</span> : null}
                {card.path ? <span className="ml-2 text-muted-foreground">{card.path}</span> : null}
              </>
            )
          ) : (
            org.error || "…"
          )
        }
        // The commons has no checkout and no lead, so no task can be given there.
        actions={isCommons ? undefined : <NewTaskButton project={slug} onCreated={(t) => setParams({ tab: "tasks", task: t.id })} />}
        bar={
          <TabsList variant="line" className="w-max">
            {PROJECT_TABS.map((t) => (
              <TabsTrigger key={t.id} value={t.id} className="gap-1.5 px-2">
                {t.label}
                {counts[t.id] ? <span className="tnum rounded-full bg-muted px-1.5 text-[10.5px] text-muted-foreground">{counts[t.id]}</span> : null}
              </TabsTrigger>
            ))}
          </TabsList>
        }
      >
        <span className="inline-flex flex-wrap items-center gap-2">
          {isCommons ? "Commons" : slug}
          {card?.brain ? <Badge variant="secondary">framework</Badge> : null}
          {isCommons ? <Badge variant="outline">shared board</Badge> : null}
          {card?.stop_present ? <ToneBadge tone="bad">STOP</ToneBadge> : null}
          {card && !card.present ? <Badge variant="destructive">missing on disk</Badge> : null}
        </span>
      </PageTitle>

      <div role="tabpanel" aria-label={tab}>
        {tab === "overview" ? <OverviewTab slug={slug} /> : null}
        {tab === "tasks" ? <TasksTab slug={slug} onOpen={(id) => setParams({ task: id })} activeId={task} /> : null}
        {tab === "chat" ? <ChatTab slug={slug} taskId={task} onPickTask={(id) => setParams({ task: id })} /> : null}
        {tab === "threads" ? <ThreadsTab slug={slug} onOpen={(k) => setParams({ thread: k })} activeKey={thread} /> : null}
        {tab === "proposals" ? <ProposalsTab slug={slug} canDecide={canDecide} /> : null}
        {tab === "suggestions" ? <SuggestionsTab slug={slug} /> : null}
        {tab === "amendments" ? <AmendmentsTab slug={slug} /> : null}
        {tab === "evolution" ? <EvolutionTab slug={slug} /> : null}
        {tab === "timeline" ? <TimelineTab slug={slug} /> : null}
        {tab === "notices" ? <NoticesTab slug={slug} /> : null}
        {tab === "env" ? <EnvTab slug={slug} /> : null}
      </div>

      <ThreadSheet slug={slug} threadKey={thread} onClose={() => setParams({ thread: null })} />
      <PostSheet slug={slug} postId={post} onClose={() => setParams({ post: null })} />
      <TaskSheet id={tab === "chat" ? null : task} slug={slug} onClose={() => setParams({ task: null })} />
    </Tabs>
  );
}

// One registered checkout on the picker: what it is, what waits in it.
function ProjectPick({ p }: { p: ProjectCardData }) {
  const attention = p.proposals_awaiting > 0;
  return (
    <ItemCard href={hrefs.project(p.slug)} highlight={attention} ariaLabel={p.slug}>
      <ItemCardHeader right={p.last_post ? <When d={p.last_post.date} /> : null}>
        {p.brain ? <Badge variant="secondary">framework</Badge> : null}
        {p.stop_present ? <ToneBadge tone="bad">STOP</ToneBadge> : null}
        {!p.present ? <Badge variant="destructive">missing on disk</Badge> : null}
        {p.branch ? <Mono className="text-muted-foreground">{p.branch}</Mono> : null}
        {p.dirty ? <span className="text-warn">dirty</span> : null}
      </ItemCardHeader>
      <ItemCardTitle chevron>{p.slug}</ItemCardTitle>
      {p.name && p.name !== p.slug ? <ItemCardBody clamp={1}>{p.name}</ItemCardBody> : null}
      <ItemCardMeta>
        <span className={attention ? "font-medium text-ok" : undefined}>{num(p.proposals_awaiting) + " for you"}</span>
        <span>{plural(p.claims_active, "claim")}</span>
        <span>{plural(p.threads_open, "open thread")}</span>
        <span>{num(p.posts_24h) + " posts/24h"}</span>
      </ItemCardMeta>
      {p.last_post ? (
        <ItemCardBody clamp={1} className="flex items-center gap-1.5">
          <KindBadge kind={p.last_post.kind} />
          <span className="truncate">{p.last_post.summary}</span>
        </ItemCardBody>
      ) : (
        <ItemCardBody clamp={1}>{p.board_present ? "No posts yet." : "No board yet - pulse init registers this checkout."}</ItemCardBody>
      )}
    </ItemCard>
  );
}
