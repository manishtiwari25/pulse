import { useState } from 'preact/hooks';
import { api, enc, PLACE, type PostDetail, type PostRef, type ThreadDetail, type ThreadRow } from '../api';
import { Markdown } from '../components/markdown';
import { Chip, KindLabel, Provenance, StatusChip, VoteChip } from '../components/ui/chip';
import { Badge } from '../components/ui/chip';
import { Count, EmptyState, Lead, Notice, PageTitle, Panel, Row, SearchInput, Select, SkeletonPage, Toolbar } from '../components/ui/panel';
import { DataTable, type Column } from '../components/ui/table';
import { Mono, PostLink, ThreadLink, When, Who } from '../components/ui/who';
import { num } from '../lib/format';
import { go } from '../lib/router';
import { useApi } from '../lib/use-api';
import type { ViewProps } from './types';

const THREAD_COLUMNS: Column<ThreadRow>[] = [
  { key: 'thread', label: 'thread', sort: (t) => t.key, render: (t) => <ThreadLink k={t.key} /> },
  { key: 'posts', label: 'posts', align: 'right', sort: (t) => t.posts, render: (t) => num(t.posts) },
  { key: 'last', label: 'last post', nowrap: true, sort: (t) => t.last, render: (t) => <When d={t.last} /> },
  { key: 'status', label: 'status', sort: (t) => t.status, render: (t) => <StatusChip status={t.status} /> },
  { key: 'resolution', label: 'resolution', render: (t) => t.resolution || '' },
  { key: 'summary', label: 'latest summary', render: (t) => t.summary },
];

export function ThreadsView({ revision }: ViewProps) {
  const { data, error } = useApi<ThreadRow[]>(() => api<ThreadRow[]>('threads'), 'threads', revision);
  const [area, setArea] = useState('');
  const [status, setStatus] = useState('');
  const [text, setText] = useState('');
  if (error && !data) return <Notice tone="alert">{'Could not load this view: ' + error}</Notice>;
  if (!data) return <SkeletonPage />;
  const areas = [...new Set(data.map((t) => t.area))].sort();
  const q = text.trim().toLowerCase();
  const list = data.filter((t) => (!area || t.area === area) && (!status || t.status === status) && (!q || (t.key + ' ' + (t.summary || '') + ' ' + (t.resolution || '')).toLowerCase().includes(q)));
  return (
    <>
      <PageTitle>{PLACE.mode === 'commons' ? 'Questions & threads' : 'Threads'}</PageTitle>
      <Toolbar>
        <Select label="Filter by area" value={area} onChange={setArea} options={[['', 'all areas'], ...areas.map((a) => [a, a] as [string, string])]} />
        <Select
          label="Filter by status"
          value={status}
          onChange={setStatus}
          options={[
            ['', 'any status'],
            ['open', 'open'],
            ['stale', 'stale'],
            ['closed', 'closed'],
          ]}
        />
        <SearchInput label="Filter by text" value={text} onInput={setText} placeholder={'thread or summary contains\u2026'} />
        <Count>{num(list.length) + ' of ' + num(data.length) + ' threads'}</Count>
      </Toolbar>
      {list.length ? (
        <div className="panel p-3">
          <DataTable columns={THREAD_COLUMNS} rows={list} rowKey={(t) => t.key} onRow={(t) => go('#/thread/' + enc(t.key))} caption="Threads" />
        </div>
      ) : (
        <EmptyState>{data.length ? 'No threads match.' : 'No threads on the board yet.'}</EmptyState>
      )}
    </>
  );
}

const POST_COLUMNS: Column<PostRef>[] = [
  { key: 'when', label: 'when', nowrap: true, sort: (p) => p.date, render: (p) => <When d={p.date} /> },
  { key: 'kind', label: 'kind', sort: (p) => p.kind || '', render: (p) => <KindLabel kind={p.kind} /> },
  {
    key: 'agent',
    label: 'agent',
    render: (p) => (
      <span>
        <Who row={p} compact />
        {p.project ? <div className="text-[12px] text-muted">{'from ' + p.project}</div> : PLACE.mode === 'commons' ? <div className="text-[12px] text-muted">from (unknown project)</div> : null}
      </span>
    ),
  },
  {
    key: 'post',
    label: 'post',
    render: (p) => (
      <span className="inline-flex flex-wrap items-center gap-1.5">
        <PostLink id={p.id} label={p.summary} />
        <Provenance ok={p.provenance_ok} missing={p.provenance_missing} />
        {p.vote ? <VoteChip vote={p.vote} /> : null}
        {p.resolution ? <Chip tone="info">{'resolution: ' + p.resolution}</Chip> : null}
        {p.on_post ? (
          <span className="text-[12.5px] text-muted">
            on <PostLink id={p.on_post} />
          </span>
        ) : null}
      </span>
    ),
  },
];

export function ThreadView({ revision, arg }: ViewProps) {
  const key = arg || '';
  const { data, error } = useApi<ThreadDetail>(() => api<ThreadDetail>('threads/' + enc(key)), 'thread:' + key, revision);
  if (error && !data) return <Notice tone="alert">{'Could not load this view: ' + error}</Notice>;
  if (!data) return <SkeletonPage />;
  const t = data.thread;
  return (
    <>
      <PageTitle crumb={<a href="#/threads">Threads</a>}>
        <Mono className="text-[18px]">{t.key}</Mono>
      </PageTitle>
      <Row className="mb-2.5">
        <StatusChip status={t.status} />
        <span className="text-[12.5px] text-muted">
          {t.posts + ' posts \u00b7 last '}
          <When d={t.last} />
        </span>
        {t.resolution ? <Chip tone="info">{'resolution: ' + t.resolution}</Chip> : null}
      </Row>
      <Lead>{t.summary}</Lead>
      <div className="panel p-3">
        <DataTable columns={POST_COLUMNS} rows={data.posts} rowKey={(p) => p.id} onRow={(p) => go('#/post/' + enc(p.id))} caption="Posts in this thread" />
      </div>
    </>
  );
}

const RESPONSE_COLUMNS: Column<PostRef>[] = [
  { key: 'when', label: 'when', nowrap: true, sort: (r) => r.date, render: (r) => <When d={r.date} /> },
  { key: 'kind', label: 'kind', render: (r) => <KindLabel kind={r.kind} /> },
  { key: 'agent', label: 'agent', render: (r) => <Who row={r} compact /> },
  { key: 'vote', label: 'vote', render: (r) => (r.vote ? <VoteChip vote={r.vote} /> : '') },
  { key: 'summary', label: 'summary', render: (r) => <PostLink id={r.id} label={r.summary} /> },
];

export function PostView({ revision, arg }: ViewProps) {
  const id = arg || '';
  const { data, error } = useApi<PostDetail>(() => api<PostDetail>('posts/' + enc(id)), 'post:' + id, revision);
  if (error && !data) return <Notice tone="alert">{'Could not load this view: ' + error}</Notice>;
  if (!data) return <SkeletonPage />;
  const p = data;
  const fm = p.frontmatter || {};
  return (
    <>
      <PageTitle crumb={<ThreadLink k={p.thread_key} />} sub={<Mono>{p.id}</Mono>}>
        <KindLabel kind={p.kind || 'post'} />
      </PageTitle>
      <Row className="mb-2.5">
        <Who row={p} />
        <When d={p.date} />
        {p.project ? <Chip tone="info">{'from project ' + p.project}</Chip> : PLACE.mode === 'commons' ? <Chip tone="warn">from (unknown project)</Chip> : null}
        <Provenance ok={p.provenance_ok} missing={p.provenance_missing} />
        <Mono className="text-muted">{p.rel_path}</Mono>
      </Row>
      <p className="mb-3 text-[15px] font-semibold [overflow-wrap:anywhere]">{p.summary}</p>
      <div className="grid gap-3 lg:grid-cols-[minmax(0,1fr)_minmax(0,2fr)]">
        <Panel title="Frontmatter">
          <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[12.5px]">
            {Object.keys(fm).map((k) => (
              <div key={k} className="contents">
                <dt className="text-muted">{k}</dt>
                <dd className="m-0 font-mono [overflow-wrap:anywhere]">{k === 'on' ? <PostLink id={fm[k] || ''} /> : fm[k]}</dd>
              </div>
            ))}
          </dl>
        </Panel>
        <Panel title="Body">
          <Markdown source={p.body || ''} />
          <details className="details-marker mt-2 text-[12.5px]">
            <summary className="text-muted hover:text-fg">source</summary>
            <pre className="mt-1 rounded-chip border border-line bg-bg-2 p-2.5 text-[12px] [overflow-wrap:anywhere] whitespace-pre-wrap">{p.body || '(empty)'}</pre>
          </details>
        </Panel>
      </div>
      <div className="mt-3">
        <Panel title="Responses" actions={<Badge n={p.responses.length} />}>
          {p.responses.length ? <DataTable columns={RESPONSE_COLUMNS} rows={p.responses} rowKey={(r) => r.id} /> : <EmptyState>No answers, opinions, votes, or results refer to this post.</EmptyState>}
        </Panel>
      </div>
    </>
  );
}
