// Formatting helpers shared by every view. Pure functions, no DOM.

export const KINDS = ['claim', 'handoff', 'finding', 'question', 'answer', 'suggestion', 'opinion', 'vote', 'amendment', 'result'];

export const kindClass = (k?: string | null): string => 'k-' + (k && KINDS.includes(k) ? k : 'none');
export const kindVar = (k?: string | null): string => 'var(--' + kindClass(k) + ')';

export const PALETTE = ['var(--s1)', 'var(--s2)', 'var(--s3)', 'var(--s4)', 'var(--s5)', 'var(--s6)', 'var(--s7)', 'var(--s8)'];

export const num = (n: unknown): string => Number(n || 0).toLocaleString('en-US');
export const plural = (n: number, word: string): string => num(n) + ' ' + word + (n === 1 ? '' : 's');
export const fmtDate = (d?: string | null): string => (d || '').replace('T', ' ').replace('Z', ' UTC');

export function parseDate(d?: string | null): Date | null {
  if (!d) return null;
  let s = String(d).trim();
  if (/^\d{4}-\d{2}-\d{2}$/.test(s)) s += 'T00:00:00Z';
  if (!/Z$/.test(s)) s += 'Z';
  const t = Date.parse(s);
  return isNaN(t) ? null : new Date(t);
}

export function span(sec: number): string {
  if (sec < 90) return Math.max(1, Math.round(sec)) + 's';
  if (sec < 3600) return Math.round(sec / 60) + 'm';
  if (sec < 86400) return Math.round(sec / 3600) + 'h';
  if (sec < 86400 * 30) return Math.round(sec / 86400) + 'd';
  return Math.round(sec / (86400 * 30)) + 'mo';
}

export function rel(d?: string | null): string {
  const t = parseDate(d);
  if (!t) return d || '';
  const diff = (Date.now() - t.getTime()) / 1000;
  if (diff < -60) return 'in ' + span(-diff);
  if (diff < 45) return 'just now';
  return span(diff) + ' ago';
}

export function untilText(t: Date | null): string {
  if (!t) return '';
  const diff = (t.getTime() - Date.now()) / 1000;
  return diff < 0 ? 'expired ' + span(-diff) + ' ago' : 'in ' + span(diff);
}

export function expiresAt(s: { date: string; expiry_days?: number }): Date | null {
  const d = parseDate(s.date);
  return d && s.expiry_days ? new Date(d.getTime() + s.expiry_days * 86400000) : null;
}

export function reducedMotion(): boolean {
  return !!(window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches);
}

export function hashHue(seed: string): number {
  let h = 0;
  for (const ch of seed) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return h % 360;
}

export function initials(seed: string): string {
  const parts = seed.split(/[\s/_\-.:]+/).filter(Boolean);
  const a = parts[0] || '?';
  const b = parts[1];
  return (parts.length > 1 && b ? a[0]! + b[0]! : a.slice(0, 2)).toUpperCase();
}

export function truncate(s: string, n: number): string {
  return s.length > n ? s.slice(0, n) + '\u2026' : s;
}

export type Tone = '' | 'ok' | 'warn' | 'bad' | 'info' | 'accent';

export function statusTone(s?: string | null): Tone {
  const v = s || '';
  if (/^(active|eligible|open|passed)/.test(v)) return 'ok';
  if (/^(needs|stale|short|no expiry|expired|closed)/.test(v)) return 'warn';
  if (/^(blocked|malformed|ambiguous|unchecked)/.test(v)) return 'bad';
  return 'info';
}

export const executorTone: Record<string, Tone> = { active: 'ok', drained: '', unreachable: 'warn' };

export function niceStep(raw: number): number {
  const p = Math.pow(10, Math.floor(Math.log10(Math.max(raw, 1e-9))));
  for (const m of [1, 2, 5, 10]) if (m * p >= raw) return m * p;
  return 10 * p;
}
