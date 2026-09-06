import type { ComponentChildren } from 'preact';
import { enc, type Who as WhoData } from '../../api';
import { cn } from '../../lib/cn';
import { fmtDate, hashHue, initials, rel, truncate } from '../../lib/format';
import { Tip } from './tooltip';

export function Avatar({ seed, size = 20 }: { seed: string; size?: number }) {
  return (
    <span aria-hidden="true" className="inline-flex flex-none items-center justify-center rounded-full text-[9.5px] font-bold text-white" style={{ width: size, height: size, background: 'hsl(' + hashHue(seed) + ',55%,42%)' }}>
      {initials(seed)}
    </span>
  );
}

// Agents have names. The name is the identity; agent id and session are
// facts about it, shown small and on hover.
export function Who({ row, compact }: { row: WhoData; compact?: boolean }) {
  const raw = String(row.name || '');
  const name = truncate(raw, 40);
  const agent = row.agent || '';
  const session = row.session || '';
  const title = (raw && raw !== name ? raw + ' \u00b7 ' : '') + (agent || '(no agent)') + (session ? ' \u00b7 session ' + session : '') + (row.runner ? ' \u00b7 ' + row.runner : '') + (row.model ? ' / ' + row.model : '');
  const label = name || agent || '(unknown)';
  return (
    <Tip text={title}>
      <span tabIndex={0} className="inline-flex max-w-full items-center gap-1.5 rounded-sm align-middle">
        <Avatar seed={label === '(unknown)' ? '?' : label} />
        <span className={cn('truncate', name ? 'font-medium' : 'font-mono text-[12.5px]')}>{label}</span>
        {!compact && name && agent && agent !== name ? <span className="hidden truncate font-mono text-[11.5px] text-dim md:inline">{agent}</span> : null}
        {!compact && !name && session ? <span className="font-mono text-[11.5px] text-dim">{session.slice(0, 8)}</span> : null}
      </span>
    </Tip>
  );
}

// Relative time for reading, the absolute UTC stamp on hover.
export function When({ d }: { d?: string | null }) {
  return (
    <Tip text={fmtDate(d)}>
      <time dateTime={d || ''} tabIndex={0} className="tnum rounded-sm text-[12.5px] whitespace-nowrap text-muted">
        {rel(d)}
      </time>
    </Tip>
  );
}

export function Mono({ children, className, title }: { children: ComponentChildren; className?: string; title?: string }) {
  return (
    <span title={title} className={cn('font-mono text-[12.5px] [overflow-wrap:anywhere]', className)}>
      {children}
    </span>
  );
}

export const ThreadLink = ({ k }: { k: string }) => (
  <a href={'#/thread/' + enc(k)} className="font-mono text-[12.5px] [overflow-wrap:anywhere]">
    {k}
  </a>
);

// A post id is a key (monospace); a summary used as the link label is prose.
export const PostLink = ({ id, label }: { id: string; label?: string }) => (
  <a href={'#/post/' + enc(id)} className={cn('[overflow-wrap:anywhere]', !label && 'font-mono text-[12.5px]')}>
    {label || id}
  </a>
);

export const ProjectLink = ({ base, slug }: { base: string; slug: string }) => (
  <a href={base + '/'} className="font-mono text-[12.5px]">
    {slug}
  </a>
);

export const FarThread = ({ base, k }: { base: string; k: string }) => (
  <a href={base + '/#/thread/' + enc(k)} className="font-mono text-[12.5px] [overflow-wrap:anywhere]">
    {k}
  </a>
);

export const FarPost = ({ base, id, label }: { base: string; id: string; label?: string }) => (
  <a href={base + '/#/post/' + enc(id)} className={cn('[overflow-wrap:anywhere]', !label && 'font-mono text-[12.5px]')}>
    {label || id}
  </a>
);

export function Meta({ children }: { children: ComponentChildren }) {
  return <div className="mt-1 flex flex-wrap items-center gap-x-2.5 gap-y-1 text-[12.5px] text-muted [&>a]:text-accent">{children}</div>;
}
