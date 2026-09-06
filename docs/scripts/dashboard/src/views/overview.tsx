import { api, enc, PLACE, type Claim, type Overview, type Proposal, type ThreadRow, type Timeline } from '../api';
import { FeedList, ProposalCard } from '../components/cards';
import { ActivityPanel, lastDays } from '../components/line-chart';
import { Legend, Sparkline, StackedBar } from '../components/ui/bars';
import { Badge, StatusChip } from '../components/ui/chip';
import { Kpi, KpiGrid } from '../components/ui/kpi';
import { EmptyState, Notice, PageTitle, Panel, SkeletonPage, Stack } from '../components/ui/panel';
import { DataTable, type Column } from '../components/ui/table';
import { PostLink, ThreadLink, When, Who } from '../components/ui/who';
import { fmtDate, KINDS, kindVar, num } from '../lib/format';
import { go } from '../lib/router';
import { settle, useApi } from '../lib/use-api';
import type { ViewProps } from './types';

interface Data {
  o: Overview;
  tl: Timeline | null;
  props: Proposal[];
}

const CLAIM_COLUMNS: Column<Claim>[] = [
  { key: 'thread', label: 'thread', render: (c) => <ThreadLink k={c.thread_key} /> },
  { key: 'agent', label: 'agent', render: (c) => <Who row={c} compact /> },
  { key: 'until', label: 'until', nowrap: true, sort: (c) => c.expires || '', render: (c) => (c.expires ? <When d={c.expires} /> : '(no expiry)') },
  { key: 'summary', label: 'summary', render: (c) => <PostLink id={c.id} label={c.summary} /> },
];

const THREAD_COLUMNS: Column<ThreadRow>[] = [
  { key: 'thread', label: 'thread', sort: (t) => t.key, render: (t) => <ThreadLink k={t.key} /> },
  { key: 'posts', label: 'posts', align: 'right', sort: (t) => t.posts, render: (t) => num(t.posts) },
  { key: 'last', label: 'last post', nowrap: true, sort: (t) => t.last, render: (t) => <When d={t.last} /> },
  { key: 'status', label: 'status', sort: (t) => t.status, render: (t) => <StatusChip status={t.status} /> },
  { key: 'summary', label: 'latest summary', render: (t) => t.summary },
];

export function OverviewView({ revision, animate, chart, onChartPrefs }: ViewProps) {
  const commons = PLACE.mode === 'commons';
  const { data, error, reload } = useApi<Data>(async () => {
    const [o, tl, props] = await Promise.all([api<Overview>('overview'), settle(api<Timeline>('timeline'), null), commons ? Promise.resolve([] as Proposal[]) : settle(api<Proposal[]>('proposals'), [] as Proposal[])]);
    return { o, tl, props };
  }, 'overview', revision);
  if (error && !data) return <Notice tone="alert">{'Could not load this view: ' + error}</Notice>;
  if (!data) return <SkeletonPage />;
  const { o, tl, props } = data;
  const evo = o.evolution || {};
  const loopText = evo.evolution_present === '1' ? (evo.stop_present === '1' ? 'STOP present' : 'loop ' + (evo.loop || 'unset')) : 'no docs/evolution';
  const title = <PageTitle>{commons ? 'Commons overview' : 'Overview'}</PageTitle>;
  if (!o.board_present) {
    return (
      <>
        {title}
        <EmptyState>No board in this project yet. Agents post claims, findings, suggestions, and votes once the store knows the project (pulse init); the loop view still works.</EmptyState>
        <div className="mt-3">
          <KpiGrid>
            <Kpi n={loopText} label="self-improvement loop" href="#/evolution" sub={evo.baseline ? 'baseline ' + evo.baseline : ''} animate={animate} />
          </KpiGrid>
        </div>
      </>
    );
  }
  const week = tl ? lastDays(tl, 7) : [];
  const kindParts = Object.entries(o.kinds || {})
    .sort((a, b) => KINDS.indexOf(a[0]) - KINDS.indexOf(b[0]))
    .map(([k, n]) => ({ label: k, n, color: kindVar(k) }));
  const awaitingHere = props.filter((r) => r.status === 'awaiting');
  const quality = (o.posts_without_provenance ? o.posts_without_provenance + ' without provenance' : '') + (o.posts_without_provenance && o.posts_without_kind ? ' \u00b7 ' : '') + (o.posts_without_kind ? o.posts_without_kind + ' without a kind' : '');
  return (
    <>
      {title}
      <KpiGrid>
        {commons ? null : <Kpi n={o.proposals_awaiting || 0} label="for you" href="#/proposals" sub="proposals awaiting your yes or no" tone={o.proposals_awaiting ? 'ok' : ''} animate={animate} />}
        <Kpi n={o.claims_active} label="active claims" href="#/overview" sub="agents working now" tone={o.claims_active ? 'ok' : ''} animate={animate} />
        <Kpi n={o.threads_open} label="open threads" href="#/threads" sub={o.threads_stale ? o.threads_stale + ' stale' : 'none stale'} tone={o.threads_stale ? 'warn' : ''} animate={animate} />
        <Kpi n={o.suggestions_eligible} label="eligible suggestions" href="#/suggestions" sub="ready for the loop" tone={o.suggestions_eligible ? 'accent' : ''} animate={animate} />
        <Kpi n={o.suggestions_needing_votes} label="need votes" href="#/suggestions" sub={'quorum ' + o.quorum + ' distinct voters'} animate={animate} />
        {commons ? null : <Kpi n={o.amendments_open} label="open amendments" href="#/amendments" sub={o.amendments_passed ? o.amendments_passed + ' passed' : ''} animate={animate} />}
        {commons ? null : <Kpi n={loopText} label="self-improvement loop" href="#/evolution" sub={evo.baseline ? 'baseline ' + evo.baseline : ''} animate={animate} />}
        <Kpi n={o.posts} label="posts" href="#/threads" sub={quality} tone={o.posts_without_provenance || o.posts_without_kind ? 'bad' : ''} spark={week.length ? <Sparkline values={week} /> : null} animate={animate} />
      </KpiGrid>
      {awaitingHere.length ? (
        <div className="mb-3">
          <Panel
            title={'For you \u00b7 say yes or no'}
            actions={
              <a href="#/proposals" className="text-[13px]">
                {'all proposals \u2192'}
              </a>
            }
          >
            <Stack>
              {awaitingHere.slice(0, 3).map((r) => (
                <ProposalCard key={r.id} r={r} far={false} onDecided={reload} />
              ))}
            </Stack>
          </Panel>
        </div>
      ) : null}
      {tl ? (
        <div className="mb-3">
          <ActivityPanel tl={tl} title="Activity" bys={['total', 'kind']} range={chart.range} by={chart.by} onPrefs={onChartPrefs} />
        </div>
      ) : null}
      <div className="grid gap-3 xl:grid-cols-[minmax(0,1.6fr)_minmax(0,1fr)]">
        <Stack>
          <Panel title="Active claims" actions={<Badge n={o.active_claims.length} />}>
            {o.active_claims.length ? <DataTable columns={CLAIM_COLUMNS} rows={o.active_claims} rowKey={(c) => c.id} /> : <EmptyState>No active claims.</EmptyState>}
          </Panel>
          <Panel title="Open threads" actions={<Badge n={o.open_threads.length} />}>
            {o.open_threads.length ? <DataTable columns={THREAD_COLUMNS} rows={o.open_threads} rowKey={(t) => t.key} onRow={(t) => go('#/thread/' + enc(t.key))} /> : <EmptyState>No open threads.</EmptyState>}
          </Panel>
          <Panel title="Expired claims (stale)">{o.stale_claims.length ? <DataTable columns={CLAIM_COLUMNS} rows={o.stale_claims} rowKey={(c) => c.id} /> : <EmptyState>No expired claims.</EmptyState>}</Panel>
        </Stack>
        <Stack>
          {kindParts.length ? (
            <Panel title="Posts by kind">
              <StackedBar parts={kindParts} />
              <Legend parts={kindParts} />
            </Panel>
          ) : null}
          {(o.agents || []).length ? (
            <Panel title="Agents seen here">
              <div className="flex flex-wrap gap-1.5">
                {o.agents.map((a, i) => (
                  <span key={(a.agent || '') + i} title={a.posts + ' posts \u00b7 last ' + fmtDate(a.last)} className="inline-flex items-center gap-1.5 rounded-chip border border-line px-2 py-0.5 text-[12.5px]">
                    <Who row={a} compact /> <span className="tnum text-dim">{a.posts}</span>
                  </span>
                ))}
              </div>
            </Panel>
          ) : null}
          <Panel title="Recent posts">
            <FeedList rows={o.recent_posts} far={false} />
          </Panel>
        </Stack>
      </div>
      {commons ? null : <p className="mt-4 text-[13px] text-muted">{'Verdict on the product scope: ' + (evo.product_verdict || 'n/a')}</p>}
    </>
  );
}
