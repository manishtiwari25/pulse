import type { ComponentChildren } from 'preact';
import { cn } from '../../lib/cn';
import { kindClass, statusTone, type Tone } from '../../lib/format';

const TONES: Record<Tone, string> = {
  '': 'bg-chip text-chip-fg',
  ok: 'bg-[color-mix(in_srgb,var(--ok)_16%,transparent)] text-ok',
  warn: 'bg-[color-mix(in_srgb,var(--warn)_18%,transparent)] text-warn',
  bad: 'bg-[color-mix(in_srgb,var(--bad)_16%,transparent)] text-bad',
  info: 'bg-[color-mix(in_srgb,var(--info)_16%,transparent)] text-info',
  accent: 'bg-[color-mix(in_srgb,var(--accent)_16%,transparent)] text-accent',
};

export function Chip({ tone = '', dotted, here, className, children, title }: { tone?: Tone; dotted?: boolean; here?: boolean; className?: string; children: ComponentChildren; title?: string }) {
  return (
    <span title={title} className={cn('inline-flex max-w-full items-center gap-1.5 rounded-chip px-2 py-0.5 text-[12px] leading-5 font-medium whitespace-nowrap', TONES[tone], here && 'ring-1 ring-current/40', className)}>
      {dotted ? <span aria-hidden="true" className="h-1.5 w-1.5 flex-none rounded-full bg-current" /> : null}
      <span className="truncate">{children}</span>
    </span>
  );
}

export function ChipLink({ href, tone = '', dotted, here, children }: { href: string; tone?: Tone; dotted?: boolean; here?: boolean; children: ComponentChildren }) {
  return (
    <a href={href} className={cn('inline-flex max-w-full items-center gap-1.5 rounded-chip px-2 py-0.5 text-[12px] leading-5 font-medium whitespace-nowrap hover:no-underline hover:brightness-110', TONES[tone], here && 'ring-1 ring-current/40')}>
      {dotted ? <span aria-hidden="true" className="h-1.5 w-1.5 flex-none rounded-full bg-current" /> : null}
      <span className="truncate">{children}</span>
    </a>
  );
}

export function StatusChip({ status }: { status?: string | null }) {
  return (
    <Chip tone={statusTone(status)} dotted>
      {status || ''}
    </Chip>
  );
}

export function VoteChip({ vote, classification }: { vote?: string; classification?: string }) {
  const uncounted = !!classification && classification !== 'counted';
  const label = (vote === 'down' ? '\u25bc down' : '\u25b2 up') + (uncounted ? ' \u00b7 ' + classification + ' (uncounted)' : '');
  return <Chip tone={uncounted ? 'warn' : vote === 'down' ? 'bad' : 'ok'}>{label}</Chip>;
}

export function ScoreBadge({ score }: { score: number }) {
  return (
    <span title="score = up \u2212 down" className={cn('tnum inline-flex min-w-[2.6em] justify-center rounded-chip border px-1.5 py-0.5 text-[13px] font-semibold', score > 0 ? 'border-ok/40 text-ok' : score < 0 ? 'border-bad/40 text-bad' : 'border-line-2 text-muted')}>
      {(score > 0 ? '+' : '') + score}
    </span>
  );
}

export function KindDot({ kind }: { kind?: string | null }) {
  return <span aria-hidden="true" className={cn('kind-dot', kindClass(kind))} />;
}

export function KindLabel({ kind }: { kind?: string | null }) {
  return (
    <span className="inline-flex items-center gap-1.5 text-[12.5px] whitespace-nowrap">
      <KindDot kind={kind} />
      {kind || '(none)'}
    </span>
  );
}

export function Provenance({ ok, missing }: { ok?: number | boolean; missing?: string[] }) {
  if (ok) return null;
  return <Chip tone="bad">{'missing provenance: ' + (missing || []).join(', ')}</Chip>;
}

export function Badge({ n }: { n: number | string }) {
  return <span className="tnum inline-flex min-w-[1.6em] justify-center rounded-full bg-chip px-1.5 py-0.5 text-[11.5px] font-semibold text-chip-fg">{n}</span>;
}
