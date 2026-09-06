import type { ComponentChildren } from 'preact';
import { useMemo, useState } from 'preact/hooks';
import { cn } from '../../lib/cn';

export interface Column<T> {
  key: string;
  label: string;
  render: (row: T) => ComponentChildren;
  // A sort key makes the header a button; rows sort by it.
  sort?: (row: T) => string | number;
  align?: 'left' | 'right';
  nowrap?: boolean;
  width?: string;
}

export interface DataTableProps<T> {
  columns: Column<T>[];
  rows: T[];
  rowKey: (row: T, i: number) => string;
  onRow?: (row: T) => void;
  dense?: boolean;
  maxHeight?: string;
  caption?: string;
}

// A table with a sticky header, sortable columns, and keyboard-reachable
// rows: Enter on a focused row opens it; the links inside stay the pointer
// path. Every cell renders text nodes and elements built from JSON values.
export function DataTable<T>({ columns, rows, rowKey, onRow, dense, maxHeight, caption }: DataTableProps<T>) {
  const [sort, setSort] = useState<{ key: string; dir: 1 | -1 } | null>(null);
  const sorted = useMemo(() => {
    if (!sort) return rows;
    const col = columns.find((c) => c.key === sort.key);
    if (!col || !col.sort) return rows;
    const s = col.sort;
    return [...rows].sort((a, b) => {
      const x = s(a);
      const y = s(b);
      if (x === y) return 0;
      return (x < y ? -1 : 1) * sort.dir;
    });
  }, [rows, sort, columns]);
  const toggle = (key: string) => setSort((cur) => (cur && cur.key === key ? (cur.dir === 1 ? { key, dir: -1 } : null) : { key, dir: 1 }));
  return (
    <div className="-mx-1 overflow-auto rounded-card" style={maxHeight ? { maxHeight } : undefined}>
      <table className="w-full min-w-[520px] text-[13px]">
        {caption ? <caption className="sr-only">{caption}</caption> : null}
        <thead className="sticky top-0 z-10 bg-panel-2 backdrop-blur">
          <tr>
            {columns.map((c) => {
              const active = sort && sort.key === c.key;
              return (
                <th key={c.key} scope="col" style={c.width ? { width: c.width } : undefined} aria-sort={active ? (sort.dir === 1 ? 'ascending' : 'descending') : c.sort ? 'none' : undefined} className={cn('border-b border-line px-2 py-1.5 text-left text-[11.5px] font-medium tracking-wide text-muted uppercase whitespace-nowrap', c.align === 'right' && 'text-right')}>
                  {c.sort ? (
                    <button type="button" onClick={() => toggle(c.key)} className={cn('inline-flex items-center gap-1 rounded-sm hover:text-fg', active && 'text-fg')}>
                      {c.label}
                      <span aria-hidden="true" className="text-[10px] opacity-70">
                        {active ? (sort.dir === 1 ? '\u25b2' : '\u25bc') : '\u2195'}
                      </span>
                    </button>
                  ) : (
                    c.label
                  )}
                </th>
              );
            })}
          </tr>
        </thead>
        <tbody>
          {sorted.map((row, i) => (
            <tr
              key={rowKey(row, i)}
              tabIndex={onRow ? 0 : undefined}
              onClick={onRow ? (ev) => {
                const target = ev.target as HTMLElement;
                if (!target.closest('a, button, input, select, details, summary')) onRow(row);
              } : undefined}
              onKeyDown={onRow ? (ev) => {
                if (ev.key === 'Enter' && ev.target === ev.currentTarget) onRow(row);
              } : undefined}
              className={cn('border-b border-line/70 align-top last:border-0 hover:bg-panel-hover', onRow && 'cursor-pointer focus-visible:bg-panel-hover')}
            >
              {columns.map((c) => (
                <td key={c.key} className={cn('px-2', dense ? 'py-1' : 'py-1.5', c.nowrap ? 'whitespace-nowrap' : '[overflow-wrap:anywhere]', c.align === 'right' && 'tnum text-right')}>
                  {c.render(row)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
