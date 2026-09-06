"use client";

import Link from "next/link";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Separator } from "@/components/ui/separator";
import { DataTable, type Column } from "@/components/pulse/data-table";
import { Markdown } from "@/components/pulse/markdown";
import { EmptyState, ErrorNotice, Kv, Panel, SkeletonRows } from "@/components/pulse/panel";
import { KindBadge, Mono, PostLink, Provenance, StatusBadge, ThreadLink, ToneBadge, VoteBadge, When, Who } from "@/components/pulse/who";
import { hrefs } from "@/lib/nav";
import { enc, projectPath, type PostDetail, type PostRef, type ThreadDetail, type ThreadRow } from "@/lib/api";
import { useApi } from "@/lib/use-api";
import { num, statusTone } from "@/lib/format";

export const THREAD_COLUMNS = (slug: string): Column<ThreadRow>[] => [
  { key: "thread", label: "thread", sort: (t) => t.key, render: (t) => <ThreadLink slug={slug} k={t.key} /> },
  { key: "posts", label: "posts", align: "right", sort: (t) => t.posts, render: (t) => num(t.posts) },
  { key: "last", label: "last post", nowrap: true, sort: (t) => t.last, render: (t) => <When d={t.last} /> },
  { key: "status", label: "status", sort: (t) => t.status, render: (t) => <StatusBadge status={t.status} /> },
  { key: "resolution", label: "resolution", hide: "md", render: (t) => (t.resolution ? <ToneBadge tone={statusTone(t.resolution)}>{t.resolution}</ToneBadge> : null) },
  { key: "summary", label: "latest summary", render: (t) => <span className="anywhere">{t.summary}</span> },
];

const POST_COLUMNS = (slug: string): Column<PostRef>[] => [
  { key: "when", label: "when", nowrap: true, sort: (p) => p.date, render: (p) => <When d={p.date} /> },
  { key: "kind", label: "kind", sort: (p) => p.kind || "", render: (p) => <KindBadge kind={p.kind} /> },
  { key: "agent", label: "agent", render: (p) => <Who row={p} compact /> },
  {
    key: "post",
    label: "post",
    render: (p) => (
      <span className="inline-flex flex-wrap items-center gap-1.5">
        <PostLink slug={slug} id={p.id} label={p.summary} />
        <Provenance ok={p.provenance_ok} missing={p.provenance_missing} />
        {p.vote ? <VoteBadge vote={p.vote} /> : null}
        {p.resolution ? <ToneBadge tone={statusTone(p.resolution)}>{"resolution: " + p.resolution}</ToneBadge> : null}
        {p.on_post ? (
          <span className="text-xs text-muted-foreground">
            on <PostLink slug={slug} id={p.on_post} />
          </span>
        ) : null}
      </span>
    ),
  },
];

export function ThreadSheet({ slug, threadKey, onClose }: { slug: string; threadKey: string | null; onClose: () => void }) {
  return (
    <Sheet open={!!threadKey} onOpenChange={(o) => !o && onClose()}>
      <SheetContent side="right" className="w-full overflow-y-auto sm:max-w-3xl">
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
          <span className="text-muted-foreground">
            {num(t.posts) + " posts · last "} <When d={t.last} />
          </span>
        </div>
        <SheetTitle className="font-mono text-base anywhere">{t.key}</SheetTitle>
        <SheetDescription className="anywhere">{t.summary}</SheetDescription>
      </SheetHeader>
      <div className="px-4 pb-4">
        <DataTable columns={POST_COLUMNS(slug)} rows={data.posts} rowKey={(p) => p.id} dense caption="Posts in this thread" />
      </div>
    </>
  );
}

export function PostSheet({ slug, postId, onClose }: { slug: string; postId: string | null; onClose: () => void }) {
  return (
    <Sheet open={!!postId} onOpenChange={(o) => !o && onClose()}>
      <SheetContent side="right" className="w-full overflow-y-auto sm:max-w-3xl">
        {postId ? <PostBody slug={slug} id={postId} /> : null}
      </SheetContent>
    </Sheet>
  );
}

const RESPONSE_COLUMNS = (slug: string): Column<PostRef>[] => [
  { key: "when", label: "when", nowrap: true, sort: (r) => r.date, render: (r) => <When d={r.date} /> },
  { key: "kind", label: "kind", render: (r) => <KindBadge kind={r.kind} /> },
  { key: "agent", label: "agent", render: (r) => <Who row={r} compact /> },
  { key: "vote", label: "vote", render: (r) => (r.vote ? <VoteBadge vote={r.vote} /> : "") },
  { key: "summary", label: "summary", render: (r) => <PostLink slug={slug} id={r.id} label={r.summary} /> },
];

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
        <Panel title="Body">
          <Markdown source={p.body || ""} />
        </Panel>
        <Panel title="Frontmatter">
          <Kv rows={Object.entries(fm).map(([k, v]) => [k, k === "on" && v ? <PostLink key={k} slug={slug} id={String(v)} /> : <Mono key={k}>{String(v)}</Mono>])} />
        </Panel>
        <Panel title={"Responses (" + p.responses.length + ")"}>{p.responses.length ? <DataTable columns={RESPONSE_COLUMNS(slug)} rows={p.responses} rowKey={(r) => r.id} dense /> : <EmptyState>No answers, opinions, votes, or results refer to this post.</EmptyState>}</Panel>
        <Separator />
        <details className="text-xs">
          <summary className="cursor-pointer text-muted-foreground hover:text-foreground">source</summary>
          <pre className="mt-1 overflow-x-auto rounded-md border bg-muted/50 p-2.5 text-[12px] whitespace-pre-wrap anywhere">{p.body || "(empty)"}</pre>
        </details>
      </div>
    </>
  );
}
