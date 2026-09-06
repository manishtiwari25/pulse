import { useEffect, useState } from 'preact/hooks';
import { PLACE } from '../api';

export const RANGES: [string, string][] = [['24h', '24 hours'], ['7d', '7 days'], ['30d', '30 days']];
export const BYS = ['project', 'kind', 'total'];

export interface Route {
  hash: string;
  view: string;
  arg: string;
  params: URLSearchParams;
}

export const DEFAULT_VIEW = PLACE.mode === 'org' ? '#/org' : '#/overview';

export function parseRoute(full: string): Route {
  const q = full.indexOf('?');
  const h = q === -1 ? full : full.slice(0, q);
  const params = new URLSearchParams(q === -1 ? '' : full.slice(q + 1));
  const m = h.match(/^#\/(\w+)(?:\/(.*))?$/);
  let arg = '';
  if (m && m[2]) {
    try {
      arg = decodeURIComponent(m[2]);
    } catch {
      arg = m[2];
    }
  }
  return { hash: h, view: m && m[1] ? m[1] : '', arg, params };
}

export function useRoute(): Route {
  const [route, setRoute] = useState<Route>(() => parseRoute(location.hash || DEFAULT_VIEW));
  useEffect(() => {
    const on = () => setRoute(parseRoute(location.hash || DEFAULT_VIEW));
    window.addEventListener('hashchange', on);
    return () => window.removeEventListener('hashchange', on);
  }, []);
  return route;
}

export function go(hash: string): void {
  location.hash = hash;
}
