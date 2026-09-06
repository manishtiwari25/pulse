import { cn } from '../../lib/cn';
import { num, type Tone } from '../../lib/format';

const FILL: Record<Tone, string> = { '': 'bg-accent', ok: 'bg-ok', warn: 'bg-warn', bad: 'bg-bad', info: 'bg-info', accent: 'bg-accent' };

export function Bar({ value, max, tone = '', title }: { value: number; max: number; tone?: Tone; title?: string }) {
  const pct = Math.max(0, Math.min(100, Math.round((100 * value) / Math.max(1, max))));
  const label = title || value + ' of ' + max;
  return (
    <div role="img" aria-label={label} title={label} className="h-1.5 w-[110px] flex-none overflow-hidden rounded-full bg-chip">
      <span className={cn('block h-full rounded-full transition-[width] duration-500', FILL[tone])} style={{ width: pct + '%' }} />
    </div>
  );
}

export function SplitBar({ up, down }: { up: number; down: number }) {
  const total = Math.max(1, up + down);
  const label = up + ' up, ' + down + ' down';
  return (
    <div role="img" aria-label={label} title={label} className="flex h-1.5 w-[110px] flex-none overflow-hidden rounded-full bg-chip">
      <span className="block h-full bg-ok" style={{ width: Math.round((100 * up) / total) + '%' }} />
      <span className="block h-full bg-bad" style={{ width: Math.round((100 * down) / total) + '%' }} />
    </div>
  );
}

export function Gauge({ pct, markPct, markLabel }: { pct: number; markPct: number; markLabel: string }) {
  return (
    <div role="img" aria-label={pct + '% up; ' + markLabel + ' at ' + markPct + '%'} className="relative h-2.5 w-[180px] flex-none rounded-full bg-chip">
      <span className={cn('absolute inset-y-0 left-0 rounded-full', pct >= markPct ? 'bg-ok' : 'bg-warn')} style={{ width: Math.max(0, Math.min(100, pct)) + '%' }} />
      <span className="absolute -inset-y-1 w-0.5 bg-fg" style={{ left: markPct + '%' }} />
      <span className="absolute top-3 -translate-x-1/2 text-[10.5px] text-muted" style={{ left: markPct + '%' }}>
        {markLabel}
      </span>
    </div>
  );
}

export function StackedBar({ parts }: { parts: { label: string; n: number; color: string }[] }) {
  const total = parts.reduce((a, p) => a + p.n, 0) || 1;
  return (
    <div role="img" aria-label={parts.map((p) => p.label + ' ' + p.n).join(', ')} className="flex h-3 w-full overflow-hidden rounded-full bg-chip">
      {parts.map((p) => (
        <span key={p.label} title={p.label + ': ' + p.n} className="block h-full" style={{ width: (100 * p.n) / total + '%', background: p.color }} />
      ))}
    </div>
  );
}

export function Legend({ parts }: { parts: { label: string; n: number; color: string }[] }) {
  return (
    <div className="mt-2 flex flex-wrap gap-x-3 gap-y-1 text-[12px]">
      {parts.map((p) => (
        <span key={p.label} className="inline-flex items-center gap-1.5">
          <span aria-hidden="true" className="inline-block h-2.5 w-2.5 rounded-sm" style={{ background: p.color }} />
          {p.label} <span className="tnum text-dim">{num(p.n)}</span>
        </span>
      ))}
    </div>
  );
}

export function Sparkline({ values, color = 'var(--accent)', w = 96, h = 26 }: { values: number[]; color?: string; w?: number; h?: number }) {
  if (!values.length) return null;
  const max = Math.max(1, ...values);
  const n = values.length;
  const pts = values.map((v, i) => [n > 1 ? ((w - 2) * i) / (n - 1) + 1 : w / 2, h - 2 - ((h - 4) * v) / max] as [number, number]);
  const d = pts.map((p, i) => (i ? 'L' : 'M') + p[0].toFixed(1) + ',' + p[1].toFixed(1)).join(' ');
  const lastPt = pts[pts.length - 1]!;
  const firstPt = pts[0]!;
  const area = d + ' L' + lastPt[0].toFixed(1) + ',' + (h - 1) + ' L' + firstPt[0].toFixed(1) + ',' + (h - 1) + ' Z';
  return (
    <svg viewBox={'0 0 ' + w + ' ' + h} width={w} height={h} role="img" aria-label={'last ' + n + ' days: ' + values.join(', ')} className="block overflow-visible">
      <path d={area} style={{ fill: color, opacity: 0.18 }} />
      <path d={d} style={{ fill: 'none', stroke: color, strokeWidth: 1.6, strokeLinejoin: 'round' }} />
      <circle cx={lastPt[0]} cy={lastPt[1]} r={2.2} style={{ fill: color }} />
    </svg>
  );
}
