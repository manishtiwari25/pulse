"use client";

import { useSyncExternalStore } from "react";
import { useTheme } from "next-themes";
import { MoonIcon, SunIcon, MonitorIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuLabel, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";

const subscribe = () => () => {};
const getClientSnapshot = () => true;
const getServerSnapshot = () => false;

export function ThemeToggle() {
  const mounted = useSyncExternalStore(subscribe, getClientSnapshot, getServerSnapshot);
  const { theme, resolvedTheme, setTheme } = useTheme();

  const current = mounted ? theme || "system" : "system";
  const label = mounted ? `Change color theme, current theme: ${current === "system" ? `system (${resolvedTheme ?? "auto"})` : current}` : "Change color theme";

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon" className="relative size-10 text-muted-foreground lg:size-8" aria-label={label}>
          <SunIcon aria-hidden="true" className="size-[18px] scale-100 rotate-0 transition-transform motion-reduce:transition-none dark:scale-0 dark:-rotate-90 lg:size-4" />
          <MoonIcon aria-hidden="true" className="absolute size-[18px] scale-0 rotate-90 transition-transform motion-reduce:transition-none dark:scale-100 dark:rotate-0 lg:size-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" sideOffset={8} collisionPadding={12} className="w-44 rounded-xl p-1.5 shadow-pop">
        <DropdownMenuLabel className="px-2.5 py-1.5 text-xs font-medium text-muted-foreground">Appearance</DropdownMenuLabel>
        <DropdownMenuRadioGroup aria-label="Color theme" value={current} onValueChange={setTheme}>
          <DropdownMenuRadioItem value="light" className="min-h-9 gap-2.5 rounded-lg py-1.5 pl-2.5">
            <SunIcon aria-hidden="true" /> Light
          </DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="dark" className="min-h-9 gap-2.5 rounded-lg py-1.5 pl-2.5">
            <MoonIcon aria-hidden="true" /> Dark
          </DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="system" className="min-h-9 gap-2.5 rounded-lg py-1.5 pl-2.5">
            <MonitorIcon aria-hidden="true" /> System
          </DropdownMenuRadioItem>
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
