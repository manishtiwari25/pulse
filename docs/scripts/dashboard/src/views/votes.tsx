import * as Tabs from '@radix-ui/react-tabs';
import { useState } from 'preact/hooks';
import { hubApi, type Votes } from '../api';
import { AmendmentCard, SuggestionCard } from '../components/cards';
import { Badge } from '../components/ui/chip';
import { Count, EmptyState, Lead, Notice, PageTitle, Select, SkeletonPage, Stack, Toolbar } from '../components/ui/panel';
import { expiresAt, num } from '../lib/format';
import { useApi } from '../lib/use-api';
import type { ViewProps } from './types';

const TAB = 'inline-flex h-8 items-center gap-1.5 rounded-chip px-3 text-[13px] text-muted hover:text-fg data-[state=active]:bg-panel data-[state=active]:font-medium data-[state=active]:text-fg data-[state=active]:shadow-panel';

export function VotesView({ revision }: ViewProps) {
  const { data, error } = useApi<Votes>(() => hubApi<Votes>('votes'), 'votes', revision);
  const [slug, setSlug] = useState('');
  const [status, setStatus] = useState('');
  const [sort, setSort] = useState('queue');
  if (error && !data) return <Notice tone="alert">{'Could not load this view: ' + error}</Notice>;
  if (!data) return <SkeletonPage />;
  const slugs = [...new Set([...data.suggestions, ...data.amendments].map((s) => s.slug || ''))].filter(Boolean).sort();
  const list = data.suggestions.filter((s) => (!slug || s.slug === slug) && (!status || s.status === status));
  const far = (d: Date | null) => (d ? d.getTime() : Infinity);
  if (sort === 'quorum') list.sort((a, b) => Math.max(0, (a.quorum || 0) - a.voters) - Math.max(0, (b.quorum || 0) - b.voters) || b.score - a.score);
  else if (sort === 'expiry') list.sort((a, b) => far(expiresAt(a)) - far(expiresAt(b)));
  else if (sort === 'newest') list.sort((a, b) => (a.date < b.date ? 1 : -1));
  const ams = data.amendments.filter((a) => !slug || a.slug === slug);
  return (
    <>
      <PageTitle sub="across the organization">Votes</PageTitle>
      <Lead>Suggestions become eligible for the loop once enough distinct agents voted; amendments need a supermajority of informed sessions. Each project counts its own votes with its own board; this page only gathers them.</Lead>
      <Toolbar>
        <Select label="Filter by project" value={slug} onChange={setSlug} options={[['', 'all projects'], ...slugs.map((s) => [s, s] as [string, string])]} />
        <Select
          label="Filter by status"
          value={status}
          onChange={setStatus}
          options={[
            ['', 'any status'],
            ['eligible', 'eligible'],
            ['needs votes', 'needs votes'],
          ]}
        />
        <Select
          label="Sort"
          value={sort}
          onChange={setSort}
          options={[
            ['queue', 'queue order (score, then date)'],
            ['quorum', 'closest to quorum'],
            ['expiry', 'expiring first'],
            ['newest', 'newest first'],
          ]}
        />
        <Count>{num(data.suggestions.length) + ' suggestions \u00b7 ' + num(data.amendments.length) + ' amendments'}</Count>
      </Toolbar>
      <Tabs.Root defaultValue="suggestions">
        <Tabs.List aria-label="Vote kinds" className="mb-3 inline-flex rounded-card border border-line bg-panel-2 p-0.5">
          <Tabs.Trigger value="suggestions" className={TAB}>
            Suggestions in vote <Badge n={list.length} />
          </Tabs.Trigger>
          <Tabs.Trigger value="amendments" className={TAB}>
            Amendments open <Badge n={ams.length} />
          </Tabs.Trigger>
        </Tabs.List>
        <Tabs.Content value="suggestions" className="outline-none">
          {list.length ? (
            <Stack>
              {list.map((s) => (
                <SuggestionCard key={s.slug + s.id} s={s} quorum={s.quorum || 0} />
              ))}
            </Stack>
          ) : (
            <EmptyState>No suggestion matches.</EmptyState>
          )}
        </Tabs.Content>
        <Tabs.Content value="amendments" className="outline-none">
          {ams.length ? (
            <Stack>
              {ams.map((a) => (
                <AmendmentCard key={a.slug + a.id} a={a} />
              ))}
            </Stack>
          ) : (
            <EmptyState>No constitutional amendment is open in any project.</EmptyState>
          )}
        </Tabs.Content>
      </Tabs.Root>
    </>
  );
}
