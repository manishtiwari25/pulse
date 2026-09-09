"use client";

import Link from "next/link";
import { FileTextIcon, MessagesSquareIcon, Rows3Icon } from "lucide-react";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Separator } from "@/components/ui/separator";
import { type Column } from "@/components/pulse/data-table";
import { CardGrid, ItemCard, ItemCardBody, ItemCardHeader, ItemCardTitle, SectionHeading } from "@/components/pulse/item-card";
import { Markdown } from "@/components/pulse/markdown";
import { EmptyState, ErrorNotice, Kv, Panel, SkeletonRows } from "@/components/pulse/panel";
import { KindBadge, Mono, PostLink, Provenance, StatusBadge, ThreadLink, ToneBadge, VoteBadge, When, Who } from "@/components/pulse/who";
import { hrefs } from "@/lib/nav";
import { enc, projectPath, type PostDetail, type PostRef, type ThreadDetail, type ThreadRow } from "@/lib/api";
import { useApi } from "@/lib/use-api";
import { num, plural, statusTone } from "@/lib/format";

function cappedList(items?: string[], cap = 5): string {
  const xs = (items || []).filter(Boolean);
  if (!xs.length) return "";
  const shown = xs.slice(0, cap).join(", ");
  return xs.length > cap ? shown + ", +" + (xs.length - cap) : shown;
}

export function ThreadParticipants({ t }: { t: ThreadRow }) {
  const text = cappedList(t.participants);
  return text ? <span className="text-xs text-muted-foreground anywhere">{"with " + text}</span> : null;
}

export function ThreadLastBy({ t }: { t: ThreadRow }) {
  if (!t.last_by && !t.last_kind) return null;
  return (
    <span className="text-xs text-muted-foreground">
      {"last by " + (t.last_by || "unknown")}
      {t.last_kind ? " (" + t.last_kind + ")" : ""}
    </span>
  );
}

export function ThreadSignals({ t }: { t: ThreadRow }) {
  const claims = (t.active_claims || []).filter(Boolean);
  return (
    <>
      {t.open_questions > 0 ? <ToneBadge tone="warn">{num(t.open_questions) + " open " + (t.open_questions === 1 ? "question" : "questions")}</ToneBadge> : null}
      {t.open_handoffs > 0 ? <ToneBadge tone="warn">{num(t.open_handoffs) + " open " + (t.open_handoffs === 1 ? "handoff" : "handoffs")}</ToneBadge> : null}
      {claims.length ? <ToneBadge tone="info">{"claims " + cappedList(claims, 3)}</ToneBadge> : null}
    </>
  );
}

export const THREAD_COLUMNS = (slug: string): Column<ThreadRow>[] => [
  { key: "thread", label: "thread", sort: (t) => t.key, render: (t) => <ThreadLink slug={slug} k={t.key} /> },
  { key: "participants", label: "participants", hide: "md", render: (t) => <ThreadParticipants t={t} /> },
  { key: "posts", label: "posts", align: "right", sort: (t) => t.posts, render: (t) => num(t.posts) },
  { key: "last", label: "last post", nowrap: true, sort: (t) => t.last, render: (t) => <span className="inline-flex flex-col"><When d={t.last} /><ThreadLastBy t={t} /></span> },
  { key: "status", label: "status", sort: (t) => t.status, render: (t) => <StatusBadge status={t.status} /> },
  { key: "resolution", label: "resolution", hide: "md", render: (t) => (t.resolution ? <ToneBadge tone={statusTone(t.resolution)}>{t.resolution}</ToneBadge> : null) },
  { key: "signals", label: "needs", render: (t) => <span className="inline-flex flex-wrap gap-1"><ThreadSignals t={t} /></span> },
  { key: "summary", label: "latest summary", render: (t) => <span className="anywhere">{t.summary}</span> },
];

// One thread as a card: status, area and resolution up top, the key as the
// title, the latest summary as the body. Opens the thread sheet through
// `onOpen`; without it the card is a plain link to the thread.
export function ThreadCard({ t, slug, onOpen, active }: { t: ThreadRow; slug: string; onOpen?: (key: string) => void; active?: boolean }) {
  const open = onOpen ? () => onOpen(t.key) : undefined;
  return (
    <ItemCard onOpen={open} href={open ? undefined : hrefs.thread(slug, t.key)} active={active} ariaLabel={t.key}>
      <ItemCardHeader
        right={
          <>
            <span className="tnum">{plural(t.posts, "post")}</span>
            <When d={t.last} />
          </>
        }
      >
        <StatusBadge status={t.status} />
        {t.area ? <ToneBadge tone="neutral">{t.area}</ToneBadge> : null}
        {t.resolution ? <ToneBadge tone={statusTone(t.resolution)}>{t.resolution}</ToneBadge> : null}
        <ThreadSignals t={t} />
      </ItemCardHeader>
      <ItemCardTitle chevron className="font-mono text-[13px]">
        {t.key}
      </ItemCardTitle>
      {t.summary ? <ItemCardBody clamp={2}>{t.summary}</ItemCardBody> : null}
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs">
        <ThreadParticipants t={t} />
        <ThreadLastBy t={t} />
      </div>
    </ItemCard>
  );
}

export function ThreadCards({ rows, slug, onOpen, activeKey, cols = 2, empty }: { rows: ThreadRow[]; slug: string; onOpen?: (key: string) => void; activeKey?: string | null; cols?: 1 | 2 | 3; empty?: React.ReactNode }) {
  if (!rows.length) return <EmptyState>{empty ?? "No thread here."}</EmptyState>;
  return (
    <CardGrid cols={cols}>
      {rows.map((t) => (
        <ThreadCard key={t.key} t={t} slug={slug} onOpen={onOpen} active={activeKey === t.key} />
      ))}
    </CardGrid>
  );
}

// Posts inside a sheet - a thread's posts, or the answers to one post - as a
// vertical list in the API's order. The author stays visible at every width
// and the thread key is not repeated: the sheet header already names it.
function PostList({ rows, slug }: { rows: PostRef[]; slug: string }) {
  return (
    <ul className="relative ml-2 border-l">
      {rows.map((p) => (
        <li key={p.id} className="relative grid gap-1 py-3 pl-5 first:pt-0 last:pb-0">
          <span className="absolute top-3 -left-[5px] size-2.5 rounded-full border border-background bg-primary" aria-hidden />
          <div className="flex flex-wrap items-center gap-1.5 text-xs">
            <Who row={p} compact />
            <KindBadge kind={p.kind} />
            {p.vote ? <VoteBadge vote={p.vote} /> : null}
            {p.resolution ? <ToneBadge tone={statusTone(p.resolution)}>{"resolution: " + p.resolution}</ToneBadge> : null}
            <Provenance ok={p.provenance_ok} missing={p.provenance_missing} />
            <When d={p.date} className="ml-auto text-muted-foreground" />
          </div>
          <Markdown source={p.summary} className="text-[13px] leading-snug" />
          {p.on_post ? (
            <p className="text-xs text-muted-foreground">
              on <PostLink slug={slug} id={p.on_post} />
            </p>
          ) : null}
          <PostLink slug={slug} id={p.id} label="open post" className="w-fit text-xs text-muted-foreground" />
        </li>
      ))}
    </ul>
  );
}

export function ThreadSheet({ slug, threadKey, onClose }: { slug: string; threadKey: string | null; onClose: () => void }) {
  return (
    <Sheet open={!!threadKey} onOpenChange={(o) => !o && onClose()}>
      <SheetContent side="right" className="scroll-thin overflow-y-auto rounded-l-xl data-[side=right]:w-full data-[side=right]:sm:max-w-3xl">
        {threadKey ? <ThreadBody slug={slug} k={threadKey} /> : null}
      </SheetContent>
    </Sheet>
  );
}

function ThreadBody({ slug, k }: { slug: string; k: string }) {
  const { data, error, reload } = useApi<ThreadDetail>(projectPath(slug, "threads/" + enc(k)));
  if (error && !data)
    return (
      <div className="grid gap-3">
        <SheetHeader>
          <SheetTitle className="font-mono text-base">{k}</SheetTitle>
          <SheetDescription>Thread</SheetDescription>
        </SheetHeader>
        <div className="px-4">
          <ErrorNotice error={error} onRetry={() => void reload()} />
        </div>
      </div>
    );
  if (!data)
    return (
      <div>
        <SheetHeader>
          <SheetTitle className="font-mono text-base">{k}</SheetTitle>
          <SheetDescription>Loading…</SheetDescription>
        </SheetHeader>
        <div className="px-4">
          <SkeletonRows n={6} />
        </div>
      </div>
    );
  const t = data.thread;
  return (
    <>
      <SheetHeader>
        <div className="flex flex-wrap items-center gap-1.5 text-xs">
          <StatusBadge status={t.status} />
          <ToneBadge tone="neutral">{t.area}</ToneBadge>
          {t.resolution ? <ToneBadge tone={statusTone(t.resolution)}>{"resolution: " + t.resolution}</ToneBadge> : null}
          <ThreadSignals t={t} />
          <Mono className="text-muted-foreground">{t.key}</Mono>
          <span className="text-muted-foreground">
            {num(t.posts) + " posts · last "} <When d={t.last} />
          </span>
        </div>
        <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1">
          <ThreadParticipants t={t} />
          <ThreadLastBy t={t} />
        </div>
        <SheetTitle className="text-lg font-semibold anywhere">{t.summary || t.key}</SheetTitle>
        <SheetDescription className="anywhere">{t.summary}</SheetDescription>
      </SheetHeader>
      <div className="px-4 pb-4">
        <Panel>
          <SectionHeading icon={MessagesSquareIcon} count={data.posts.length}>
            Posts
          </SectionHeading>
          {data.posts.length ? <PostList rows={data.posts} slug={slug} /> : <EmptyState title="No posts yet">No posts in this thread yet.</EmptyState>}
        </Panel>
      </div>
    </>
  );
}

export function PostSheet({ slug, postId, onClose }: { slug: string; postId: string | null; onClose: () => void }) {
  return (
    <Sheet open={!!postId} onOpenChange={(o) => !o && onClose()}>
      <SheetContent side="right" className="scroll-thin overflow-y-auto rounded-l-xl data-[side=right]:w-full data-[side=right]:sm:max-w-3xl">
        {postId ? <PostBody slug={slug} id={postId} /> : null}
      </SheetContent>
    </Sheet>
  );
}

function PostBody({ slug, id }: { slug: string; id: string }) {
  const { data: p, error, reload } = useApi<PostDetail>(projectPath(slug, "posts/" + enc(id)));
  if (error && !p)
    return (
      <div>
        <SheetHeader>
          <SheetTitle className="font-mono text-base">{id}</SheetTitle>
          <SheetDescription>Post</SheetDescription>
        </SheetHeader>
        <div className="px-4">
          <ErrorNotice error={error} onRetry={() => void reload()} />
        </div>
      </div>
    );
  if (!p)
    return (
      <div>
        <SheetHeader>
          <SheetTitle className="font-mono text-base">{id}</SheetTitle>
          <SheetDescription>Loading…</SheetDescription>
        </SheetHeader>
        <div className="px-4">
          <SkeletonRows n={6} />
        </div>
      </div>
    );
  const fm = p.frontmatter || {};
  return (
    <>
      <SheetHeader>
        <div className="flex flex-wrap items-center gap-1.5 text-xs">
          <KindBadge kind={p.kind} />
          <StatusBadge status={p.resolution || undefined} />
          {p.vote ? <VoteBadge vote={p.vote} /> : null}
          {p.resolution ? <ToneBadge tone={statusTone(p.resolution)}>{"resolution: " + p.resolution}</ToneBadge> : null}
          <Provenance ok={p.provenance_ok} missing={p.provenance_missing} />
          <Link href={hrefs.thread(slug, p.thread_key)} className="font-mono hover:underline">
            {p.thread_key}
          </Link>
          {p.project ? <ToneBadge tone="info">{"from " + p.project}</ToneBadge> : null}
        </div>
        <SheetTitle className="text-base leading-snug anywhere">{p.summary}</SheetTitle>
        <SheetDescription className="flex flex-wrap items-center gap-3">
          <Who row={p} compact />
          <When d={p.date} exact />
          <Mono className="text-muted-foreground">{p.id}</Mono>
        </SheetDescription>
      </SheetHeader>
      <div className="grid gap-3 px-4 pb-4">
        <Panel>
          <SectionHeading icon={FileTextIcon}>Body</SectionHeading>
          <Markdown source={p.body || ""} />
        </Panel>
        <Panel>
          <SectionHeading icon={Rows3Icon}>Frontmatter</SectionHeading>
          <Kv rows={Object.entries(fm).map(([k, v]) => [k, k === "on" && v ? <PostLink key={k} slug={slug} id={String(v)} /> : <Mono key={k}>{String(v)}</Mono>])} />
        </Panel>
        <Panel>
          <SectionHeading icon={MessagesSquareIcon} count={p.responses.length}>
            Responses
          </SectionHeading>
          {p.responses.length ? <PostList rows={p.responses} slug={slug} /> : <EmptyState title="No responses yet">No answers, opinions, votes, or results refer to this post.</EmptyState>}
        </Panel>
        <Separator />
        <details className="text-xs">
          <summary className="cursor-pointer text-muted-foreground hover:text-foreground">source</summary>
          <pre className="mt-1 overflow-x-auto rounded-md border bg-muted/50 p-2.5 text-[12px] whitespace-pre-wrap anywhere">{p.body || "(empty)"}</pre>
        </details>
      </div>
    </>
  );
}
