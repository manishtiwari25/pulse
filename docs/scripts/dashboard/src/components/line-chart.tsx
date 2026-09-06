import { useEffect, useRef, useState } from 'preact/hooks';
import type { Bucket, Timeline } from '../api';
import { cn } from '../lib/cn';
import { KINDS, kindVar, niceStep, num, PALETTE, reducedMotion } from '../lib/format';
import { RANGES } from '../lib/router';
import { EmptyState, Panel } from './ui/panel';
import { Seg } from './ui/seg';

export interface Series {
  label: string;
  color: string;
  values: number[];
  total?: boolean;
  area?: boolean;
}

interface ChartProps {
  labels: string[];
  series: Series[];
  fmt: (l: string) => string;
  fmtLong: (l: string) => string;
  aria: string;
  height?: number;
}

// Line chart without a library. Hovering or focusing a column shows every
// series' value; the axis redraws on resize; the legend toggles a series.
export function LineChart({ labels, series: all, fmt, fmtLong, aria, height = 220 }: ChartProps) {
  const box = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(720);
  const [hidden, setHidden] = useState<Set<string>>(new Set());
  const [tip, setTip] = useState<number | null>(null);
  useEffect(() => {
    const el = box.current;
    if (!el || typeof ResizeObserver !== 'function') return;
    const ro = new ResizeObserver(() => {
      const w = Math.floor(el.clientWidth);
      if (w) setWidth(w);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const W = Math.max(320, width);
  const H = height;
  const padL = 36;
  const padR = 14;
  const padT = 14;
  const padB = 28;
  const n = labels.length;
  const series = all.filter((s) => !hidden.has(s.label));
  const max = Math.max(1, ...series.flatMap((s) => s.values));
  const step = niceStep(max / 4);
  const top = step * 4;
  const x = (i: number) => padL + (n > 1 ? ((W - padL - padR) * i) / (n - 1) : (W - padL - padR) / 2);
  const y = (v: number) => padT + (H - padT - padB) * (1 - v / top);
  const every = Math.max(1, Math.ceil(n / Math.max(3, Math.floor((W - padL - padR) / 84))));
  const colW = n > 1 ? (W - padL - padR) / (n - 1) : W - padL - padR;
  const motion = !reducedMotion();
  const toggleSeries = (label: string) =>
    setHidden((h) => {
      const next = new Set(h);
      if (next.has(label)) next.delete(label);
      else next.add(label);
      return next;
    });
  const peakAt = (i: number) => (series.length ? Math.max(...series.map((s) => s.values[i] || 0)) : 0);
  return (
    <div ref={box} className={cn('relative w-full', tip !== null && 'has-tip')}>
      <svg viewBox={'0 0 ' + W + ' ' + H} width={W} height={H} role="img" aria-label={aria} className="block max-w-full">
        {[0, 1, 2, 3, 4].map((k) => {
          const v = step * k;
          return (
            <g key={k}>
              <line x1={padL} x2={W - padR} y1={y(v)} y2={y(v)} className="stroke-grid" strokeWidth={1} />
              <text x={padL - 6} y={y(v) + 3.5} textAnchor="end" className="fill-dim text-[10.5px]">
                {num(v)}
              </text>
            </g>
          );
        })}
        {labels.map((lab, i) =>
          i % every === 0 || i === n - 1 ? (
            <text key={lab} x={x(i)} y={H - padB + 16} textAnchor={i === 0 ? 'start' : i === n - 1 ? 'end' : 'middle'} className="fill-dim text-[10.5px]">
              {fmt(lab)}
            </text>
          ) : null,
        )}
        {series.map((s) => {
          const d = s.values.map((v, i) => (i ? 'L' : 'M') + x(i).toFixed(1) + ',' + y(v).toFixed(1)).join(' ');
          const last = s.values.length - 1;
          return (
            <g key={s.label}>
              {s.area && n > 1 ? <path d={d + ' L' + x(n - 1).toFixed(1) + ',' + y(0) + ' L' + x(0).toFixed(1) + ',' + y(0) + ' Z'} style={{ fill: s.color, opacity: 0.08 }} /> : null}
              <path d={d} pathLength={1} className={cn(s.total && motion && 'draw-in')} style={{ fill: 'none', stroke: s.color, strokeWidth: s.total ? 2.2 : 1.6, strokeLinejoin: 'round', strokeLinecap: 'round' }} />
              <circle cx={x(last)} cy={y(s.values[last] || 0)} r={3} style={{ fill: 'var(--panel)', stroke: s.color, strokeWidth: 1.6 }} />
            </g>
          );
        })}
        {tip !== null ? <line x1={x(tip)} x2={x(tip)} y1={padT} y2={H - padB} className="stroke-line-2" strokeDasharray="3 3" /> : null}
        {labels.map((lab, i) => {
          const rows = series.map((s) => s.label + ' ' + (s.values[i] || 0)).join(', ') || 'nothing';
          return <rect key={lab} x={x(i) - colW / 2} y={padT} width={colW} height={H - padT - padB} tabIndex={0} role="img" aria-label={fmtLong(lab) + ': ' + rows} className="fill-transparent outline-none" onMouseEnter={() => setTip(i)} onFocus={() => setTip(i)} onMouseLeave={() => setTip(null)} onBlur={() => setTip(null)} />;
        })}
      </svg>
      {tip !== null && labels[tip] !== undefined ? (
        <div role="status" aria-live="polite" className="pointer-events-none absolute z-10 min-w-[140px] -translate-x-1/2 -translate-y-full rounded-chip bg-tip-bg px-2.5 py-1.5 text-[12px] text-tip-fg shadow-panel" style={{ left: x(tip) + 'px', top: Math.max(0, y(peakAt(tip)) - 10) + 'px' }}>
          <b>{fmtLong(labels[tip]!)}</b>
          {series.length ? (
            series.map((s) => (
              <div key={s.label} className="flex justify-between gap-3">
                <span className="inline-flex items-center gap-1.5">
                  <span aria-hidden="true" className="inline-block h-2 w-2 rounded-sm" style={{ background: s.color }} />
                  {s.label}
                </span>
                <span className="tnum">{num(s.values[tip] || 0)}</span>
              </div>
            ))
          ) : (
            <div className="text-dim">no series</div>
          )}
        </div>
      ) : null}
      <div className="mt-2 flex flex-wrap gap-1.5">
        {all.map((s) => {
          const off = hidden.has(s.label);
          return (
            <button key={s.label} type="button" aria-pressed={!off} title={(off ? 'show ' : 'hide ') + s.label} onClick={() => toggleSeries(s.label)} className={cn('inline-flex items-center gap-1.5 rounded-chip border border-line px-2 py-0.5 text-[12px] hover:border-line-2', off && 'opacity-45')}>
              <span aria-hidden="true" className="inline-block h-2.5 w-2.5 rounded-sm" style={{ background: s.color }} />
              {s.label} <span className="tnum text-dim">{num(s.values.reduce((a, b) => a + b, 0))}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}

export function buckets(tl: Timeline, range: string): Bucket[] {
  if (range === '24h') return tl.hours || [];
  const days = tl.days || [];
  return range === '7d' ? days.slice(-7) : days;
}

const fmtBucket = (range: string) => (t: string) => (range === '24h' ? t.slice(11) + ':00' : t.slice(5).replace('-', '/'));
const fmtBucketLong = (range: string) => (t: string) => (range === '24h' ? t.replace('T', ' ') + ':00 UTC' : t);

export function chartFor(tl: Timeline, range: string, by: string) {
  const bs = buckets(tl, range);
  const labels = bs.map((b) => b.t);
  let series: Series[] = [];
  if (by === 'project' && tl.series) {
    series = tl.series.map((s, i) => ({ label: s.slug, color: PALETTE[i % PALETTE.length]!, values: buckets(s, range).map((b) => b.n) }));
    if (tl.series.length > 1) series.unshift({ label: 'all projects', color: 'var(--fg)', values: bs.map((b) => b.n), total: true, area: true });
  } else if (by === 'kind') {
    const kinds = [...new Set(bs.flatMap((b) => Object.keys(b.kinds || {})))];
    kinds.sort((a, b) => KINDS.indexOf(a) - KINDS.indexOf(b));
    series = kinds.map((k) => ({ label: k, color: kindVar(k), values: bs.map((b) => (b.kinds || {})[k] || 0) }));
  }
  if (!series.length) series = [{ label: 'posts', color: 'var(--accent)', values: bs.map((b) => b.n), total: true, area: true }];
  const total = bs.reduce((a, b) => a + b.n, 0);
  const rangeText = range === '24h' ? '24 hours' : range === '7d' ? '7 days' : '30 days';
  if (!total) return <EmptyState>{'No posts in the last ' + rangeText + '.'}</EmptyState>;
  return <LineChart labels={labels} series={series} fmt={fmtBucket(range)} fmtLong={fmtBucketLong(range)} aria={'Posts per ' + (range === '24h' ? 'hour' : 'day') + ', ' + num(total) + ' in total'} />;
}

export const lastDays = (tl: Timeline | null | undefined, k: number): number[] => (tl?.days || []).slice(-k).map((b) => b.n);

// The activity panel: range and breakdown controls plus the chart. The
// choice lives in the URL (#/activity?range=24h&by=kind) so a view can be
// linked and survives a quiet redraw.
export function ActivityPanel({ tl, title, bys, range, by, onPrefs }: { tl: Timeline; title: string; bys: string[]; range: string; by: string; onPrefs: (p: { range?: string; by?: string }) => void }) {
  const useBy = bys.includes(by) ? by : bys[0]!;
  const note =
    tl.before_window || tl.unparsed ? (
      <p className="mt-2 text-[11.5px] text-muted">
        {(tl.before_window ? num(tl.before_window) + ' older posts are outside the 30-day window' : '') + (tl.before_window && tl.unparsed ? '; ' : '') + (tl.unparsed ? num(tl.unparsed) + ' posts have a date the chart cannot place' : '') + '.'}
      </p>
    ) : null;
  return (
    <Panel
      title={title}
      actions={
        <>
          {bys.length > 1 ? <Seg options={bys.map((b) => [b, 'by ' + b])} value={useBy} onChange={(v) => onPrefs({ by: v })} label="Breakdown" /> : null}
          <Seg options={RANGES} value={range} onChange={(v) => onPrefs({ range: v })} label="Range" />
        </>
      }
    >
      {chartFor(tl, range, useBy)}
      {note}
    </Panel>
  );
}
