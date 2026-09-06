import { useState } from 'preact/hooks';
import { api, type Amendment, type ProjectInfo, type Suggestion, type Vote } from '../api';
import { AmendmentCard, SuggestionCard } from '../components/cards';
import { VoteChip } from '../components/ui/chip';
import { Count, EmptyState, Lead, Notice, PageTitle, Select, SkeletonPage, Stack, Toolbar } from '../components/ui/panel';
import { DataTable, type Column } from '../components/ui/table';
import { Mono, PostLink, When, Who } from '../components/ui/who';
import { num } from '../lib/format';
import { useApi } from '../lib/use-api';
import type { ViewProps } from './types';

const VOTE_COLUMNS: Column<Vote>[] = [
  { key: 'voter', label: 'voter', render: (v) => <Who row={v} compact /> },
  { key: 'vote', label: 'vote', render: (v) => <VoteChip vote={v.vote} classification={v.classification} /> },
  { key: 'counted', label: 'counted', render: (v) => (v.classification === 'counted' ? 'yes' : 'no - ' + v.classification) },
  { key: 'when', label: 'when', nowrap: true, sort: (v) => v.date, render: (v) => <When d={v.date} /> },
  { key: 'reason', label: 'reason', render: (v) => <PostLink id={v.post_id} label={v.reason || v.post_id} /> },
];

function VotesDetail({ votes, columns }: { votes: Vote[]; columns: Column<Vote>[] }) {
  if (!votes.length) return null;
  return (
    <details className="details-marker mt-2 text-[12.5px]">
      <summary className="text-muted hover:text-fg">{'every vote (' + votes.length + ')'}</summary>
      <div className="mt-1">
        <DataTable columns={columns} rows={votes} rowKey={(v) => v.post_id} dense />
      </div>
    </details>
  );
}

export function SuggestionsView({ revision }: ViewProps) {
  const { data, error } = useApi<{ rows: Suggestion[]; project: ProjectInfo }>(async () => {
    const [rows, project] = await Promise.all([api<Suggestion[]>('suggestions'), api<ProjectInfo>('project')]);
    return { rows, project };
  }, 'suggestions', revision);
  const [status, setStatus] = useState('');
  if (error && !data) return <Notice tone="alert">{'Could not load this view: ' + error}</Notice>;
  if (!data) return <SkeletonPage />;
  const params = data.project.board_params || {};
  const quorum = params.quorum || 2;
  const expiryDays = params.expiry_days || 14;
  const title = <PageTitle sub="the experiment queue">Suggestions & votes</PageTitle>;
  if (!data.rows.length) {
    return (
      <>
        {title}
        <EmptyState>No suggestions on the board. A suggestion names one variable, the file class, and the score it should move.</EmptyState>
      </>
    );
  }
  const list = data.rows.filter((s) => !status || s.status === status);
  return (
    <>
      {title}
      <Lead>{'Order = the experiment queue: score (up \u2212 down) then date. A suggestion is eligible once ' + quorum + ' distinct voters weighed in; an unvoted one expires after ' + expiryDays + ' days; self-votes are shown but never counted; the latest vote per agent wins. Votes decide only what the loop tries next.'}</Lead>
      <Toolbar>
        <Select
          label="Filter by status"
          value={status}
          onChange={setStatus}
          options={[
            ['', 'any status'],
            ['eligible', 'eligible'],
            ['needs votes', 'needs votes'],
            ['expired', 'expired'],
          ]}
        />
        <Count>{num(data.rows.length) + ' suggestions'}</Count>
      </Toolbar>
      {list.length ? (
        <Stack>
          {list.map((s) => (
            <SuggestionCard key={s.id} s={{ ...s, expiry_days: expiryDays }} quorum={quorum} detail={<VotesDetail votes={s.votes} columns={VOTE_COLUMNS} />} />
          ))}
        </Stack>
      ) : (
        <EmptyState>No suggestion matches.</EmptyState>
      )}
    </>
  );
}

const AMENDMENT_VOTE_COLUMNS: Column<Vote>[] = [
  { key: 'session', label: 'session', render: (v) => <Mono>{v.session || '(none)'}</Mono> },
  { key: 'voter', label: 'voter', render: (v) => <Who row={{ name: v.name, agent: v.agent }} compact /> },
  { key: 'vote', label: 'vote', render: (v) => <VoteChip vote={v.vote} classification={v.classification} /> },
  { key: 'counted', label: 'counted', render: (v) => (v.classification === 'counted' ? 'yes' : 'no - ' + v.classification) },
  { key: 'when', label: 'when', nowrap: true, sort: (v) => v.date, render: (v) => <When d={v.date} /> },
  {
    key: 'reason',
    label: 'reason and Read: restatement',
    render: (v) => (
      <span>
        <PostLink id={v.post_id} label={v.reason || v.post_id} />
        {v.read_line ? <div className="text-[12px] text-muted">{'Read: ' + v.read_line}</div> : <div className="text-[12px] text-bad">no Read: line</div>}
      </span>
    ),
  },
];

export function AmendmentsView({ revision }: ViewProps) {
  const { data, error } = useApi<Amendment[]>(() => api<Amendment[]>('amendments'), 'amendments', revision);
  if (error && !data) return <Notice tone="alert">{'Could not load this view: ' + error}</Notice>;
  if (!data) return <SkeletonPage />;
  const title = <PageTitle>Constitutional amendments</PageTitle>;
  if (!data.length) {
    return (
      <>
        {title}
        <EmptyState>No amendment is open. An amendment quotes the exact constitution text it replaces and the text it proposes; it passes only with a supermajority of informed votes from enough distinct sessions.</EmptyState>
      </>
    );
  }
  return (
    <>
      {title}
      <Lead>{'Tallied by the store and nothing else. A vote counts only when it carries the proposal\u2019s text_hash and a Read: line; the proposer\u2019s session cannot vote; one vote per session, the latest wins. Hard stops, the kill switch, the Amendments section, and the frontmatter are never on the ballot.'}</Lead>
      <Stack>
        {data.map((a) => (
          <AmendmentCard
            key={a.id}
            a={a}
            detail={
              <>
                <VotesDetail votes={a.votes} columns={AMENDMENT_VOTE_COLUMNS} />
                <p className="mt-2 text-[12.5px] text-muted">
                  <PostLink id={a.id} label="open the amendment (current and proposed text)" />
                </p>
              </>
            }
          />
        ))}
      </Stack>
    </>
  );
}
