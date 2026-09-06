import { hubApi, PLACE, type Org, type PostRef, type Runner, type Timeline, type Votes } from '../api';
import { num, plural } from '../lib/format';
import { useApi, settle } from '../lib/use-api';
import { AttentionRow, attentionItems, FeedList, MachinePanel, ProjectCard, ProposalCard, StatusLine, SuggestionCard, WorkingNow } from '../components/cards';
import { ActivityPanel, lastDays } from '../components/line-chart';
import { Sparkline } from '../components/ui/bars';
import { Badge } from '../components/ui/chip';
import { Kpi, KpiGrid } from '../components/ui/kpi';
import { EmptyState, Lead, Notice, PageTitle, Panel, SkeletonPage, Stack } from '../components/ui/panel';
import { ProjectLink } from '../components/ui/who';
import type { ViewProps } from './types';

interface Data {
  o: Org;
  tl: Timeline | null;
  v: Votes;
  act: PostRef[];
  runner: Runner | null;
}

const EMPTY_VOTES: Votes = { suggestions: [], amendments: [] };

export function MissionControl({ revision, animate, chart, onChartPrefs, onStamp }: ViewProps) {
  const { data, error, loading, reload } = useApi<Data>(
    async () => {
      const [o, tl, v, act, runner] = await Promise.all([hubApi<Org>('org'), settle(hubApi<Timeline>('timeline'), null), settle(hubApi<Votes>('votes'), EMPTY_VOTES), settle(hubApi<PostRef[]>('activity'), []), settle(hubApi<Runner>('runner'), null)]);
      onStamp(o.stamp || '');
      return { o, tl, v, act, runner };
    },
    'org',
    revision,
  );
  if (error && !data) return <Notice tone="alert">{'Could not load this view: ' + error}</Notice>;
  if (!data) return <SkeletonPage />;
  const { o, tl, v, act, runner } = data;
  const t = o.totals;
  const missing = PLACE.missing;
  const known = !!missing && (o.projects.some((p) => p.slug === missing) || (o.commons ? o.commons.slug === missing : false));
  const commonsHref = o.commons && o.commons.present ? '/commons/#/threads' : '#/org';
  const week = tl ? lastDays(tl, 7) : [];
  const weekTotal = week.reduce((a, b) => a + b, 0);
  const sparks: Record<string, number[]> = {};
  for (const s of (tl && tl.series) || []) sparks[s.slug] = lastDays(s, 7);
  const items = attentionItems(v).slice(0, 6);
  const eligible = v.suggestions.filter((s) => s.status === 'eligible');
  const shipped = act.filter((p) => p.kind === 'result').slice(0, 6);
  const lastResult = shipped[0] || null;
  const standbySub = runner && runner.available ? plural(runner.hands.slots, 'slot') + ' on ' + plural(runner.hands.executors_active, 'executor') + ' \u00b7 daemon ' + runner.daemon.state : '';
  return (
    <>
      <PageTitle sub="every PULSE project on this machine, live">Mission control</PageTitle>
      <StatusLine runner={runner} eligible={eligible.length} lastResult={lastResult} projects={t.projects_present} />
      {loading && error ? <Notice tone="alert">{'The last refresh failed: ' + error}</Notice> : null}
      {missing ? (
        <div className="mb-3">
          <Notice tone="alert">
            {known ? (
              <>
                The project <code>{missing}</code> is registered but cannot be shown right now (see its card below); the rest of the organization follows.
              </>
            ) : (
              <>
                No project <code>{missing}</code> is registered on this machine. Register one with pulse-dashboard.py register --repo PATH, or pick a project below.
              </>
            )}
          </Notice>
        </div>
      ) : null}
      {(t.brains || []).length > 1 ? (
        <div className="mb-3">
          <Notice tone="alert">{'More than one project is marked brain: ' + (t.brains || []).join(', ') + '. Run index --brain in the intended checkout to repair the registry.'}</Notice>
        </div>
      ) : t.brain ? (
        <Lead>
          Suggestions about PULSE itself belong on the brain{'\u2019'}s board: <ProjectLink base={'/p/' + t.brain} slug={t.brain} />.
        </Lead>
      ) : null}
      <KpiGrid>
        <Kpi n={t.proposals_awaiting || 0} label="for you" href="#/proposals" sub="proposals awaiting your yes or no" tone={t.proposals_awaiting ? 'ok' : ''} animate={animate} />
        <Kpi n={t.projects_present} label="projects" href="#/org" sub={t.projects_present !== t.projects ? t.projects - t.projects_present + ' registered but missing' : 'on this machine'} tone="accent" animate={animate} />
        <Kpi n={t.agents_active} label="working now" href="#/agents" sub="agents holding a claim" tone={t.agents_active ? 'ok' : ''} animate={animate} />
        {runner && runner.available ? <Kpi n={runner.hands.free} label="hands in standby" href="#/agents" sub={standbySub} tone={runner.hands.free ? 'ok' : 'warn'} animate={animate} /> : null}
        <Kpi n={t.agents} label="agents" href="#/agents" sub="seen across all projects" animate={animate} />
        <Kpi n={t.votes_open} label="votes open" href="#/votes" sub="suggestions and amendments" tone={t.votes_open ? 'warn' : ''} animate={animate} />
        <Kpi n={t.questions_open} label="open questions" href={commonsHref} sub="on the commons" animate={animate} />
        <Kpi n={t.posts_24h} label="posts / 24h" href="#/activity" sub={weekTotal ? num(weekTotal) + ' in 7 days' : ''} spark={week.length ? <Sparkline values={week} /> : null} animate={animate} />
      </KpiGrid>
      {(o.proposals || []).length ? (
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
              {o.proposals.slice(0, 3).map((r) => (
                <ProposalCard key={r.id} r={r} far onDecided={reload} />
              ))}
            </Stack>
          </Panel>
        </div>
      ) : null}
      {tl ? (
        <div className="mb-3">
          <ActivityPanel tl={tl} title="Activity across projects" bys={['project', 'kind']} range={chart.range} by={chart.by} onPrefs={onChartPrefs} />
        </div>
      ) : null}
      <div className="grid gap-3 xl:grid-cols-[minmax(0,1.6fr)_minmax(0,1fr)]">
        <Stack>
          <Panel title="Projects">
            {o.projects.length ? (
              <div className="grid gap-3 2xl:grid-cols-2">
                {o.projects.map((p) => (
                  <ProjectCard key={p.slug} p={p} spark={sparks[p.slug] ? <Sparkline values={sparks[p.slug]!} w={84} h={22} /> : null} />
                ))}
                {o.commons ? <ProjectCard p={o.commons} spark={sparks[o.commons.slug] ? <Sparkline values={sparks[o.commons.slug]!} w={84} h={22} /> : null} /> : null}
              </div>
            ) : (
              <EmptyState hint="pulse-dashboard.py register --repo PATH">No project is registered yet. Run docs/scripts/pulse-dashboard.py index (or serve) inside a repository that adopts PULSE, or register one.</EmptyState>
            )}
          </Panel>
          <Panel
            title="Latest activity"
            actions={
              <a href="#/activity" className="text-[13px]">
                {'all activity \u2192'}
              </a>
            }
          >
            <FeedList rows={act.slice(0, 12)} far />
          </Panel>
        </Stack>
        <Stack>
          <MachinePanel r={runner} />
          <Panel title="Working now" actions={<Badge n={o.active_claims.length} />}>
            <WorkingNow claims={o.active_claims} />
          </Panel>
          <Panel title="Testing next" actions={<Badge n={eligible.length} />}>
            {eligible.length ? (
              <Stack gap={2}>
                {eligible.slice(0, 3).map((s) => (
                  <SuggestionCard key={s.id} s={s} quorum={s.quorum || 0} />
                ))}
              </Stack>
            ) : (
              <EmptyState>No suggestion is eligible for the loop yet; votes make one eligible.</EmptyState>
            )}
          </Panel>
          <Panel title="Shipped" actions={<Badge n={shipped.length} />}>
            <FeedList rows={shipped} far />
          </Panel>
          <Panel
            title="Needs attention"
            actions={
              <a href="#/votes" className="text-[13px]">
                {'all votes \u2192'}
              </a>
            }
          >
            {items.length ? (
              <Stack gap={2}>
                {items.map((it) => (
                  <AttentionRow key={it.row.id} it={it} />
                ))}
              </Stack>
            ) : (
              <EmptyState>No vote needs attention.</EmptyState>
            )}
          </Panel>
        </Stack>
      </div>
      <p className="mt-4 font-mono text-[11.5px] text-dim [overflow-wrap:anywhere]">{'registry ' + o.registry + ' \u00b7 commons ' + o.commons_dir}</p>
    </>
  );
}
