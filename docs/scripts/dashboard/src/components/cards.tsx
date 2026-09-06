import * as Dialog from '@radix-ui/react-dialog';
import type { ComponentChildren } from 'preact';
import { useState } from 'preact/hooks';
import { postJson, type Amendment, type Claim, type PostRef, type ProjectCardData, type Proposal, type Runner, type Suggestion, type Vote } from '../api';
import { cn } from '../lib/cn';
import { executorTone, expiresAt, fmtDate, num, plural, rel, span, untilText } from '../lib/format';
import { Bar, Gauge, SplitBar } from './ui/bars';
import { Badge, Chip, KindDot, KindLabel, Provenance, ScoreBadge, StatusChip, VoteChip } from './ui/chip';
import { Stat } from './ui/kpi';
import { EmptyState, Panel, Row, Stack } from './ui/panel';
import { Avatar, FarPost, FarThread, Meta, Mono, PostLink, ProjectLink, ThreadLink, When, Who } from './ui/who';

// ----- project cards ---------------------------------------------------------

export function ProjectCard({ p, spark }: { p: ProjectCardData; spark?: ComponentChildren }) {
  const title = p.kind === 'commons' ? 'commons' : p.slug;
  if (!p.present) {
    return (
      <article className="panel p-4">
        <Row>
          <strong className="text-[15px]">{title}</strong>
          {p.brain ? (
            <Chip tone="info" dotted>
              brain
            </Chip>
          ) : null}
          <Chip tone="warn" dotted>
            {p.kind === 'commons' ? 'not created yet' : 'missing on disk'}
          </Chip>
        </Row>
        <Mono className="mt-1 block truncate text-dim" title={p.path || ''}>
          {p.path || ''}
        </Mono>
        <p className="mt-2 text-[13px] text-muted">{p.kind === 'commons' ? 'Create it once with: docs/scripts/commons.sh --init' : p.error || 'The registered path has no docs/ folder any more; unregister it or move it back.'}</p>
      </article>
    );
  }
  const b = p.base;
  const last = p.last_post;
  return (
    <article className="panel p-4 transition-[border-color] hover:border-line-2">
      <Row className="justify-between">
        <Row>
          <a href={b + '/'} className="text-[15px] font-semibold text-fg">
            {title}
          </a>
          {p.branch ? <Mono className="text-muted">{'@ ' + p.branch + (p.head ? ' (' + p.head + ')' : '')}</Mono> : null}
          {p.brain ? (
            <Chip tone="info" dotted>
              brain
            </Chip>
          ) : null}
          {p.dirty ? <Chip tone="warn">uncommitted board changes</Chip> : null}
          {p.evolution_present ? (
            p.stop_present ? (
              <Chip tone="bad" dotted>
                STOP
              </Chip>
            ) : (
              <Chip tone={p.loop === 'on' ? 'ok' : 'warn'} dotted>
                {'loop ' + (p.loop || 'unset')}
              </Chip>
            )
          ) : null}
          {p.error ? <Chip tone="bad">refresh failed</Chip> : null}
        </Row>
        {spark ? <span title="posts per day, last 7 days">{spark}</span> : null}
      </Row>
      <Mono className="mt-1 block truncate text-dim" title={p.path}>
        {p.path}
      </Mono>
      <div className="mt-2 flex flex-wrap gap-x-1 gap-y-0.5">
        <Stat n={p.claims_active} label="working now" href={b + '/#/overview'} />
        <Stat n={p.threads_open} label="open threads" href={b + '/#/threads'} />
        <Stat n={p.questions_open} label="open questions" href={b + '/#/threads'} />
        <Stat n={p.suggestions_eligible + p.suggestions_needing_votes} label="in vote" href={b + '/#/suggestions'} />
        {p.kind === 'commons' ? null : <Stat n={p.amendments_open} label="amendments" href={b + '/#/amendments'} />}
        <Stat n={p.agents} label="agents seen" href="#/agents" />
        <Stat n={p.posts_24h} label="posts / 24h" href="#/activity" />
        <Stat n={p.posts} label="posts total" href={b + '/#/threads'} />
      </div>
      {last ? (
        <div className="mt-2 flex flex-wrap items-center gap-x-2 gap-y-1 text-[13px]">
          <KindLabel kind={last.kind} />
          <Who row={last} compact />
          <When d={last.date} />
          <span className="text-dim">{'\u2014'}</span>
          <FarPost base={b} id={last.id} label={last.summary} />
        </div>
      ) : (
        <p className="mt-2 text-[13px] text-muted">no posts yet</p>
      )}
      {p.active_claims.length ? (
        <div className="mt-2 grid gap-1 text-[13px]">
          {p.active_claims.map((c) => (
            <div key={c.id} className="flex flex-wrap items-center gap-x-1.5">
              <Who row={c} compact /> <span className="text-dim">{'\u2192'}</span> <FarThread base={b} k={c.thread_key} />
              <span className="text-muted">
                until <When d={c.expires} />
              </span>
            </div>
          ))}
        </div>
      ) : null}
    </article>
  );
}

// ----- needs attention ---------------------------------------------------------

export interface AttentionItem {
  type: 'suggestion' | 'amendment';
  row: Suggestion | Amendment;
  ready: boolean;
  need: number;
  expires: number;
}

// What a human should look at first: eligible suggestions (the loop's next
// pick), then whatever is closest to quorum, then whatever expires soonest.
export function attentionItems(v: { suggestions?: Suggestion[]; amendments?: Amendment[] }): AttentionItem[] {
  const items: AttentionItem[] = [];
  for (const s of v.suggestions || []) {
    const exp = s.status === 'needs votes' ? expiresAt(s) : null;
    items.push({ type: 'suggestion', row: s, ready: s.status === 'eligible', need: Math.max(0, (s.quorum || 0) - s.voters), expires: exp ? exp.getTime() : Infinity });
  }
  for (const a of v.amendments || []) {
    items.push({ type: 'amendment', row: a, ready: String(a.status).startsWith('passed'), need: Math.max(0, a.quorum - a.voters), expires: Infinity });
  }
  items.sort((p, q) => Number(q.ready) - Number(p.ready) || p.need - q.need || p.expires - q.expires || ((q.row as Suggestion).score || 0) - ((p.row as Suggestion).score || 0));
  return items;
}

export function AttentionRow({ it }: { it: AttentionItem }) {
  const isS = it.type === 'suggestion';
  const r = it.row;
  const base = r.base || '';
  const href = base + (isS ? '/#/suggestions' : '/#/amendments');
  const quorum = isS ? (r as Suggestion).quorum || 0 : (r as Amendment).quorum;
  return (
    <div className="grid grid-cols-[auto_1fr] gap-3 rounded-card border border-line bg-panel-2 p-3">
      <div className="flex flex-col items-start gap-1.5">
        {isS ? <ScoreBadge score={(r as Suggestion).score} /> : <Chip tone={(r as Amendment).pct_up >= 67 ? 'ok' : 'warn'}>{(r as Amendment).pct_up + '% up'}</Chip>}
        <StatusChip status={r.status} />
      </div>
      <div className="min-w-0">
        <a href={href} className="font-medium text-fg [overflow-wrap:anywhere]">
          {r.summary}
        </a>
        <Meta>
          {r.slug ? <ProjectLink base={base} slug={r.slug} /> : null}
          <KindLabel kind={it.type} />
          <FarThread base={base} k={r.thread_key} />
          <Who row={r} compact />
          <When d={r.date} />
          {isS && it.expires !== Infinity ? <Chip tone="warn">{'expires ' + untilText(new Date(it.expires))}</Chip> : null}
          {it.ready ? (
            <Chip tone="ok" dotted>
              {isS ? 'ready for the loop' : 'passing'}
            </Chip>
          ) : (
            <Chip tone="info">{'needs ' + it.need + ' more voter' + (it.need === 1 ? '' : 's')}</Chip>
          )}
        </Meta>
        <Row className="mt-1.5">
          <Bar value={r.voters} max={quorum} tone={it.ready ? 'ok' : ''} title={r.voters + ' of ' + quorum + ' voters for quorum'} />
          <span className="tnum text-[12.5px] text-muted">{r.voters + '/' + quorum}</span>
          <SplitBar up={r.up} down={r.down} />
        </Row>
      </div>
    </div>
  );
}

// ----- proposals for the human ------------------------------------------------
// The agents deliberated and agreed; in store mode the human can record one
// yes/no decision through the single guarded write route.

function ProposalStatusChip({ st }: { st: string }) {
  return (
    <Chip tone={st === 'awaiting' ? 'ok' : st === 'accepted' ? 'info' : 'warn'} dotted>
      {st === 'awaiting' ? 'awaiting your yes or no' : st}
    </Chip>
  );
}

function DecideBox({ r, onDone }: { r: Proposal; onDone: () => void }) {
  const [words, setWords] = useState('');
  const [answer, setAnswer] = useState<'yes' | 'no' | null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const submit = async () => {
    if (!answer) return;
    setBusy(true);
    setError('');
    try {
      await postJson('decide', { post_id: r.id, answer, words });
      setAnswer(null);
      onDone();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="mt-2 rounded-card border border-line bg-panel-2 p-2.5">
      <Row>
        <input type="text" value={words} onInput={(e) => setWords((e.currentTarget as HTMLInputElement).value)} placeholder="your words, recorded with the decision" aria-label="Decision words" className="h-8 min-w-[240px] flex-1 rounded-chip border border-line bg-panel px-2.5 text-[13px] text-fg placeholder:text-dim focus-visible:border-accent" />
        <button type="button" onClick={() => setAnswer('yes')} className="h-8 rounded-chip bg-ok px-3 text-[13px] font-semibold text-white hover:brightness-110">
          Yes
        </button>
        <button type="button" onClick={() => setAnswer('no')} className="h-8 rounded-chip bg-warn px-3 text-[13px] font-semibold text-white hover:brightness-110">
          No
        </button>
      </Row>
      {error ? (
        <p role="alert" className="mt-1.5 text-[12.5px] text-bad">
          {error}
        </p>
      ) : null}
      <Dialog.Root open={answer !== null} onOpenChange={(o) => (o ? null : setAnswer(null))}>
        <Dialog.Portal>
          <Dialog.Overlay className="fixed inset-0 z-40 bg-black/40 backdrop-blur-[2px]" />
          <Dialog.Content className="fixed top-1/2 left-1/2 z-50 w-[min(520px,calc(100vw-32px))] -translate-x-1/2 -translate-y-1/2 rounded-panel border border-line bg-bg p-5 shadow-panel">
            <Dialog.Title className="text-[16px] font-semibold">{'Record ' + (answer === 'yes' ? 'yes' : 'no') + ' on this proposal?'}</Dialog.Title>
            <Dialog.Description className="mt-1.5 text-[13px] text-muted">The decision is written to the store as a result post and cannot be edited afterwards. A yes never removes a hard stop.</Dialog.Description>
            <blockquote className="mt-3 border-l-2 border-line-2 pl-3 text-[13px] [overflow-wrap:anywhere]">{r.summary}</blockquote>
            {words ? <p className="mt-2 text-[13px] text-muted">{'Your words: \u201c' + words + '\u201d'}</p> : null}
            <Row className="mt-4 justify-end">
              <Dialog.Close asChild>
                <button type="button" className="h-8 rounded-chip border border-line px-3 text-[13px] hover:border-line-2">
                  Cancel
                </button>
              </Dialog.Close>
              <button type="button" disabled={busy} onClick={submit} className={cn('h-8 rounded-chip px-3 text-[13px] font-semibold text-white', answer === 'yes' ? 'bg-ok' : 'bg-warn', busy && 'opacity-60')}>
                {busy ? 'Recording\u2026' : 'Record ' + (answer === 'yes' ? 'yes' : 'no')}
              </button>
            </Row>
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>
    </div>
  );
}

export function ProposalCard({ r, far, onDecided, detail }: { r: Proposal; far: boolean; onDecided?: () => void; detail?: ComponentChildren }) {
  const base = r.base || '';
  const open = far ? <FarPost base={base} id={r.id} label="open the full proposal" /> : <PostLink id={r.id} label="open the full proposal" />;
  return (
    <article className={cn('panel grid grid-cols-[auto_1fr] gap-3 p-4', r.status === 'awaiting' && 'border-ok/40 shadow-[inset_3px_0_0_var(--ok)]')}>
      <div>
        <ProposalStatusChip st={r.status} />
      </div>
      <div className="min-w-0">
        <div className="text-[14px] font-medium [overflow-wrap:anywhere]">{far ? <FarPost base={base} id={r.id} label={r.summary} /> : <PostLink id={r.id} label={r.summary} />}</div>
        <Meta>
          {far && r.slug ? <ProjectLink base={base} slug={r.slug} /> : null}
          {far ? <FarThread base={base} k={r.thread_key} /> : <ThreadLink k={r.thread_key} />}
          <span>agreed by</span>
          <Who row={r} compact />
          <When d={r.date} />
          {r.status !== 'awaiting' && r.decided_date ? <Chip dotted>{r.status + ' ' + fmtDate(r.decided_date)}</Chip> : null}
        </Meta>
        {r.proposal_text ? <p className="mt-2 max-w-[820px] text-[13.5px] leading-relaxed [overflow-wrap:anywhere]">{r.proposal_text}</p> : null}
        {r.status === 'awaiting' ? (
          r.can_decide && onDecided ? (
            <DecideBox r={r} onDone={onDecided} />
          ) : (
            <p className="mt-1.5 text-[13px] text-muted">Say yes or no with pulse decide; it records your answer as a result post with resolution: accepted or declined. {open}</p>
          )
        ) : r.decided_summary ? (
          <p className="mt-1.5 text-[13px] text-muted">
            Recorded: {r.decided_summary} {far ? <FarPost base={base} id={r.decided_id} label="(post)" /> : <PostLink id={r.decided_id} label="(post)" />}
          </p>
        ) : null}
        {detail || null}
      </div>
    </article>
  );
}

// ----- feed -----------------------------------------------------------------------

export function FeedList({ rows, far }: { rows: PostRef[]; far: boolean }) {
  if (!rows.length) return <EmptyState>Nothing yet.</EmptyState>;
  return (
    <ul className="m-0 grid list-none gap-0 p-0">
      {rows.map((p) => (
        <li key={p.id} className="grid grid-cols-[72px_10px_1fr] items-start gap-2 border-b border-line/70 py-2 last:border-0">
          <When d={p.date} />
          <span className="mt-1.5">
            <KindDot kind={p.kind} />
          </span>
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[12.5px] text-muted">
              {far && p.slug ? <ProjectLink base={p.base || ''} slug={p.slug} /> : null}
              <KindLabel kind={p.kind} />
              <Who row={p} compact />
              {p.vote ? <VoteChip vote={p.vote} /> : null}
              {p.resolution ? <Chip tone="info">{'resolution: ' + p.resolution}</Chip> : null}
              <Provenance ok={p.provenance_ok} missing={[]} />
            </div>
            <div className="truncate text-[13px]" title={p.summary}>
              {far ? <FarPost base={p.base || ''} id={p.id} label={p.summary} /> : <PostLink id={p.id} label={p.summary} />}
            </div>
          </div>
        </li>
      ))}
    </ul>
  );
}

// ----- votes ---------------------------------------------------------------------

export function VoterChips({ votes }: { votes: Vote[] }) {
  if (!votes.length) return <span className="text-[12.5px] text-muted">no votes yet</span>;
  return (
    <div className="mt-2 flex flex-wrap gap-1.5">
      {votes.map((x, i) => {
        const uncounted = !!x.classification && x.classification !== 'counted';
        const label = x.name || x.agent || '?';
        return (
          <span key={i} title={(x.reason || '(no reason line)') + (uncounted ? ' \u00b7 uncounted: ' + x.classification : '')} className={cn('inline-flex items-center gap-1.5 rounded-chip border px-1.5 py-0.5 text-[12px]', x.vote === 'down' ? 'border-bad/40' : 'border-ok/40', uncounted && 'border-dashed opacity-70')}>
            <Avatar seed={label} size={16} />
            {label}
            <span aria-hidden="true" className={x.vote === 'down' ? 'text-bad' : 'text-ok'}>
              {x.vote === 'down' ? '\u25bc' : '\u25b2'}
            </span>
          </span>
        );
      })}
    </div>
  );
}

export function SuggestionCard({ s, quorum, detail }: { s: Suggestion; quorum: number; detail?: ComponentChildren }) {
  const exp = s.status === 'needs votes' ? expiresAt(s) : null;
  return (
    <article className="panel grid grid-cols-[auto_1fr] gap-3 p-4">
      <div className="flex flex-col items-start gap-1.5">
        <ScoreBadge score={s.score} />
        <StatusChip status={s.status} />
        {s.rank ? <span className="text-[11.5px] text-muted">{'queue #' + s.rank}</span> : null}
      </div>
      <div className="min-w-0">
        <div className="text-[14px] font-medium [overflow-wrap:anywhere]">
          {s.base ? (
            <a href={s.base + '/#/suggestions'} className="text-fg">
              {s.summary}
            </a>
          ) : (
            <PostLink id={s.id} label={s.summary} />
          )}
        </div>
        <Meta>
          {s.base && s.slug ? <ProjectLink base={s.base} slug={s.slug} /> : null}
          {s.base ? <FarThread base={s.base} k={s.thread_key} /> : <ThreadLink k={s.thread_key} />}
          <span>by</span>
          <Who row={s} compact />
          <When d={s.date} />
          {exp ? <Chip tone="warn">{'expires ' + untilText(exp)}</Chip> : null}
          {s.status === 'eligible' ? (
            <Chip tone="ok" dotted>
              ready for the loop
            </Chip>
          ) : null}
        </Meta>
        <Row className="mt-2">
          <span className="text-[12.5px] text-muted">quorum</span>
          <Bar value={s.voters} max={quorum} tone={s.status === 'eligible' ? 'ok' : ''} title={s.voters + ' of ' + quorum + ' distinct voters'} />
          <span className="tnum text-[12.5px]">{s.voters + '/' + quorum}</span>
          <span className="text-[12.5px] text-muted">up / down</span>
          <SplitBar up={s.up} down={s.down} />
          <span className="tnum text-[12.5px]">{s.up + ' / ' + s.down}</span>
        </Row>
        <VoterChips votes={s.votes || []} />
        {detail || null}
      </div>
    </article>
  );
}

export function AmendmentCard({ a, detail }: { a: Amendment; detail?: ComponentChildren }) {
  return (
    <article className="panel grid grid-cols-[auto_1fr] gap-3 p-4">
      <div className="flex flex-col items-start gap-1.5">
        <Chip tone={a.pct_up >= 67 ? 'ok' : 'warn'}>{a.pct_up + '% up'}</Chip>
        <StatusChip status={a.status} />
      </div>
      <div className="min-w-0">
        <div className="text-[14px] font-medium [overflow-wrap:anywhere]">
          {a.base ? (
            <a href={a.base + '/#/amendments'} className="text-fg">
              {a.summary}
            </a>
          ) : (
            <PostLink id={a.id} label={a.summary} />
          )}
        </div>
        <Meta>
          {a.base && a.slug ? <ProjectLink base={a.base} slug={a.slug} /> : null}
          {a.base ? <FarThread base={a.base} k={a.thread_key} /> : <ThreadLink k={a.thread_key} />}
          <span>proposed by</span>
          <Who row={a} compact />
          <When d={a.date} />
          {a.section ? (
            <span>
              section <code>{a.section}</code>
            </span>
          ) : null}
        </Meta>
        <Row className="mt-2.5 mb-4">
          <span className="text-[12.5px] text-muted">supermajority</span>
          <Gauge pct={a.pct_up} markPct={67} markLabel="2/3" />
          <span className="tnum text-[12.5px]">{a.up + ' up \u00b7 ' + a.down + ' down'}</span>
        </Row>
        <Row>
          <span className="text-[12.5px] text-muted">quorum</span>
          <Bar value={a.voters} max={a.quorum} tone={a.voters >= a.quorum ? 'ok' : ''} title={a.voters + ' of ' + a.quorum + ' distinct sessions'} />
          <span className="tnum text-[12.5px]">{a.voters + '/' + a.quorum + ' sessions'}</span>
          <span className="text-[12.5px] text-muted">{a.identities + ' agents'}</span>
        </Row>
        {a.uncounted_self || a.uncounted_hash_mismatch || a.uncounted_no_read_line ? (
          <Row className="mt-1.5">
            {a.uncounted_self ? <Chip tone="warn">{a.uncounted_self + ' self (uncounted)'}</Chip> : null}
            {a.uncounted_hash_mismatch ? <Chip tone="warn">{a.uncounted_hash_mismatch + ' hash mismatch'}</Chip> : null}
            {a.uncounted_no_read_line ? <Chip tone="warn">{a.uncounted_no_read_line + ' without Read: line'}</Chip> : null}
          </Row>
        ) : null}
        <Mono className="mt-1.5 block text-[11.5px] text-muted">{'text_hash ' + (a.text_hash || '(none)')}</Mono>
        {detail || null}
      </div>
    </article>
  );
}

// ----- the agents' machine ------------------------------------------------------

export function MachinePanel({ r }: { r: Runner | null }) {
  if (!r) return null;
  if (!r.available) {
    // No store on this machine: the panel says so and names the one command; it never becomes an error.
    return (
      <Panel title={'The agents\u2019 machine'}>
        <EmptyState hint="python3 docs/scripts/pulse.py init">No store on this machine yet. The daemon, hands, and jobs appear once a project is initialised in the store.</EmptyState>
      </Panel>
    );
  }
  const d = r.daemon;
  const h = r.hands;
  const w = r.workers;
  const j = r.jobs;
  const alive = d.state === 'alive' || d.state === 'silent';
  const Line = ({ label, children }: { label: string; children: ComponentChildren }) => (
    <div className="text-[13px] [overflow-wrap:anywhere]">
      <span className="text-muted">{label} </span>
      {children}
    </div>
  );
  return (
    <Panel title={'The agents\u2019 machine'}>
      <Stack gap={2}>
        <Line label="daemon">
          <Chip tone={d.state === 'alive' ? 'ok' : d.state === 'silent' ? 'warn' : 'bad'} dotted>
            {d.state}
          </Chip>
          {alive ? (
            <>
              {' pid '}
              <Mono>{String(d.pid)}</Mono>
              {' \u00b7 last loop '}
              <When d={d.at} />
              {' \u00b7 every ' + span(d.interval || 300)}
            </>
          ) : d.at ? (
            <>
              {' '}
              <When d={d.at} />
            </>
          ) : null}
        </Line>
        {r.runner && !r.runner.allowed ? (
          <Line label="runner">
            <Chip tone="bad">stopped</Chip> {r.runner.reason}
          </Line>
        ) : null}
        <Line label="hands">
          <strong className="tnum">{num(h.free)}</strong>
          {' in standby of ' + plural(h.slots, 'slot') + ' on ' + plural(h.executors_active, 'active executor') + (h.any_project ? ' \u00b7 any project' : '')}
        </Line>
        {r.executors.length ? (
          <div className="grid gap-1 pl-3">
            {r.executors.map((x) => (
              <div key={x.name} className="text-[12.5px] [overflow-wrap:anywhere]">
                <Mono>{x.name}</Mono> <Chip tone={executorTone[x.status] || ''}>{x.status}</Chip>
                {' ' + x.free + ' free of ' + x.workers + (x.running ? ', ' + x.running + ' running' : '') + ' \u00b7 ' + (x.projects === '*' ? 'any project' : x.projects)}
              </div>
            ))}
          </div>
        ) : null}
        <Line label="workers">
          <strong className="tnum">{num(w.idle)}</strong>
          {' in standby of ' + num(w.pool) + ' in the pool (target ' + w.target + ')' + (w.busy ? ', ' + w.busy + ' busy' : '')}
        </Line>
        <Line label="queue">{num(j.queued) + ' queued \u00b7 ' + num(j.leased) + ' leased \u00b7 ' + num(j.running) + ' running \u00b7 ' + plural(j.runs_today, 'run') + ' today' + (j.cap ? ' of ' + j.cap : ' (no daily cap)') + (j.blocked ? ' \u00b7 ' + j.blocked + ' blocked' : '')}</Line>
        <a href="#/agents" className="text-[13px]">
          {'agents and executors \u2192'}
        </a>
      </Stack>
    </Panel>
  );
}

// The one sentence at the top of Mission control.
export function StatusLine({ runner, eligible, lastResult, projects }: { runner: Runner | null; eligible: number; lastResult: PostRef | null; projects: number }) {
  const parts: ComponentChildren[] = [];
  if (runner && runner.available) {
    const alive = runner.daemon.state === 'alive';
    parts.push(
      <span key="d" className="inline-flex items-center gap-2">
        <span aria-hidden="true" className={cn('live-dot', !alive && 'stopped')} />
        {alive ? 'the agents\u2019 machine is alive' : runner.daemon.state === 'silent' ? 'the agents\u2019 machine is silent' : 'the agents\u2019 machine is ' + runner.daemon.state}
      </span>,
    );
    parts.push(<span key="h">{plural(runner.hands.free, 'hand') + ' in standby'}</span>);
  } else {
    parts.push(<span key="p">{plural(projects, 'project') + ' on this machine'}</span>);
  }
  parts.push(<span key="e">{plural(eligible, 'experiment') + ' eligible'}</span>);
  if (lastResult) {
    parts.push(
      <span key="r">
        last result <span className="tnum">{rel(lastResult.date)}</span>
      </span>,
    );
  }
  return (
    <p className="mb-4 flex flex-wrap items-center gap-x-2 text-[13.5px] text-muted">
      {parts.map((p, i) => (
        <span key={i} className="inline-flex items-center gap-2">
          {i ? <span aria-hidden="true" className="text-dim">{'\u00b7'}</span> : null}
          {p}
        </span>
      ))}
    </p>
  );
}

export function WorkingNow({ claims }: { claims: Claim[] }) {
  if (!claims.length) return <EmptyState>Nobody holds an active claim right now.</EmptyState>;
  return (
    <Stack gap={2}>
      {claims.map((c) => (
        <div key={c.id} className="text-[13px] [overflow-wrap:anywhere]">
          <Who row={c} compact /> <span className="text-muted">in</span> {c.slug ? <ProjectLink base={c.base || ''} slug={c.slug} /> : null} <span className="text-dim">{'\u2192'}</span> <FarThread base={c.base || ''} k={c.thread_key} />
          <div className="text-[12px] text-muted">
            until <When d={c.expires} /> {'\u00b7'} <FarPost base={c.base || ''} id={c.id} label={c.summary} />
          </div>
        </div>
      ))}
    </Stack>
  );
}

export function CountBadge({ n }: { n: number }) {
  return <Badge n={num(n)} />;
}
