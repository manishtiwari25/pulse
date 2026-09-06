import * as Tabs from '@radix-ui/react-tabs';
import { useMemo, useState } from 'preact/hooks';
import { hubApi, type Claim, type Executor, type RosterRow, type Runner } from '../api';
import { Chip, StatusChip } from '../components/ui/chip';
import { Kpi, KpiGrid } from '../components/ui/kpi';
import { Count, EmptyState, Lead, Notice, PageTitle, SearchInput, SkeletonPage, Stack, Toolbar } from '../components/ui/panel';
import { Toggle } from '../components/ui/seg';
import { DataTable, type Column } from '../components/ui/table';
import { FarThread, Mono, ProjectLink, When, Who } from '../components/ui/who';
import { executorTone, num } from '../lib/format';
import { settle, useApi } from '../lib/use-api';
import type { ViewProps } from './types';

interface Data {
  rows: RosterRow[];
  executors: Executor[];
  runner: Runner | null;
}

const ORDER: Record<string, number> = { working: 0, active: 1, recent: 2, idle: 3, dormant: 4, pruned: 5 };
const rank = (s: string) => ORDER[s] ?? 6;

interface Group {
  agent: string;
  sessions: RosterRow[];
  status: string;
  primary: RosterRow;
  projects: string[];
  posts: number;
  active_claims: Claim[];
}

function groupRows(rows: RosterRow[]): Group[] {
  const byAgent = new Map<string, RosterRow[]>();
  for (const p of rows) {
    const list = byAgent.get(p.agent) || [];
    list.push(p);
    byAgent.set(p.agent, list);
  }
  return [...byAgent.entries()]
    .map(([agent, sessions]) => {
      sessions.sort((a, b) => rank(a.status) - rank(b.status) || a.days_since - b.days_since || String(a.session).localeCompare(String(b.session)));
      const status = sessions.map((p) => p.status).sort((a, b) => rank(a) - rank(b))[0] || 'dormant';
      return {
        agent,
        sessions,
        status,
        primary: sessions[0]!,
        projects: [...new Set(sessions.flatMap((p) => p.projects || []))].sort(),
        posts: sessions.reduce((a, p) => a + p.posts, 0),
        active_claims: sessions.flatMap((p) => p.active_claims || []),
      };
    })
    .sort((a, b) => rank(a.status) - rank(b.status) || a.agent.localeCompare(b.agent));
}

const TAB = 'inline-flex h-8 items-center gap-1.5 rounded-chip px-3 text-[13px] text-muted hover:text-fg data-[state=active]:bg-panel data-[state=active]:font-medium data-[state=active]:text-fg data-[state=active]:shadow-panel';

const SESSION_COLUMNS: Column<RosterRow>[] = [
  {
    key: 'agent',
    label: 'agent',
    render: (s) => (
      <span className="inline-flex flex-wrap items-center gap-1.5">
        <Who row={s} />
        {s.name_shared ? (
          <Chip tone="warn" dotted>
            {'name shared with ' + (s.shared_with || []).join(', ')}
          </Chip>
        ) : null}
      </span>
    ),
  },
  {
    key: 'status',
    label: 'status / kind',
    sort: (s) => rank(s.status),
    render: (s) => (
      <span className="inline-flex flex-wrap items-center gap-1">
        <StatusChip status={s.status} />
        {s.kind ? (
          <Chip tone="info" dotted>
            {s.kind}
          </Chip>
        ) : null}
      </span>
    ),
  },
  { key: 'sessions', label: 'sessions', render: (s) => <Mono>{s.sessions && s.sessions.length ? s.sessions.join(', ') : s.session || '(none)'}</Mono> },
  { key: 'projects', label: 'projects', render: (s) => <span className="text-[12.5px]">{s.projects.join(', ')}</span> },
  { key: 'posts', label: 'posts', align: 'right', sort: (s) => s.posts, render: (s) => num(s.posts) },
  { key: 'votes', label: 'votes', align: 'right', sort: (s) => s.votes, render: (s) => num(s.votes) },
  { key: 'claims', label: 'claims', align: 'right', sort: (s) => s.claims, render: (s) => num(s.claims) },
  { key: 'qa', label: 'q / a', align: 'right', render: (s) => s.questions + ' / ' + s.answers },
  { key: 'jobs_done', label: 'jobs done', align: 'right', sort: (s) => s.jobs_done || 0, render: (s) => num(s.jobs_done || 0) },
  { key: 'jobs_failed', label: 'jobs failed', align: 'right', sort: (s) => s.jobs_failed || 0, render: (s) => num(s.jobs_failed || 0) },
  { key: 'useful', label: 'last useful', nowrap: true, sort: (s) => s.last_useful_at || '', render: (s) => (s.last_useful_at ? <When d={s.last_useful_at} /> : <span className="text-muted">(none)</span>) },
  { key: 'first', label: 'first seen', nowrap: true, sort: (s) => s.first_seen || '', render: (s) => <When d={s.first_seen} /> },
  {
    key: 'last',
    label: 'last seen',
    nowrap: true,
    sort: (s) => s.last_seen || '',
    render: (s) => (
      <span title={s.approximate ? 'status uses the local clock because projection now is missing' : ''}>
        <When d={s.last_seen} /> <span className="tnum text-muted">{'(' + Number(s.days_since || 0).toFixed(1) + 'd)'}</span>
      </span>
    ),
  },
  {
    key: 'working',
    label: 'working on',
    render: (s) =>
      s.active_claims.length ? (
        <div className="grid gap-0.5">
          {s.active_claims.map((c) => (
            <div key={c.id} className="text-[12.5px]">
              {c.slug ? <ProjectLink base={c.base || ''} slug={c.slug} /> : null} <FarThread base={c.base || ''} k={c.thread_key} />
              <span className="text-muted">
                {' until '}
                <When d={c.expires} />
              </span>
            </div>
          ))}
        </div>
      ) : (
        <span className="text-[12.5px] text-muted">no active claim</span>
      ),
  },
];

function GroupCard({ g }: { g: Group }) {
  const p = g.primary;
  const n = p.session_count || g.sessions.length;
  return (
    <details className="panel details-marker p-0" open={g.status === 'working'}>
      <summary className="flex flex-wrap items-center gap-2 px-4 py-3">
        <span aria-hidden="true" className="text-[11px] text-dim">
          {'\u25b6'}
        </span>
        <Who row={{ agent: g.agent, session: '', name: p.name || '', runner: p.runner || '', model: p.model || '' }} />
        <StatusChip status={g.status} />
        {p.kind ? (
          <Chip tone="info" dotted>
            {p.kind}
          </Chip>
        ) : null}
        <span className="text-[12.5px] text-muted">{n + ' session' + (n === 1 ? '' : 's')}</span>
        <span className="text-[12.5px] text-muted">{g.projects.join(', ')}</span>
        {p.status === 'pruned' && p.pruned_reason ? <Chip tone="warn">{'pruned: ' + p.pruned_reason}</Chip> : null}
        {g.active_claims.length ? (
          <Chip tone="ok" dotted>
            {g.active_claims.length + ' active claim' + (g.active_claims.length === 1 ? '' : 's')}
          </Chip>
        ) : null}
        <span className="tnum ml-auto text-[12.5px] text-muted">{num(g.posts) + ' posts'}</span>
      </summary>
      <div className="border-t border-line px-3 pb-3">
        <DataTable columns={SESSION_COLUMNS} rows={g.sessions} rowKey={(s, i) => (s.session || '') + i} dense />
      </div>
    </details>
  );
}

const EXECUTOR_COLUMNS: Column<Executor>[] = [
  { key: 'name', label: 'executor', render: (x) => <Mono>{x.name}</Mono> },
  { key: 'status', label: 'status', sort: (x) => x.status, render: (x) => <Chip tone={executorTone[x.status] || ''}>{x.status}</Chip> },
  { key: 'address', label: 'address', render: (x) => <Mono>{x.address || '(this machine)'}</Mono> },
  { key: 'workers', label: 'workers', align: 'right', sort: (x) => x.workers, render: (x) => String(x.workers) },
  { key: 'running', label: 'running', align: 'right', sort: (x) => x.running, render: (x) => String(x.running) },
  { key: 'free', label: 'free', align: 'right', sort: (x) => x.free, render: (x) => String(x.free === undefined ? '' : x.free) },
  { key: 'projects', label: 'projects', render: (x) => (x.projects === '*' ? 'any project' : x.projects) },
  { key: 'done', label: 'runs done', align: 'right', sort: (x) => x.runs_done || 0, render: (x) => num(x.runs_done || 0) },
  { key: 'failed', label: 'runs failed', align: 'right', sort: (x) => x.runs_failed || 0, render: (x) => num(x.runs_failed || 0) },
  { key: 'errors', label: 'errors', align: 'right', render: (x) => String(x.errors || 0) },
  { key: 'probe', label: 'last probe', nowrap: true, render: (x) => <Mono>{x.last_probe_at || ''}</Mono> },
  { key: 'note', label: 'note', render: (x) => <span className="text-[12.5px]">{x.probe_note || ''}</span> },
];

export function AgentsView({ revision, animate }: ViewProps) {
  const { data, error } = useApi<Data>(async () => {
    const [rows, executors, runner] = await Promise.all([hubApi<RosterRow[]>('roster'), settle(hubApi<Executor[]>('executors'), []), settle(hubApi<Runner>('runner'), null)]);
    return { rows, executors, runner };
  }, 'agents', revision);
  const [needle, setNeedle] = useState('');
  const [onlyWorking, setOnlyWorking] = useState(false);
  const [showDormant, setShowDormant] = useState(false);
  const [showPruned, setShowPruned] = useState(false);
  const groups = useMemo(() => (data ? groupRows(data.rows) : []), [data]);
  if (error && !data) return <Notice tone="alert">{'Could not load this view: ' + error}</Notice>;
  if (!data) return <SkeletonPage />;
  const { rows, executors, runner } = data;
  const counts: Record<string, number> = { working: 0, active: 0, recent: 0, idle: 0, dormant: 0, pruned: 0 };
  for (const g of groups) counts[g.status] = (counts[g.status] || 0) + 1;
  const q = needle.trim().toLowerCase();
  const filtered = groups.filter((g) => (showDormant || g.status !== 'dormant') && (showPruned || g.status !== 'pruned') && (!onlyWorking || g.active_claims.length) && (!q || [g.agent, g.projects.join(' '), ...g.sessions.flatMap((p) => [p.name, p.session, p.runner, p.model])].join(' ').toLowerCase().includes(q)));
  const live = runner && runner.available ? runner : null;
  const jobsFailed = rows.reduce((a, p) => a + Number(p.jobs_failed || 0), 0);
  return (
    <>
      <PageTitle sub="the roster across every project">Agents</PageTitle>
      <KpiGrid>
        <Kpi n={groups.length} label="names on the roster" href="#/agents" sub={'identities, not processes' + (live && live.workers.prune_after_days ? '; pruned after ' + live.workers.prune_after_days + ' idle days' : '')} animate={animate} />
        <Kpi n={counts.working || 0} label="working" href="#/agents" sub="holding active claims" tone={counts.working ? 'ok' : ''} animate={animate} />
        {live ? <Kpi n={live.hands.free} label="hands in standby" href="#/agents" sub={'free slots on active executors \u00b7 daemon ' + live.daemon.state} tone={live.hands.free ? 'ok' : 'warn'} animate={animate} /> : null}
        {live ? <Kpi n={live.workers.idle} label="workers in standby" href="#/agents" sub={'of ' + live.workers.pool + ' in the pool, target ' + live.workers.target} animate={animate} /> : null}
        <Kpi n={(counts.active || 0) + (counts.recent || 0)} label="active or recent" href="#/agents" sub="posted in the last 7 days" animate={animate} />
        <Kpi n={(counts.dormant || 0) + (counts.pruned || 0)} label="hidden" href="#/agents" sub="dormant or pruned" animate={animate} />
        <Kpi n={rows.reduce((a, p) => a + Number(p.jobs_done || 0), 0)} label="jobs done" href="#/agents" sub="from the agents table" animate={animate} />
        <Kpi n={jobsFailed} label="jobs failed" href="#/agents" sub="from the agents table" tone={jobsFailed ? 'warn' : ''} animate={animate} />
      </KpiGrid>
      <Lead>In store mode this roster comes from the agents table, grouped by agent name, with pruning history and scorecards. Markdown-only projects still group legacy posts without deleting anything.</Lead>
      <Tabs.Root defaultValue="roster">
        <Tabs.List aria-label="Agents and executors" className="mb-3 inline-flex rounded-card border border-line bg-panel-2 p-0.5">
          <Tabs.Trigger value="roster" className={TAB}>
            Roster
          </Tabs.Trigger>
          <Tabs.Trigger value="executors" className={TAB}>
            Executors {executors.length ? <span className="tnum text-dim">{executors.length}</span> : null}
          </Tabs.Trigger>
        </Tabs.List>
        <Tabs.Content value="roster" className="outline-none">
          <Toolbar>
            <SearchInput label="Search agents" value={needle} onInput={setNeedle} placeholder="name, agent id, runner, model, project..." />
            <Toggle label="working only" checked={onlyWorking} onChange={setOnlyWorking} />
            <Toggle label={'show dormant (' + (counts.dormant || 0) + ')'} checked={showDormant} onChange={setShowDormant} />
            <Toggle label={'show pruned (' + (counts.pruned || 0) + ')'} checked={showPruned} onChange={setShowPruned} />
            <Count>{num(filtered.length) + ' of ' + num(groups.length) + ' agents'}</Count>
          </Toolbar>
          {filtered.length ? (
            <Stack>
              {filtered.map((g) => (
                <GroupCard key={g.agent} g={g} />
              ))}
            </Stack>
          ) : (
            <EmptyState>{groups.length ? 'No agent matches.' : 'No agent has posted on any board yet.'}</EmptyState>
          )}
        </Tabs.Content>
        <Tabs.Content value="executors" className="outline-none">
          <Lead>Where the runner{'\u2019'}s jobs execute: this machine plus every pair of ssh hands added with pulse executor add (ADR-020). The dashboard only reads them; probe, drain, and remove are commands the human runs.</Lead>
          {executors.length ? (
            <div className="panel p-3">
              <DataTable columns={EXECUTOR_COLUMNS} rows={executors} rowKey={(x) => x.name} />
            </div>
          ) : (
            <EmptyState hint="python3 docs/scripts/pulse.py executor add --name NAME --address user@host">No executor is registered; jobs run on this machine only.</EmptyState>
          )}
        </Tabs.Content>
      </Tabs.Root>
    </>
  );
}
