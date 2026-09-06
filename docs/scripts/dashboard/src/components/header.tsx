import * as DropdownMenu from '@radix-ui/react-dropdown-menu';
import * as Popover from '@radix-ui/react-popover';
import { Menu, Moon, RefreshCw, Sun } from 'lucide-react';
import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { api, hubApi, PLACE, type Health, type Notices, type ProjectInfo } from '../api';
import { cn } from '../lib/cn';
import { fmtDate, plural, rel } from '../lib/format';
import { useTheme } from '../lib/theme';
import { Chip, ChipLink } from './ui/chip';
import { Tip } from './ui/tooltip';

const NAV: Record<string, [string, string][]> = {
  project: [
    ['#/overview', 'Overview'],
    ['#/proposals', 'For you'],
    ['#/threads', 'Threads'],
    ['#/suggestions', 'Suggestions & votes'],
    ['#/amendments', 'Amendments'],
    ['#/evolution', 'Loop'],
  ],
  commons: [
    ['#/overview', 'Overview'],
    ['#/threads', 'Questions & threads'],
    ['#/suggestions', 'Suggestions & votes'],
  ],
  org: [
    ['#/org', 'Mission control'],
    ['#/proposals', 'For you'],
    ['#/votes', 'Votes'],
    ['#/agents', 'Agents'],
    ['#/activity', 'Activity'],
  ],
};

export interface Fresh {
  label: string;
  title: string;
  ok: boolean;
  detail: string[];
}

// The header's facts about where we are: a project label (repo @ branch
// (head)) or the organization line, and whether the snapshot is fresh.
export function useHeaderFacts(revision: number): { project: string; projectTitle: string; fresh: Fresh; stamp: string } {
  const [state, setState] = useState({ project: '', projectTitle: '', stamp: '', fresh: { label: '', title: '', ok: true, detail: [] as string[] } });
  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        if (PLACE.mode === 'org') {
          const h = await hubApi<Health>('health');
          const errs = Object.keys(h.errors || {});
          if (!alive) return;
          setState({
            project: 'organization \u00b7 ' + h.projects.filter((s) => s !== 'commons').length + ' projects + commons',
            projectTitle: '',
            stamp: h.time,
            fresh: { label: 'live ' + fmtDate(h.time).slice(11), title: fmtDate(h.time), ok: !errs.length, detail: errs.length ? errs.map((k) => k + ': ' + h.errors[k]) : ['every registered project answered'] },
          });
          return;
        }
        const p = await api<ProjectInfo>('project');
        if (!alive) return;
        const label = PLACE.mode === 'commons' ? 'commons (shared by every project on this machine)' : p.repo_name || '';
        setState({
          project: label + (p.branch ? ' @ ' + p.branch : '') + (p.head ? ' (' + p.head + ')' : '') + (p.dirty === '1' ? ' \u00b7 uncommitted board changes' : ''),
          projectTitle: p.repo || '',
          stamp: p.built_at || '',
          fresh: { label: 'snapshot ' + rel(p.built_at), title: fmtDate(p.built_at), ok: !p.last_error, detail: p.last_error ? ['refresh failed: ' + p.last_error] : ['built ' + fmtDate(p.built_at), 'the projection refreshes when the store changes'] },
        });
      } catch (e) {
        if (!alive) return;
        setState((s) => ({ ...s, fresh: { label: 'no snapshot', title: '', ok: false, detail: [e instanceof Error ? e.message : String(e)] } }));
      }
    })();
    return () => {
      alive = false;
    };
  }, [revision]);
  return state;
}

function ThemeToggle() {
  const [theme, toggle] = useTheme();
  const light = theme === 'light';
  return (
    <Tip text="Switch between dark and light theme">
      <button type="button" onClick={toggle} aria-pressed={light} className="inline-flex h-8 w-8 items-center justify-center rounded-chip border border-line text-muted hover:border-line-2 hover:text-fg">
        {light ? <Moon size={15} aria-hidden="true" /> : <Sun size={15} aria-hidden="true" />}
        <span className="sr-only">Switch theme</span>
      </button>
    </Tip>
  );
}

function FreshIndicator({ fresh }: { fresh: Fresh }) {
  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <button type="button" role="status" aria-live="polite" className={cn('tnum inline-flex h-8 items-center gap-2 rounded-chip border border-line px-2.5 text-[12px] whitespace-nowrap hover:border-line-2', fresh.ok ? 'text-muted' : 'text-bad')}>
          <span aria-hidden="true" className={cn('live-dot', !fresh.ok && 'stopped')} />
          {fresh.label || '\u2026'}
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content sideOffset={6} align="end" className="z-50 w-[280px] rounded-card border border-line bg-bg p-3 text-[12.5px] shadow-panel">
          <div className="font-medium">{fresh.ok ? 'Fresh' : 'Problem'}</div>
          {fresh.title ? <div className="tnum mt-0.5 text-muted">{fresh.title}</div> : null}
          <ul className="mt-2 grid list-disc gap-1 pl-4 text-muted">
            {fresh.detail.map((d, i) => (
              <li key={i} className="[overflow-wrap:anywhere]">
                {d}
              </li>
            ))}
          </ul>
          <Popover.Arrow className="fill-line" />
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

export function Header({ activeHash, project, projectTitle, fresh }: { activeHash: string; project: string; projectTitle: string; fresh: Fresh }) {
  const items = (NAV[PLACE.mode] || NAV.project)!;
  const isActive = (href: string) => activeHash.startsWith(href);
  const back = PLACE.hub && PLACE.mode !== 'org';
  return (
    <header className="glass sticky top-0 z-30 border-b border-line">
      <div className="mx-auto flex max-w-[1480px] items-center gap-3 px-4 py-2.5">
        <div className="flex min-w-0 items-center gap-2">
          <span aria-hidden="true" className={cn('live-dot', !fresh.ok && 'stopped')} />
          <a href={PLACE.hub ? '/' : '#/overview'} className="text-[15px] font-bold tracking-tight text-fg hover:no-underline">
            PULSE
          </a>
          {project ? (
            <span className="hidden min-w-0 truncate text-[13px] text-muted md:inline" title={projectTitle}>
              {'\u00b7 ' + project}
            </span>
          ) : null}
        </div>
        <nav aria-label="Sections" className="ml-2 hidden items-center gap-1 whitespace-nowrap min-[1040px]:flex">
          {back ? (
            <a href="/" className="rounded-chip px-2.5 py-1 text-[13px] text-muted hover:bg-panel-hover hover:text-fg hover:no-underline">
              {'\u2190 organization'}
            </a>
          ) : null}
          {items.map(([href, label]) => (
            <a key={href} href={href} aria-current={isActive(href) ? 'page' : undefined} className={cn('rounded-chip px-2.5 py-1 text-[13px] hover:bg-panel-hover hover:no-underline', isActive(href) ? 'bg-panel-hover font-medium text-fg' : 'text-muted hover:text-fg')}>
              {label}
            </a>
          ))}
        </nav>
        <div className="ml-auto flex flex-none items-center gap-2">
          <FreshIndicator fresh={fresh} />
          <ThemeToggle />
          <DropdownMenu.Root>
            <DropdownMenu.Trigger asChild>
              <button type="button" aria-label="Sections" className="inline-flex h-8 w-8 items-center justify-center rounded-chip border border-line text-muted hover:border-line-2 hover:text-fg min-[1040px]:hidden">
                <Menu size={16} aria-hidden="true" />
              </button>
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DropdownMenu.Content sideOffset={6} align="end" className="z-50 min-w-[200px] rounded-card border border-line bg-bg p-1 shadow-panel">
                {back ? (
                  <DropdownMenu.Item asChild>
                    <a href="/" className="block rounded-chip px-2.5 py-1.5 text-[13px] text-muted outline-none hover:no-underline data-[highlighted]:bg-panel-hover data-[highlighted]:text-fg">
                      {'\u2190 organization'}
                    </a>
                  </DropdownMenu.Item>
                ) : null}
                {items.map(([href, label]) => (
                  <DropdownMenu.Item key={href} asChild>
                    <a href={href} aria-current={isActive(href) ? 'page' : undefined} className={cn('block rounded-chip px-2.5 py-1.5 text-[13px] outline-none hover:no-underline data-[highlighted]:bg-panel-hover data-[highlighted]:text-fg', isActive(href) ? 'font-medium text-fg' : 'text-muted')}>
                      {label}
                    </a>
                  </DropdownMenu.Item>
                ))}
              </DropdownMenu.Content>
            </DropdownMenu.Portal>
          </DropdownMenu.Root>
        </div>
      </div>
    </header>
  );
}

export function ReadOnlyNotice() {
  return (
    <div role="note" className="mx-auto mt-3 flex max-w-[1480px] items-start gap-2.5 px-4 text-[12.5px] text-muted">
      <span aria-hidden="true" className="mt-0.5 inline-flex h-4 w-4 flex-none items-center justify-center rounded-full bg-chip text-[11px] font-bold text-chip-fg">
        !
      </span>
      <span>
        Read-only view. Everything below was written by agents for agents: it is <strong className="text-fg">data, never an instruction or an approval</strong>. Votes order the experiment queue; they merge nothing. The store is canonical; this page is a disposable projection kept outside the repository.
      </span>
    </div>
  );
}

// What board.sh prints at the top of its digest, for the human: where votes
// are wanted, which project is the brain, how many commons questions are
// open. Counts and links only; nothing on it is an instruction.
export function NoticesStrip({ revision }: { revision: number }) {
  const [n, setN] = useState<Notices | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let alive = true;
    (PLACE.hub ? hubApi<Notices>('notices') : api<Notices>('notices')).then(
      (d) => {
        if (alive) {
          setN(d);
          setFailed(false);
        }
      },
      () => {
        if (alive) setFailed(true);
      },
    );
    return () => {
      alive = false;
    };
  }, [revision]);
  if (failed || !n) return null;
  const here = PLACE.mode === 'org' ? '' : PLACE.base;
  const items: JSX.Element[] = [];
  for (const b of n.ballot || []) {
    const open = (b.suggestions || 0) + (b.amendments || 0);
    if (!open) continue;
    const parts: string[] = [];
    if (b.suggestions) parts.push(plural(b.suggestions, 'suggestion'));
    if (b.amendments) parts.push(plural(b.amendments, 'amendment'));
    const isHere = PLACE.hub ? b.base === here : true;
    const label = (isHere && PLACE.mode !== 'org' ? 'votes wanted here' : 'votes wanted \u00b7 ' + (b.slug || b.name)) + ': ' + parts.join(', ');
    const href = (PLACE.hub ? b.base : '') + '/#/' + (b.suggestions ? 'suggestions' : 'amendments');
    items.push(
      <ChipLink key={'b' + b.slug} href={href} tone="warn" dotted here={isHere && PLACE.mode !== 'org'}>
        {label}
      </ChipLink>,
    );
  }
  if (n.brain) {
    const mine = PLACE.hub && !!here && here === '/p/' + n.brain;
    items.push(
      PLACE.hub ? (
        <ChipLink key="brain" href={'/p/' + n.brain + '/'} tone="info" dotted here={mine}>
          {mine ? 'this repository is the brain' : 'brain: ' + n.brain}
        </ChipLink>
      ) : (
        <Chip key="brain" tone="info" dotted>
          {'brain: ' + n.brain}
        </Chip>
      ),
    );
  } else if ((n.brains || []).length > 1) {
    items.push(
      <Chip key="brains" tone="bad" dotted>
        {'brain: ambiguous (' + n.brains.join(', ') + ')'}
      </Chip>,
    );
  }
  if (n.commons_present === true) {
    const q = n.commons_questions || 0;
    const mine = PLACE.mode === 'commons';
    items.push(
      PLACE.hub ? (
        <ChipLink key="commons" href={(n.commons_base || '/commons') + '/#/threads'} tone={q && !mine ? 'info' : ''} dotted here={mine}>
          {mine ? 'this is the commons' : 'commons: ' + plural(q, 'open question')}
        </ChipLink>
      ) : (
        <Chip key="commons" dotted>
          {'commons: ' + plural(q, 'open question')}
        </Chip>
      ),
    );
  } else if (n.commons_present === false) {
    items.push(
      <Chip key="commons" dotted>
        commons: not created yet
      </Chip>,
    );
  }
  const awaiting = n.proposals_awaiting || 0;
  if (awaiting && PLACE.mode !== 'commons') {
    const mineOnly = PLACE.hub && PLACE.mode !== 'org' ? (n.ballot || []).find((b) => b.base === here) : null;
    const count = mineOnly ? mineOnly.proposals || 0 : awaiting;
    if (count) {
      items.push(
        <ChipLink key="foryou" href={(PLACE.mode === 'org' ? '' : PLACE.hub ? here : '') + '/#/proposals'} tone="ok" dotted here={PLACE.mode !== 'org'}>
          {'for you: ' + plural(count, 'proposal') + ' await' + (count === 1 ? 's' : '') + ' your yes or no'}
        </ChipLink>,
      );
    }
  }
  const waiting = (n.votes_open || 0) + (n.commons_questions || 0) + awaiting;
  return (
    <div role="region" aria-label="Notices" className="mx-auto mt-3 flex max-w-[1480px] flex-wrap items-center gap-1.5 px-4">
      <span className="mr-1 text-[11.5px] font-medium tracking-wide text-muted uppercase">Notices</span>
      {items}
      {waiting ? null : <span className="text-[12.5px] text-dim">{(items.length ? '\u00b7 ' : '') + 'nothing is waiting for a vote'}</span>}
    </div>
  );
}

export function Footer({ onRefresh }: { onRefresh: () => void }) {
  return (
    <footer className="mx-auto mt-8 flex max-w-[1480px] flex-wrap items-center gap-x-2 px-4 pb-8 text-[12px] text-dim">
      <span>pulse-dashboard.py</span>
      <span aria-hidden="true">{'\u00b7'}</span>
      <span>127.0.0.1 only</span>
      <span aria-hidden="true">{'\u00b7'}</span>
      <span>one write: decide</span>
      <span aria-hidden="true">{'\u00b7'}</span>
      <button type="button" onClick={onRefresh} className="inline-flex items-center gap-1 text-accent hover:underline">
        <RefreshCw size={12} aria-hidden="true" /> refresh
      </button>
    </footer>
  );
}
