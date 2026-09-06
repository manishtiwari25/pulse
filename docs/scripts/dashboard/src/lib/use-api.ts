import { useEffect, useRef, useState } from 'preact/hooks';

export interface Loaded<T> {
  data: T | null;
  error: string;
  loading: boolean;
  reload: () => void;
}

// Fetch once per key (and again when `revision` changes, which the poller
// bumps when the snapshot stamp moves); keep the previous data while the next
// load is in flight so a quiet redraw never flashes a skeleton.
export function useApi<T>(load: () => Promise<T>, key: string, revision: number): Loaded<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(true);
  const [tick, setTick] = useState(0);
  const alive = useRef(0);
  useEffect(() => {
    const id = ++alive.current;
    setLoading(true);
    load().then(
      (d) => {
        if (alive.current !== id) return;
        setData(d);
        setError('');
        setLoading(false);
      },
      (e: unknown) => {
        if (alive.current !== id) return;
        setError(e instanceof Error ? e.message : String(e));
        setLoading(false);
      },
    );
  }, [key, revision, tick]);
  return { data, error, loading, reload: () => setTick((t) => t + 1) };
}

export function settle<T>(p: Promise<T>, fallback: T): Promise<T> {
  return p.catch(() => fallback);
}
