"use client";

import { useCallback, useSyncExternalStore } from "react";
import { LayoutGridIcon, TableIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

export type ViewMode = "cards" | "table";

const listeners = new Set<() => void>();
function subscribe(cb: () => void) {
  listeners.add(cb);
  window.addEventListener("storage", cb);
  return () => {
    listeners.delete(cb);
    window.removeEventListener("storage", cb);
  };
}

// Cards are the default everywhere; the dense table stays one click away for
// the long lists (records, agents). The choice is remembered per screen in
// localStorage. The server snapshot is the default, so the static export
// hydrates cleanly and the stored choice applies right after.
export function useViewMode(key: string, initial: ViewMode = "cards"): [ViewMode, (v: ViewMode) => void] {
  const storageKey = "pulse.view." + key;
  const mode = useSyncExternalStore<ViewMode>(
    subscribe,
    () => {
      try {
        const v = window.localStorage.getItem(storageKey);
        return v === "cards" || v === "table" ? v : initial;
      } catch {
        return initial;
      }
    },
    () => initial,
  );
  const set = useCallback(
    (v: ViewMode) => {
      try {
        window.localStorage.setItem(storageKey, v);
      } catch {
        // private mode or storage disabled: the choice lasts for this render only
      }
      listeners.forEach((l) => l());
    },
    [storageKey],
  );
  return [mode, set];
}

export function ViewToggle({ value, onChange, className }: { value: ViewMode; onChange: (v: ViewMode) => void; className?: string }) {
  return (
    <div role="group" aria-label="View" className={cn("surface-glass ring-soft inline-flex rounded-full border border-transparent bg-muted/35 p-1", className)}>
      <Tooltip>
        <TooltipTrigger asChild>
          <Button size="icon-xs" variant="ghost" aria-pressed={value === "cards"} aria-label="Cards" onClick={() => onChange("cards")} className={cn("rounded-full", value === "cards" ? "bg-card/90 text-foreground shadow-card hover:bg-card" : "text-muted-foreground hover:bg-muted hover:text-foreground")}>
            <LayoutGridIcon />
          </Button>
        </TooltipTrigger>
        <TooltipContent>Cards</TooltipContent>
      </Tooltip>
      <Tooltip>
        <TooltipTrigger asChild>
          <Button size="icon-xs" variant="ghost" aria-pressed={value === "table"} aria-label="Table" onClick={() => onChange("table")} className={cn("rounded-full", value === "table" ? "bg-card/90 text-foreground shadow-card hover:bg-card" : "text-muted-foreground hover:bg-muted hover:text-foreground")}>
            <TableIcon />
          </Button>
        </TooltipTrigger>
        <TooltipContent>Table</TooltipContent>
      </Tooltip>
    </div>
  );
}
