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
        <Button variant="outline" size="icon" className="relative size-11 rounded-xl text-muted-foreground" aria-label={label}>
          <SunIcon aria-hidden="true" className="size-[18px] scale-100 rotate-0 transition-transform motion-reduce:transition-none dark:scale-0 dark:-rotate-90" />
          <MoonIcon aria-hidden="true" className="absolute size-[18px] scale-0 rotate-90 transition-transform motion-reduce:transition-none dark:scale-100 dark:rotate-0" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" sideOffset={8} collisionPadding={12} className="surface-glass w-48 rounded-2xl p-2 shadow-pop">
        <DropdownMenuLabel className="px-3 py-2">Appearance</DropdownMenuLabel>
        <DropdownMenuRadioGroup aria-label="Color theme" value={current} onValueChange={setTheme}>
          <DropdownMenuRadioItem value="light" className="min-h-11 gap-3 rounded-xl py-2 pl-3">
            <SunIcon aria-hidden="true" /> Light
          </DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="dark" className="min-h-11 gap-3 rounded-xl py-2 pl-3">
            <MoonIcon aria-hidden="true" /> Dark
          </DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="system" className="min-h-11 gap-3 rounded-xl py-2 pl-3">
            <MonitorIcon aria-hidden="true" /> System
          </DropdownMenuRadioItem>
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
