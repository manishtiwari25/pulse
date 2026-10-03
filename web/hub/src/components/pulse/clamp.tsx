"use client";

import { useEffect, useRef, useState } from "react";
import { cn } from "@/lib/utils";

// A paragraph folded to its first line or two, with "more" only when there
// is more: the settings page carries a sentence of explanation per row and
// per section, and the owner reads the names first (ADR-042 D5).
export function Clamp({ children, lines = 1, className }: { children: React.ReactNode; lines?: 1 | 2; className?: string }) {
  const ref = useRef<HTMLParagraphElement>(null);
  const [open, setOpen] = useState(false);
  const [overflows, setOverflows] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setOverflows(el.scrollHeight > el.clientHeight + 1);
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [children]);
  const folded = !open;
  return (
    <div className={cn("min-w-0", className)}>
      <p ref={ref} className={cn("text-[13px] leading-snug text-muted-foreground anywhere", folded && (lines === 1 ? "line-clamp-1" : "line-clamp-2"))}>
        {children}
      </p>
      {overflows || open ? (
        <button type="button" onClick={() => setOpen((v) => !v)} className="mt-1 rounded-full px-1.5 text-xs font-medium text-primary hover:bg-primary/10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" aria-expanded={open}>
          {open ? "less" : "more"}
        </button>
      ) : null}
    </div>
  );
}
