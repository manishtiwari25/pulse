import { useState } from 'preact/hooks';
import { hubApi, type PostRef, type Timeline } from '../api';
import { ActivityPanel } from '../components/line-chart';
import { Chip, KindLabel, Provenance, VoteChip } from '../components/ui/chip';
import { Count, EmptyState, Notice, PageTitle, Select, SkeletonPage, Toolbar } from '../components/ui/panel';
import { DataTable, type Column } from '../components/ui/table';
import { FarPost, FarThread, ProjectLink, When, Who } from '../components/ui/who';
import { num } from '../lib/format';
import { settle, useApi } from '../lib/use-api';
import type { ViewProps } from './types';

interface Data {
  rows: PostRef[];
  tl: Timeline | null;
}

const COLUMNS: Column<PostRef>[] = [
  { key: 'when', label: 'when', nowrap: true, sort: (p) => p.date, render: (p) => <When d={p.date} /> },
  { key: 'project', label: 'project', sort: (p) => p.slug || '', render: (p) => (p.slug ? <ProjectLink base={p.base || ''} slug={p.slug} /> : null) },
  { key: 'kind', label: 'kind', sort: (p) => p.kind || '', render: (p) => <KindLabel kind={p.kind} /> },
  { key: 'agent', label: 'agent', render: (p) => <Who row={p} compact /> },
  { key: 'thread', label: 'thread', render: (p) => <FarThread base={p.base || ''} k={p.thread_key} /> },
  {
    key: 'post',
    label: 'post',
    render: (p) => (
      <span className="inline-flex flex-wrap items-center gap-1.5">
        <FarPost base={p.base || ''} id={p.id} label={p.summary} />
        {p.vote ? <VoteChip vote={p.vote} /> : null}
        {p.resolution ? <Chip tone="info">{'resolution: ' + p.resolution}</Chip> : null}
        <Provenance ok={p.provenance_ok} missing={[]} />
      </span>
    ),
  },
];

export function ActivityView({ revision, chart, onChartPrefs }: ViewProps) {
  const { data, error } = useApi<Data>(async () => {
    const [rows, tl] = await Promise.all([hubApi<PostRef[]>('activity'), settle(hubApi<Timeline>('timeline'), null)]);
    return { rows, tl };
  }, 'activity', revision);
  const [slug, setSlug] = useState('');
  const [kind, setKind] = useState('');
  if (error && !data) return <Notice tone="alert">{'Could not load this view: ' + error}</Notice>;
  if (!data) return <SkeletonPage />;
  const { rows, tl } = data;
  const slugs = [...new Set(rows.map((p) => p.slug || ''))].filter(Boolean).sort();
  const kinds = [...new Set(rows.map((p) => p.kind || '(none)'))].sort();
  const list = rows.filter((p) => (!slug || p.slug === slug) && (!kind || (p.kind || '(none)') === kind));
  return (
    <>
      <PageTitle sub="the newest posts across every project and the commons">Activity</PageTitle>
      {tl ? (
        <div className="mb-3">
          <ActivityPanel tl={tl} title="Posts over time" bys={['project', 'kind']} range={chart.range} by={chart.by} onPrefs={onChartPrefs} />
        </div>
      ) : null}
      <Toolbar>
        <Select label="Filter by project" value={slug} onChange={setSlug} options={[['', 'all projects'], ...slugs.map((s) => [s, s] as [string, string])]} />
        <Select label="Filter by kind" value={kind} onChange={setKind} options={[['', 'any kind'], ...kinds.map((k) => [k, k] as [string, string])]} />
        <Count>{num(list.length) + ' of the latest ' + num(rows.length) + ' posts'}</Count>
      </Toolbar>
      {list.length ? (
        <div className="panel p-3">
          <DataTable columns={COLUMNS} rows={list} rowKey={(p) => (p.slug || '') + p.id} />
        </div>
      ) : (
        <EmptyState>Nothing matches.</EmptyState>
      )}
    </>
  );
}
