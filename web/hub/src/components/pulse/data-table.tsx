"use client";

import { useMemo, useState } from "react";
import { ArrowDownIcon, ArrowUpIcon, ArrowUpDownIcon } from "lucide-react";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { cn } from "@/lib/utils";

export interface Column<T> {
  key: string;
  label: React.ReactNode;
  render: (row: T, i: number) => React.ReactNode;
  sort?: (row: T) => string | number;
  align?: "left" | "right";
  nowrap?: boolean;
  className?: string;
  hide?: "sm" | "md" | "lg" | "xl" | "2xl";
}

const HIDE: Record<NonNullable<Column<unknown>["hide"]>, string> = { sm: "hidden sm:table-cell", md: "hidden md:table-cell", lg: "hidden lg:table-cell", xl: "hidden xl:table-cell", "2xl": "hidden 2xl:table-cell" };

// A sortable, dense table. Rows are clickable when onRow is given (keyboard:
// Enter/Space). Sorting is client-side and stable.
export function DataTable<T>({ columns, rows, rowKey, onRow, dense, caption, initialSort, initialDir = "desc", className, emptyText, rowClassName, rowAriaSelected }: { columns: Column<T>[]; rows: T[]; rowKey: (row: T, i: number) => string; onRow?: (row: T) => void; dense?: boolean; caption?: string; initialSort?: string; initialDir?: "asc" | "desc"; className?: string; emptyText?: string; rowClassName?: (row: T) => string | undefined; rowAriaSelected?: (row: T) => boolean }) {
  const [sortKey, setSortKey] = useState<string>(initialSort ?? "");
  const [dir, setDir] = useState<"asc" | "desc">(initialDir);
  const sorted = useMemo(() => {
    const col = columns.find((c) => c.key === sortKey);
    if (!col?.sort) return rows;
    const f = col.sort;
    return rows
      .map((r, i) => ({ r, i, v: f(r) }))
      .sort((a, b) => {
        const c = a.v < b.v ? -1 : a.v > b.v ? 1 : a.i - b.i;
        return dir === "asc" ? c : -c;
      })
      .map((x) => x.r);
  }, [rows, columns, sortKey, dir]);

  const toggle = (c: Column<T>) => {
    if (!c.sort) return;
    if (sortKey === c.key) setDir((d) => (d === "asc" ? "desc" : "asc"));
    else {
      setSortKey(c.key);
      setDir("desc");
    }
  };

  return (
    <div className={cn("surface-raised relative w-full overflow-hidden rounded-2xl border", className)}>
      <Table className={cn(dense ? "text-[13px]" : "text-sm")}>
        {caption ? <caption className="sr-only">{caption}</caption> : null}
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            {columns.map((c) => (
              <TableHead key={c.key} className={cn("h-9 bg-muted/50 text-xs font-medium text-muted-foreground first-letter:uppercase", c.align === "right" && "text-right", c.hide && HIDE[c.hide], c.className)} aria-sort={sortKey === c.key ? (dir === "asc" ? "ascending" : "descending") : undefined}>
                {c.sort ? (
                  <button type="button" onClick={() => toggle(c)} className={cn("inline-flex items-center gap-1 rounded-sm first-letter:uppercase hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40", c.align === "right" && "flex-row-reverse")}>
                    {c.label}
                    {sortKey === c.key ? dir === "asc" ? <ArrowUpIcon className="size-3 opacity-70" /> : <ArrowDownIcon className="size-3 opacity-70" /> : <ArrowUpDownIcon className="size-3 opacity-35" />}
                  </button>
                ) : (
                  c.label
                )}
              </TableHead>
            ))}
          </TableRow>
        </TableHeader>
        <TableBody>
          {sorted.length === 0 ? (
            <TableRow>
              <TableCell colSpan={columns.length} className="py-6 text-center text-muted-foreground">
                {emptyText ?? "Nothing to show yet."}
              </TableCell>
            </TableRow>
          ) : (
            sorted.map((r, i) => (
              <TableRow
                key={rowKey(r, i)}
                tabIndex={onRow ? 0 : undefined}
                aria-selected={rowAriaSelected ? rowAriaSelected(r) : undefined}
                onClick={onRow ? () => onRow(r) : undefined}
                onKeyDown={
                  onRow
                    ? (e) => {
                        if (e.key === "Enter" || e.key === " ") {
                          e.preventDefault();
                          onRow(r);
                        }
                      }
                    : undefined
                }
                className={cn("hover:bg-muted/40", onRow && "cursor-pointer focus-visible:bg-muted/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/50", rowClassName?.(r))}
              >
                {columns.map((c) => (
                  <TableCell key={c.key} className={cn(dense ? "py-2" : "py-2.5", "align-top", c.align === "right" && "text-right tnum", c.nowrap && "whitespace-nowrap", c.hide && HIDE[c.hide], c.className)}>
                    {c.render(r, i)}
                  </TableCell>
                ))}
              </TableRow>
            ))
          )}
        </TableBody>
      </Table>
    </div>
  );
}
