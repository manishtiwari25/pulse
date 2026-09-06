import type { ComponentChildren } from 'preact';
import { cn } from '../../lib/cn';

export function Panel({ title, actions, children, className, tight }: { title?: ComponentChildren; actions?: ComponentChildren; children: ComponentChildren; className?: string; tight?: boolean }) {
  return (
    <section className={cn('panel min-w-0 overflow-hidden', tight ? 'p-3' : 'p-4', className)}>
      {title ? (
        <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
          <h2 className="text-[13px] font-semibold tracking-wide text-muted uppercase">{title}</h2>
          {actions ? <div className="flex flex-wrap items-center gap-2">{actions}</div> : null}
        </div>
      ) : null}
      {children}
    </section>
  );
}

export function EmptyState({ children, hint }: { children: ComponentChildren; hint?: string }) {
  return (
    <div className="rounded-card border border-dashed border-line-2 px-3 py-4 text-[13px] text-muted">
      <div>{children}</div>
      {hint ? <code className="mt-1.5 block text-[12px] text-fg/80">{hint}</code> : null}
    </div>
  );
}

export function Notice({ tone = 'note', children, role }: { tone?: 'note' | 'alert'; children: ComponentChildren; role?: 'note' | 'alert' | 'status' }) {
  return (
    <div role={role || (tone === 'alert' ? 'alert' : 'note')} className={cn('flex items-start gap-2.5 rounded-card border px-3 py-2 text-[13px]', tone === 'alert' ? 'border-bad/40 bg-[color-mix(in_srgb,var(--bad)_8%,transparent)]' : 'border-line bg-panel-2 text-muted')}>
      <span aria-hidden="true" className={cn('mt-0.5 inline-flex h-4 w-4 flex-none items-center justify-center rounded-full text-[11px] font-bold', tone === 'alert' ? 'bg-bad text-white' : 'bg-chip text-chip-fg')}>
        !
      </span>
      <span className="min-w-0 [overflow-wrap:anywhere]">{children}</span>
    </div>
  );
}

export function PageTitle({ children, sub, crumb }: { children: ComponentChildren; sub?: ComponentChildren; crumb?: ComponentChildren }) {
  return (
    <h1 className="mb-3 flex flex-wrap items-baseline gap-x-3 gap-y-1 text-[22px] font-semibold tracking-tight">
      {crumb ? <span className="text-[14px] font-normal text-muted after:ml-2 after:content-['/']">{crumb}</span> : null}
      <span className="min-w-0 [overflow-wrap:anywhere]">{children}</span>
      {sub ? <span className="text-[13px] font-normal text-muted">{sub}</span> : null}
    </h1>
  );
}

export function Lead({ children }: { children: ComponentChildren }) {
  return <p className="mb-3 max-w-[900px] text-[13px] leading-relaxed text-muted">{children}</p>;
}

const GAPS: Record<number, string> = { 1: 'gap-1', 2: 'gap-2', 3: 'gap-3', 4: 'gap-4' };

export function Stack({ children, gap = 3, className }: { children: ComponentChildren; gap?: 1 | 2 | 3 | 4; className?: string }) {
  return <div className={cn('flex min-w-0 flex-col', GAPS[gap], className)}>{children}</div>;
}

export function Row({ children, className }: { children: ComponentChildren; className?: string }) {
  return <div className={cn('flex min-w-0 flex-wrap items-center gap-2', className)}>{children}</div>;
}

export function Toolbar({ children }: { children: ComponentChildren }) {
  return <div className="mb-3 flex flex-wrap items-center gap-2">{children}</div>;
}

export function Skeleton({ className }: { className?: string }) {
  return <div aria-hidden="true" className={cn('skel', className)} />;
}

export function SkeletonPage() {
  return (
    <div aria-hidden="true" className="grid gap-3">
      <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
        <Skeleton className="h-[92px]" />
        <Skeleton className="h-[92px]" />
        <Skeleton className="h-[92px]" />
        <Skeleton className="h-[92px]" />
      </div>
      <Skeleton className="h-[220px]" />
    </div>
  );
}

export function Select({ label, value, onChange, options }: { label: string; value: string; onChange: (v: string) => void; options: [string, string][] }) {
  return (
    <select aria-label={label} value={value} onChange={(e) => onChange((e.currentTarget as HTMLSelectElement).value)} className="h-8 rounded-chip border border-line bg-panel px-2 text-[13px] text-fg hover:border-line-2 focus-visible:border-accent">
      {options.map(([v, l]) => (
        <option key={v} value={v}>
          {l}
        </option>
      ))}
    </select>
  );
}

export function SearchInput({ label, value, onInput, placeholder, type = 'search' }: { label: string; value: string; onInput: (v: string) => void; placeholder?: string; type?: string }) {
  return <input type={type} aria-label={label} value={value} placeholder={placeholder} onInput={(e) => onInput((e.currentTarget as HTMLInputElement).value)} className="h-8 min-w-[220px] rounded-chip border border-line bg-panel px-2.5 text-[13px] text-fg placeholder:text-dim hover:border-line-2 focus-visible:border-accent" />;
}

export function Count({ children }: { children: ComponentChildren }) {
  return <span className="tnum ml-auto text-[12.5px] text-muted">{children}</span>;
}
