import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import { api, hubApi, PLACE, type Org, type ProjectInfo } from './api';
import { Footer, Header, NoticesStrip, ReadOnlyNotice, useHeaderFacts } from './components/header';
import { cn } from './lib/cn';
import { reducedMotion } from './lib/format';
import { BYS, RANGES, useRoute } from './lib/router';
import { ActivityView } from './views/activity';
import { AgentsView } from './views/agents';
import { EvolutionView } from './views/evolution';
import { MissionControl } from './views/mission-control';
import { OverviewView } from './views/overview';
import { ProposalsView } from './views/proposals';
import { AmendmentsView, SuggestionsView } from './views/suggestions';
import { PostView, ThreadsView, ThreadView } from './views/threads';
import type { ChartPrefs, ViewProps } from './views/types';
import { VotesView } from './views/votes';

const POLL_MS = 15000;

function pickView(mode: string, view: string): string {
  if (mode === 'org') return ['votes', 'proposals', 'agents', 'activity'].includes(view) ? view : 'org';
  if (view === 'proposals' && mode === 'commons') return 'overview';
  return ['threads', 'thread', 'post', 'proposals', 'suggestions', 'amendments', 'evolution'].includes(view) ? view : 'overview';
}

function View({ name, props }: { name: string; props: ViewProps }) {
  switch (name) {
    case 'org':
      return <MissionControl {...props} />;
    case 'votes':
      return <VotesView {...props} />;
    case 'proposals':
      return <ProposalsView {...props} />;
    case 'agents':
      return <AgentsView {...props} />;
    case 'activity':
      return <ActivityView {...props} />;
    case 'threads':
      return <ThreadsView {...props} />;
    case 'thread':
      return <ThreadView {...props} />;
    case 'post':
      return <PostView {...props} />;
    case 'suggestions':
      return <SuggestionsView {...props} />;
    case 'amendments':
      return <AmendmentsView {...props} />;
    case 'evolution':
      return <EvolutionView {...props} />;
    default:
      return <OverviewView {...props} />;
  }
}

export function App() {
  const route = useRoute();
  const [revision, setRevision] = useState(0);
  const [quiet, setQuiet] = useState(false);
  const [chart, setChart] = useState<ChartPrefs>({ range: '7d', by: PLACE.mode === 'org' ? 'project' : 'total' });
  const stamp = useRef('');
  const main = useRef<HTMLElement>(null);
  const facts = useHeaderFacts(revision);
  const view = pickView(PLACE.mode, route.view);

  // Optional chart state after '?', so a view with a range can be linked:
  // #/activity?range=24h&by=kind
  useEffect(() => {
    const range = route.params.get('range');
    const by = route.params.get('by');
    setChart((c) => {
      const next = { ...c };
      if (range && RANGES.some(([v]) => v === range)) next.range = range;
      if (by && BYS.includes(by)) next.by = by;
      return next.range === c.range && next.by === c.by ? c : next;
    });
  }, [route.hash, route.params]);

  // Navigation is a fresh draw: focus main so screen readers follow.
  useEffect(() => {
    setQuiet(false);
    const el = main.current;
    if (el && document.activeElement !== document.body) el.focus({ preventScroll: true });
  }, [route.hash, route.arg]);

  // Keep the stamp the header saw in project mode so the first poll compares
  // against something real.
  useEffect(() => {
    if (PLACE.mode !== 'org' && facts.stamp) stamp.current = facts.stamp;
  }, [facts.stamp]);

  const onStamp = useCallback((s: string) => {
    if (s) stamp.current = s;
  }, []);

  // Poll the snapshot stamp; redraw only when the projection changed, so
  // filters, scroll position, and focus survive quiet periods.
  useEffect(() => {
    const id = setInterval(async () => {
      if (document.hidden) return;
      try {
        if (PLACE.mode === 'org') {
          const o = await hubApi<Org>('org');
          if (!stamp.current) stamp.current = o.stamp;
          else if (o.stamp !== stamp.current) {
            stamp.current = o.stamp;
            setQuiet(true);
            setRevision((r) => r + 1);
          }
          return;
        }
        const p = await api<ProjectInfo>('project');
        if (p.built_at !== stamp.current) {
          stamp.current = p.built_at;
          setQuiet(true);
          setRevision((r) => r + 1);
        }
      } catch {
        setQuiet(true);
        setRevision((r) => r + 1);
      }
    }, POLL_MS);
    return () => clearInterval(id);
  }, []);

  const refresh = () => {
    (PLACE.mode === 'org' ? Promise.resolve() : api('refresh').then(() => undefined, () => undefined)).then(() => {
      setQuiet(false);
      setRevision((r) => r + 1);
    });
  };

  const animate = !quiet && !reducedMotion();
  const props: ViewProps = { revision, animate, chart, onChartPrefs: (p) => setChart((c) => ({ ...c, ...p })), onStamp, arg: route.arg };
  return (
    <>
      <a href="#main" className="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:rounded-chip focus:bg-accent focus:px-3 focus:py-1.5 focus:text-white">
        Skip to content
      </a>
      <Header activeHash={route.hash} project={facts.project} projectTitle={facts.projectTitle} fresh={facts.fresh} />
      <ReadOnlyNotice />
      <NoticesStrip revision={revision} />
      <main id="main" ref={main} tabIndex={-1} className={cn('mx-auto min-h-[60vh] max-w-[1480px] px-4 pt-4 outline-none', animate && 'view-in')} key={route.hash}>
        <View name={view} props={props} />
      </main>
      <Footer onRefresh={refresh} />
    </>
  );
}
