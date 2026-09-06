import { api, type ArchiveTable, type Evolution } from '../api';
import { Bar } from '../components/ui/bars';
import { Chip, StatusChip } from '../components/ui/chip';
import { Kpi, KpiGrid } from '../components/ui/kpi';
import { EmptyState, Notice, PageTitle, Panel, Row, SkeletonPage, Stack } from '../components/ui/panel';
import { DataTable, type Column } from '../components/ui/table';
import { Mono } from '../components/ui/who';
import { useApi } from '../lib/use-api';
import type { ViewProps } from './types';

function ArchivePanel({ section, tbl, last }: { section: string; tbl: ArchiveTable; last: number }) {
  const cols = tbl.columns.length ? tbl.columns : ['Row'];
  const rows = tbl.rows;
  const shown = section.toLowerCase().startsWith('variant') ? rows.slice(-last) : rows;
  // Short identifier-like columns (row ids, dates, tree hashes, decisions) stay on one line; prose wraps.
  const short = (c: string) => /^(row|date|tree|commit|decision|kept|id)$/i.test(c.trim());
  const columns: Column<{ cells: string[] }>[] = cols.map((c, i) => ({ key: String(i), label: c, nowrap: short(c), render: (r) => (short(c) ? <span className="tnum">{r.cells[i] || ''}</span> : r.cells[i] || '') }));
  return (
    <Panel title={section + ' (' + rows.length + ')'} actions={shown.length !== rows.length ? <span className="text-[12px] text-muted">{'last ' + shown.length}</span> : null}>
      {shown.length ? <DataTable columns={columns} rows={shown} rowKey={(_r, i) => String(i)} dense /> : <EmptyState>No rows.</EmptyState>}
    </Panel>
  );
}

export function EvolutionView({ revision, animate }: ViewProps) {
  const { data, error } = useApi<Evolution>(() => api<Evolution>('evolution'), 'evolution', revision);
  if (error && !data) return <Notice tone="alert">{'Could not load this view: ' + error}</Notice>;
  if (!data) return <SkeletonPage />;
  const e = data;
  const s = e.summary || ({} as Evolution['summary']);
  const title = <PageTitle>Self-improvement loop</PageTitle>;
  if (s.evolution_present !== '1') {
    return (
      <>
        {title}
        <EmptyState>No docs/evolution/ in this repository. The loop starts when the owner fills the constitution and evaluators (docs/evolution/README.md).</EmptyState>
      </>
    );
  }
  const stop = s.stop_present === '1';
  const missing = Array.isArray(s.missing_files) ? s.missing_files : s.missing_files ? [String(s.missing_files)] : [];
  const decisions = Object.entries(e.decisions || {});
  const origins = Object.entries(e.origins || {});
  const incidents = Number(s.count_incidents || 0);
  return (
    <>
      {title}
      <KpiGrid>
        <Kpi n={stop ? 'STOP' : s.loop || 'unset'} label="loop flag" sub={stop ? 'kill switch present - every cycle halts' : 'from the constitution'} tone={stop ? 'bad' : s.loop === 'on' ? 'ok' : 'warn'} animate={animate} />
        <Kpi n={s.baseline || 'unset'} label="baseline" sub="from the evaluators" animate={animate} />
        <Kpi n={Number(s.count_variants || 0)} label="variants tried" animate={animate} />
        <Kpi n={Number(s.count_baselines || 0)} label="baseline runs" animate={animate} />
        <Kpi n={incidents} label="incidents" tone={incidents ? 'bad' : ''} animate={animate} />
      </KpiGrid>
      <Stack>
        <div className="grid gap-3 lg:grid-cols-2">
          <Panel title="Constitution">
            <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[13px]">
              <dt className="text-muted">limits</dt>
              <dd className="m-0">{s.limits_note || s.limits || 'unset'}</dd>
              {missing.length ? (
                <>
                  <dt className="text-muted">missing</dt>
                  <dd className="m-0 text-bad">{missing.join(', ')}</dd>
                </>
              ) : null}
            </dl>
          </Panel>
          <Panel title="Decisions">
            <Row>
              {decisions.length ? (
                decisions.map(([k, v]) => (
                  <Chip key={k} tone={k === 'kept' || k === 'promoted' ? 'ok' : 'warn'} dotted>
                    {k + ' ' + v}
                  </Chip>
                ))
              ) : (
                <span className="text-[13px] text-muted">no variants yet</span>
              )}
            </Row>
            <div className="mt-1.5 grid gap-0.5 text-[12.5px] text-muted">
              {origins.map(([k, v]) => (
                <div key={k}>{(k === 'voted' ? 'from voted suggestions: ' : 'from signals: ') + v.tried + ' tried, ' + v.kept + ' kept' + (v.tried ? ' (' + Math.round((100 * v.kept) / v.tried) + '%)' : '')}</div>
              ))}
            </div>
          </Panel>
        </div>
        {Object.entries(e.archive || {}).map(([section, tbl]) => (
          <ArchivePanel key={section} section={section} tbl={tbl} last={Number(s.last || 5)} />
        ))}
        <Panel title="Editable scope (autonomy per file class)">
          {e.scope.length ? (
            <DataTable
              columns={[
                { key: 'class', label: 'file class', render: (r) => r.file_class },
                { key: 'paths', label: 'paths', render: (r) => <Mono>{r.paths}</Mono> },
                { key: 'stage', label: 'stage', render: (r) => r.stage },
                { key: 'autonomy', label: 'autonomy', render: (r) => <StatusChip status={r.autonomy} /> },
                {
                  key: 'clean',
                  label: 'clean cycles',
                  nowrap: true,
                  render: (r) => (
                    <span className="inline-flex items-center gap-2">
                      <Bar value={Number(r.clean_cycles_so_far) || 0} max={Number(r.clean_cycles_required) || 1} tone="ok" />
                      <span className="tnum text-[12.5px]">{r.clean_cycles_so_far + ' / ' + r.clean_cycles_required}</span>
                    </span>
                  ),
                },
                { key: 'incidents', label: 'incidents', align: 'right', render: (r) => r.incidents },
              ]}
              rows={e.scope}
              rowKey={(r) => r.file_class}
            />
          ) : (
            <EmptyState>The constitution has no editable-scope rows yet.</EmptyState>
          )}
        </Panel>
        <div className="grid gap-3 lg:grid-cols-2">
          <Panel title="Product scope">
            <div className="grid gap-1 text-[13px]">
              {e.product_notes.map((n, i) => (
                <div key={i}>{'\u00b7 ' + n}</div>
              ))}
            </div>
            <p className="mt-2 text-[13px]">
              <strong>verdict: </strong>
              {s.product_verdict || ''}
            </p>
          </Panel>
          <Panel title="Hygiene (branches and worktrees the loop must remove)">
            {e.hygiene.length ? (
              <DataTable
                columns={[
                  { key: 'kind', label: 'kind', render: (h) => <Chip tone="warn">{h.kind}</Chip> },
                  { key: 'value', label: 'name', render: (h) => <Mono>{h.value}</Mono> },
                ]}
                rows={e.hygiene}
                rowKey={(h) => h.kind + h.value}
                dense
              />
            ) : (
              <p className="m-0 text-[13px] text-muted">{s.hygiene_git === '0' ? 'not a git repository' : 'clean: main tree only, no evolve/* branches'}</p>
            )}
          </Panel>
        </div>
      </Stack>
    </>
  );
}
