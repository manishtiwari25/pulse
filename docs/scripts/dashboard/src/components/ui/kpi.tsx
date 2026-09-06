import type { ComponentChildren } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { cn } from '../../lib/cn';
import { num, reducedMotion, type Tone } from '../../lib/format';

const TONE_TEXT: Record<Tone, string> = { '': 'text-fg', ok: 'text-ok', warn: 'text-warn', bad: 'text-bad', info: 'text-info', accent: 'text-accent' };

// A big tabular number that counts up on navigation (never on a quiet redraw,
// never under prefers-reduced-motion).
export function AnimatedNumber({ value, animate }: { value: number; animate: boolean }) {
  const ref = useRef<HTMLSpanElement>(null);
  const last = useRef<number | null>(null);
  useEffect(() => {
    const node = ref.current;
    if (!node) return;
    const finalText = num(value);
    if (!animate || reducedMotion() || value < 2 || last.current === value) {
      node.textContent = finalText;
      last.current = value;
      return;
    }
    last.current = value;
    const start = performance.now();
    const dur = 600;
    let raf = 0;
    const step = () => {
      const p = Math.max(0, Math.min(1, (performance.now() - start) / dur));
      node.textContent = num(Math.round(value * (1 - Math.pow(1 - p, 3))));
      if (p < 1) raf = requestAnimationFrame(step);
      else node.textContent = finalText;
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  }, [value, animate]);
  return <span ref={ref}>{num(value)}</span>;
}

export interface KpiProps {
  n: number | string;
  label: string;
  href?: string | null;
  sub?: string;
  tone?: Tone;
  spark?: ComponentChildren;
  animate?: boolean;
}

export function Kpi({ n, label, href, sub, tone = '', spark, animate = true }: KpiProps) {
  const zero = n === 0 || n === '0';
  const inner = (
    <>
      <div className="text-[11.5px] font-medium tracking-wide text-muted uppercase">{label}</div>
      <div className={cn('tnum mt-0.5 text-[26px] leading-none font-semibold tracking-tight', TONE_TEXT[tone], zero && 'text-dim')}>{typeof n === 'number' ? <AnimatedNumber value={n} animate={animate} /> : n}</div>
      <div className="mt-1.5 min-h-[16px] truncate text-[12px] text-muted" title={sub || ''}>
        {sub || ''}
      </div>
      {spark ? <span className="absolute right-3 bottom-3 opacity-90">{spark}</span> : null}
    </>
  );
  const cls = cn('panel relative block min-w-0 p-3.5 transition-[transform,border-color] duration-200', href && 'hover:-translate-y-px hover:border-line-2 hover:no-underline', spark && 'pr-[112px]');
  return href ? (
    <a href={href} className={cn(cls, 'text-fg')} data-kpi={label}>
      {inner}
    </a>
  ) : (
    <div className={cls} data-kpi={label}>
      {inner}
    </div>
  );
}

export function KpiGrid({ children }: { children: ComponentChildren }) {
  return <div className="mb-3.5 grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-4">{children}</div>;
}

export function Stat({ n, label, href }: { n: number; label: string; href?: string }) {
  const inner = (
    <>
      <b className={cn('tnum text-[15px] font-semibold', n ? 'text-fg' : 'text-dim')}>{num(n)}</b>
      <span className="text-[11.5px] text-muted">{label}</span>
    </>
  );
  const cls = 'inline-flex items-baseline gap-1.5 rounded-chip px-1.5 py-0.5';
  return href ? (
    <a href={href} className={cn(cls, 'hover:bg-panel-hover hover:no-underline')}>
      {inner}
    </a>
  ) : (
    <span className={cls}>{inner}</span>
  );
}
