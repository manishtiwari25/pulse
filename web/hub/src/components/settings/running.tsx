"use client";

import { Kv } from "@/components/pulse/panel";
import { Mono, ToneBadge } from "@/components/pulse/who";
import type { RunningPayload, RunningState } from "@/lib/api";
import { num, rel, type Tone } from "@/lib/format";
import { useApi } from "@/lib/use-api";

// The "Running" strip at the top of the daemon section (ADR-035): which pulse
// runs where against the brain's HEAD, and what the daemon will do about it.
// Read-only - the daemon rebuilds itself; turning that off is the
// daemon.self_update row underneath.

const STATE: Record<RunningState, { tone: Tone; label: string; text: string }> = {
  current: { tone: "ok", label: "current", text: "The daemon runs the brain's HEAD. Nothing to do." },
  behind: { tone: "info", label: "rebuilding", text: "The brain's HEAD is newer than the daemon's build: it rebuilds and installs itself on its next tick, then restarts once its runs finish. The hub follows within a minute." },
  waiting: { tone: "warn", label: "waiting", text: "The brain checkout is dirty or off its default branch, so the daemon waits (ADR-035 rule 4). Commit or switch back and it rebuilds on the next tick." },
  unstamped: { tone: "info", label: "unstamped", text: "The installed build carries no revision (built in a worktree or from a tarball). The daemon rebuilds once from the brain on the next landing and is stamped from then on." },
  off: { tone: "neutral", label: "off", text: "daemon.self_update is off: the daemon keeps the build it has. cargo install --path crates/pulse --root ~/.pulse --locked installs by hand." },
  "no-daemon": { tone: "warn", label: "no daemon", text: "The daemon has not run yet on this store. Keep PULSE running - the card on the Hub page, or pulse service install - starts it and this console as user services." },
  "no-brain": { tone: "neutral", label: "as built", text: "No registered project is a PULSE checkout, so this build stays as it is: the framework's records came with it and every project works. Only someone developing PULSE itself registers their checkout with pulse init PATH --brain (ADR-035), and then the daemon rebuilds from it." },
  "no-store": { tone: "bad", label: "no store", text: "This hub has no store open." },
};

function short(rev: string): string {
  return rev.length > 12 ? rev.slice(0, 12) : rev;
}

function Build({ b }: { b: RunningPayload["hub"] | null }) {
  if (!b) return <span className="text-muted-foreground">not yet</span>;
  return (
    <span className="inline-flex flex-wrap items-baseline gap-x-2">
      <Mono>{b.version || "dev"}</Mono>
      {b.revision && !(b.version || "").includes(short(b.revision)) ? <Mono className="text-muted-foreground">{short(b.revision)}</Mono> : null}
      {b.pid ? <span className="text-muted-foreground tnum">pid {b.pid}</span> : null}
      {b.at ? <span className="text-muted-foreground">beat {rel(b.at)}</span> : null}
    </span>
  );
}

export function RunningStrip() {
  const q = useApi<RunningPayload>("running", { refresh: 30000 });
  const d = q.data;
  if (!d) {
    return <div className="px-4 py-2 text-xs text-muted-foreground">{q.error ? "Could not read what is running: " + q.error : "reading what is running…"}</div>;
  }
  const st = STATE[d.state] ?? STATE["no-store"];
  const rows: [React.ReactNode, React.ReactNode][] = [
    ["hub", <Build key="h" b={d.hub} />],
    ["daemon", <Build key="d" b={d.daemon} />],
  ];
  const bus = d.daemon?.bus;
  if (bus) {
    rows.push([
      "bus",
      <span key="bus" className="inline-flex flex-wrap items-baseline gap-x-2">
        <span className="tnum">{bus.subscribers + " subscribers"}</span>
        <ToneBadge tone={bus.dropped > 0 ? "warn" : "neutral"}>{num(bus.dropped) + " dropped"}</ToneBadge>
        <span className="tnum text-muted-foreground">{num(bus.deduped) + " deduped"}</span>
        {bus.dropped > 0 ? <span className="text-warn">slow subscribers lost events</span> : null}
      </span>,
    ]);
  }
  if (d.brain) {
    rows.push([
      "brain",
      <span key="b" className="inline-flex flex-wrap items-baseline gap-x-2">
        <span>{d.brain.slug}</span>
        <Mono className="text-muted-foreground">{d.brain.branch}</Mono>
        <Mono>{short(d.brain.head) || "no commits"}</Mono>
        {d.brain.dirty ? <span className="text-warn">dirty</span> : null}
        {d.brain.branch !== d.brain.default_branch ? <span className="text-warn">off {d.brain.default_branch}</span> : null}
      </span>,
    ]);
  }
  if (d.signed === true || d.signed === false) {
    rows.push([
      "signature",
      d.signed ? (
        <span key="s" className="inline-flex flex-wrap items-baseline gap-x-2">
          <span>signed</span>
          <span className="text-muted-foreground">macOS keeps your folder answers across rebuilds</span>
        </span>
      ) : (
        <span key="s" className="inline-flex flex-wrap items-baseline gap-x-2">
          <span className="text-warn">unsigned</span>
          <span className="text-muted-foreground anywhere">
            each rebuild asks macOS for the Desktop/Documents folders again - <Mono>pulse service install</Mono> signs this build as pulse (no password) and every later build keeps the answer
          </span>
        </span>
      ),
    ]);
  }
  if (d.last) {
    rows.push([
      "last rebuild",
      <span key="l" className="inline-flex flex-wrap items-baseline gap-x-2">
        <span>{rel(d.last.at)}</span>
        <Mono className="text-muted-foreground">{short(d.last.from) || "unstamped"}</Mono>
        <span className="text-muted-foreground">→</span>
        <Mono>{short(d.last.to)}</Mono>
        {d.last.inflight ? <span className="text-muted-foreground tnum">restart after {d.last.inflight} in flight</span> : null}
      </span>,
    ]);
  }
  return (
    <div className="grid gap-3 px-4 py-3 text-[13px]" data-testid="running-strip">
      <div className="flex flex-wrap items-center gap-2">
        <ToneBadge tone={st.tone} dot>
          {st.label}
        </ToneBadge>
        <span className="text-muted-foreground anywhere">{st.text}</span>
      </div>
      <Kv rows={rows} className="rounded-lg border bg-muted/30 p-3" />
    </div>
  );
}
