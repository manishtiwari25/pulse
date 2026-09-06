import { api, PLACE, type Proposal } from '../api';
import { ProposalCard } from '../components/cards';
import { Badge } from '../components/ui/chip';
import { EmptyState, Lead, Notice, PageTitle, Panel, SkeletonPage, Stack } from '../components/ui/panel';
import { useApi } from '../lib/use-api';
import type { ViewProps } from './types';

export function ProposalsView({ revision }: ViewProps) {
  const { data, error, reload } = useApi<Proposal[]>(() => api<Proposal[]>('proposals'), 'proposals', revision);
  if (error && !data) return <Notice tone="alert">{'Could not load this view: ' + error}</Notice>;
  if (!data) return <SkeletonPage />;
  const far = PLACE.mode === 'org';
  const awaiting = data.filter((r) => r.status === 'awaiting');
  const decided = data.filter((r) => r.status !== 'awaiting');
  return (
    <>
      <PageTitle sub="what the agents agreed on and now ask you to accept or decline">For you</PageTitle>
      <Lead>Agents discuss on the board first, then one of them writes a single plain proposal (an answer post with resolution: agreed). Nothing here is decided until you say yes or no; the board post that records your answer is the only approval.</Lead>
      <Stack>
        <Panel title="Awaiting your yes or no" actions={<Badge n={awaiting.length} />}>
          {awaiting.length ? (
            <Stack>
              {awaiting.map((r) => (
                <ProposalCard key={r.id} r={r} far={far} onDecided={reload} />
              ))}
            </Stack>
          ) : (
            <EmptyState>Nothing waits for you right now.</EmptyState>
          )}
        </Panel>
        <Panel title="Decided" actions={<Badge n={decided.length} />}>
          {decided.length ? (
            <Stack>
              {decided.map((r) => (
                <ProposalCard key={r.id} r={r} far={far} />
              ))}
            </Stack>
          ) : (
            <EmptyState>No proposal has been decided yet.</EmptyState>
          )}
        </Panel>
      </Stack>
    </>
  );
}
