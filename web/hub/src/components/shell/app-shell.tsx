"use client";

import Link from "next/link";
import { usePathname, useSearchParams } from "next/navigation";
import { Suspense } from "react";
import { ActivityIcon, BotIcon, BookOpenIcon, ChevronDownIcon, CircleDotIcon, CpuIcon, HomeIcon, ListTodoIcon, SettingsIcon, WifiIcon, WifiOffIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { hrefs } from "@/lib/nav";
import { useApi, useEventStream } from "@/lib/use-api";
import { COMMONS_HINT, type Org, type RecordsPayload } from "@/lib/api";
import { CommandPalette } from "./command-palette";
import { ThemeToggle } from "./theme-toggle";

const NAV: { href: string; label: string; icon: React.ComponentType<{ className?: string }>; match: (p: string) => boolean }[] = [
  { href: "/", label: "Hub", icon: HomeIcon, match: (p) => p === "/" },
  { href: "/tasks/", label: "Tasks", icon: ListTodoIcon, match: (p) => p.startsWith("/tasks") },
  { href: "/records/", label: "Records", icon: BookOpenIcon, match: (p) => p.startsWith("/records") },
  { href: "/compute/", label: "Compute", icon: CpuIcon, match: (p) => p.startsWith("/compute") },
  { href: "/agents/", label: "Agents", icon: BotIcon, match: (p) => p.startsWith("/agents") },
  { href: "/settings/", label: "Settings", icon: SettingsIcon, match: (p) => p.startsWith("/settings") },
];

function LiveDot() {
  const { state } = useEventStream();
  const label = state === "live" ? "live: the hub streams every store event" : state === "polling" ? "polling: the event stream is reconnecting, refreshing every 10 s" : "connecting to the hub";
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span className="inline-flex items-center gap-1.5 rounded-md px-2 py-1 text-xs text-muted-foreground" aria-live="polite" aria-label={label}>
          {state === "live" ? <WifiIcon className="size-3.5 text-ok" /> : state === "polling" ? <WifiOffIcon className="size-3.5 text-warn" /> : <CircleDotIcon className="size-3.5 animate-pulse" />}
          <span className="hidden sm:inline">{state}</span>
        </span>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

function ProjectSwitcher() {
  const params = useSearchParams();
  const pathname = usePathname();
  const current = pathname.startsWith("/project") ? params.get("slug") || "" : "";
  const { data } = useApi<Org>("org", { refresh: 30000 });
  // Proposed ADRs count as "for you" next to the awaiting proposals (ADR-028).
  const proposed = useApi<RecordsPayload>("records?kind=adr&status=Proposed", { refresh: 60000 });
  const decisions = new Map<string, number>();
  for (const r of proposed.data?.records ?? []) if (r.kind === "adr" && r.status === "Proposed") decisions.set(r.project, (decisions.get(r.project) || 0) + 1);
  const projects = data?.projects ?? [];
  const commons = data?.commons;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="outline" size="sm" className="max-w-[220px] gap-1.5" aria-label="Switch project">
          <ActivityIcon className="size-3.5 text-muted-foreground" />
          <span className="truncate">{current || "Projects"}</span>
          <ChevronDownIcon className="size-3.5 opacity-60" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-72">
        <DropdownMenuLabel>Projects on this machine</DropdownMenuLabel>
        {projects.length === 0 ? <div className="px-2 py-1.5 text-xs text-muted-foreground">{data ? "No project is registered yet: run pulse init in a checkout." : "Loading…"}</div> : null}
        {projects.map((p) => {
          const forYou = p.proposals_awaiting + (decisions.get(p.slug) || 0);
          return (
            <DropdownMenuItem key={p.slug} asChild>
              <Link href={hrefs.project(p.slug)} className="flex items-center justify-between gap-2">
                <span className="truncate">{p.slug}</span>
                <span className="flex items-center gap-1">
                  {p.brain ? <Badge variant="secondary">framework</Badge> : null}
                  {forYou ? <Badge>{forYou} for you</Badge> : null}
                  {!p.present ? <Badge variant="destructive">missing</Badge> : null}
                </span>
              </Link>
            </DropdownMenuItem>
          );
        })}
        {commons ? (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuLabel>Shared by every project</DropdownMenuLabel>
            <DropdownMenuItem asChild>
              <Link href={hrefs.project(commons.slug)} title={COMMONS_HINT} className="flex items-center justify-between gap-2">
                <span className="truncate">Commons</span>
                <span className="flex items-center gap-1">
                  {commons.questions_open ? <Badge variant="outline">{commons.questions_open} open</Badge> : null}
                  <Badge variant="outline">board</Badge>
                </span>
              </Link>
            </DropdownMenuItem>
          </>
        ) : null}
        <DropdownMenuSeparator />
        <DropdownMenuItem asChild>
          <Link href="/">All projects</Link>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export function AppShell({ children }: { children: React.ReactNode }) {
  const pathname = usePathname();
  return (
    <>
      <a href="#main" className="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:rounded-md focus:bg-primary focus:px-3 focus:py-1.5 focus:text-primary-foreground">
        Skip to content
      </a>
      <header className="sticky top-0 z-40 h-12 border-b bg-background/85 backdrop-blur supports-[backdrop-filter]:bg-background/70">
        <div className="mx-auto flex h-12 max-w-[1560px] items-center gap-2 px-3 sm:px-4">
          <Link href="/" className="mr-1 flex items-center gap-2 font-semibold tracking-tight" aria-label="PULSE hub">
            <span className="inline-flex size-6 items-center justify-center rounded-md bg-primary text-primary-foreground">
              <ActivityIcon className="size-3.5" />
            </span>
            <span className="hidden sm:inline">PULSE</span>
          </Link>
          <Suspense fallback={<div className="h-7 w-32 rounded-md bg-muted" />}>
            <ProjectSwitcher />
          </Suspense>
          <nav aria-label="Primary" className="ml-1 hidden items-center gap-0.5 md:flex">
            {NAV.map((n) => {
              const active = n.match(pathname);
              return (
                <Link key={n.href} href={n.href} aria-current={active ? "page" : undefined} className={cn("inline-flex h-8 items-center gap-1.5 rounded-md px-2.5 text-sm text-muted-foreground transition-colors hover:bg-muted hover:text-foreground", active && "bg-muted text-foreground")}>
                  <n.icon className="size-3.5" />
                  {n.label}
                </Link>
              );
            })}
          </nav>
          <div className="ml-auto flex items-center gap-1">
            <LiveDot />
            <Suspense fallback={null}>
              <CommandPalette />
            </Suspense>
            <ThemeToggle />
          </div>
        </div>
      </header>
      <main id="main" tabIndex={-1} className="mx-auto w-full max-w-[1560px] flex-1 px-3 pt-0 pb-20 outline-none sm:px-4 md:pb-4">
        {children}
      </main>
      <footer className="hidden border-t px-4 py-3 text-center text-xs text-muted-foreground md:block">
        PULSE hub · 127.0.0.1 only · the owner writes here: projects, tasks, messages, decisions, settings, compute; agents write with the CLI.
      </footer>
      {/* On phones the primary nav sits at the thumb, not in a second header row. */}
      <nav aria-label="Primary (compact)" className="fixed inset-x-0 bottom-0 z-40 grid grid-cols-6 border-t bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/85 md:hidden" style={{ paddingBottom: "env(safe-area-inset-bottom)" }}>
        {NAV.map((n) => {
          const active = n.match(pathname);
          return (
            <Link key={n.href} href={n.href} aria-current={active ? "page" : undefined} className={cn("flex h-14 flex-col items-center justify-center gap-1 text-[10.5px] font-medium text-muted-foreground", active && "text-foreground")}>
              <span className={cn("inline-flex h-6 w-10 items-center justify-center rounded-full", active && "bg-muted")}>
                <n.icon className="size-4" />
              </span>
              {n.label}
            </Link>
          );
        })}
      </nav>
    </>
  );
}
