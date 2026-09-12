"use client";

import Link from "next/link";
import { usePathname, useSearchParams } from "next/navigation";
import { Suspense } from "react";
import { ActivityIcon, BotIcon, BookOpenIcon, ChevronDownIcon, CpuIcon, FolderIcon, HomeIcon, ListTodoIcon, MoreHorizontalIcon, SettingsIcon, TerminalIcon, VoteIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { hrefs } from "@/lib/nav";
import { useApi, useEventStream } from "@/lib/use-api";
import { COMMONS_HINT, evolveGate, type Org, type ProjectsPayload, type RecordsPayload } from "@/lib/api";
import { gateStep } from "@/components/pulse/evolve";
import { CommandPalette } from "./command-palette";
import { ThemeToggle } from "./theme-toggle";

const NAV: { href: string; label: string; icon: React.ComponentType<{ className?: string }>; match: (p: string) => boolean }[] = [
  { href: "/", label: "Hub", icon: HomeIcon, match: (p) => p === "/" },
  { href: "/tasks/", label: "Tasks", icon: ListTodoIcon, match: (p) => p.startsWith("/tasks") },
  { href: "/votes/", label: "Votes", icon: VoteIcon, match: (p) => p.startsWith("/votes") },
  { href: "/records/", label: "Records", icon: BookOpenIcon, match: (p) => p.startsWith("/records") },
  { href: "/compute/", label: "Compute", icon: CpuIcon, match: (p) => p.startsWith("/compute") },
  { href: "/agents/", label: "Agents", icon: BotIcon, match: (p) => p.startsWith("/agents") },
  { href: "/harness/", label: "Harness", icon: TerminalIcon, match: (p) => p.startsWith("/harness") },
  { href: "/settings/", label: "Settings", icon: SettingsIcon, match: (p) => p.startsWith("/settings") },
];

const PRIMARY_NAV = NAV.slice(0, 4);
const MORE_NAV = NAV.slice(4);
const NAV_GROUPS = [
  { label: "Workspace", items: PRIMARY_NAV },
  { label: "Manage", items: MORE_NAV },
];

function projectInitial(slug: string) {
  return slug.trim().charAt(0).toUpperCase();
}

function ProjectAvatar({ slug }: { slug: string }) {
  const initial = projectInitial(slug);
  if (!initial) return <FolderIcon aria-hidden="true" className="size-4 text-muted-foreground" />;
  return <span aria-hidden="true" className="inline-flex size-6 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-xs font-semibold text-primary">{initial}</span>;
}

function LiveDot() {
  const { state } = useEventStream();
  const label = state === "live" ? "live: the hub streams every store event" : state === "polling" ? "polling: the event stream is reconnecting, refreshing every 10 s" : "connecting to the hub";
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span tabIndex={0} role="status" className="inline-flex h-11 shrink-0 items-center gap-2 rounded-full px-2.5 text-xs text-muted-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring sm:h-9 sm:border sm:border-border/60 sm:bg-card/50" aria-live="polite" aria-atomic="true" aria-label={label}>
          <span aria-hidden="true" className={cn("inline-flex size-2 shrink-0 rounded-full", state === "live" && "bg-ok", state === "polling" && "bg-warn", state === "connecting" && "bg-muted-foreground")} />
          <span className="sr-only capitalize sm:not-sr-only">{state}</span>
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
  const { data, error, loading } = useApi<Org>("org", { refresh: 30000 });
  // Proposed ADRs count as "for you" next to the awaiting proposals (ADR-028).
  const proposed = useApi<RecordsPayload>("records?kind=adr&status=Proposed", { refresh: 60000 });
  // So does a self-improvement step only the owner can take (ADR-029); the registry row carries the gates when the org row does not.
  const reg = useApi<ProjectsPayload>("projects", { refresh: 60000 });
  const decisions = new Map<string, number>();
  for (const r of proposed.data?.records ?? []) if (r.kind === "adr" && r.status === "Proposed") decisions.set(r.project, (decisions.get(r.project) || 0) + 1);
  const projects = data?.projects ?? [];
  const commons = data?.commons;
  const currentLabel = current && current === commons?.slug ? "Commons" : current || "All projects";
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="outline" size="sm" className="h-11 w-full min-w-0 justify-start gap-2 rounded-xl border-border/60 bg-card/60 px-2.5 shadow-none sm:px-3" aria-label={`Switch project, currently ${currentLabel}`} title={currentLabel}>
          {current ? <ProjectAvatar slug={current} /> : <FolderIcon aria-hidden="true" className="size-4 text-muted-foreground" />}
          <span className="min-w-0 flex-1 truncate text-left">{currentLabel}</span>
          <ChevronDownIcon aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" sideOffset={8} collisionPadding={12} className="w-80 max-w-[calc(100vw-1.5rem)] rounded-2xl p-2 shadow-pop">
        <DropdownMenuLabel className="px-3 py-2">Projects on this machine</DropdownMenuLabel>
        {error ? <div role="status" className="px-3 py-2 text-xs break-words text-destructive">Could not refresh projects: {error}{data ? " Showing the last available list." : ""}</div> : null}
        {loading ? <div role="status" className="px-3 py-2 text-xs text-muted-foreground">Loading projects...</div> : null}
        {!loading && !error && projects.length === 0 ? <div className="px-3 py-2 text-xs text-muted-foreground">No project is registered yet: run pulse init in a checkout.</div> : null}
        {projects.map((p) => {
          const step = gateStep(p.slug, evolveGate(p, reg.data?.projects?.find((r) => r.slug === p.slug)));
          const forYou = p.proposals_awaiting + (decisions.get(p.slug) || 0) + (step ? 1 : 0);
          return (
            <DropdownMenuItem key={p.slug} asChild className={cn("min-h-11 rounded-xl px-3 py-2", current === p.slug && "bg-primary/10 text-primary")}>
              <Link href={hrefs.project(p.slug)} aria-current={current === p.slug ? "true" : undefined} className="flex items-center justify-between gap-2">
                <span className="flex min-w-0 items-center gap-2">
                  <ProjectAvatar slug={p.slug} />
                  <span className="truncate">{p.slug}</span>
                </span>
                <span className="ml-auto flex shrink-0 items-center gap-1">
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
            <DropdownMenuSeparator className="mx-1 my-2" />
            <DropdownMenuLabel className="px-3 py-2">Shared by every project</DropdownMenuLabel>
            <DropdownMenuItem asChild className={cn("min-h-11 rounded-xl px-3 py-2", current === commons.slug && "bg-primary/10 text-primary")}>
              <Link href={hrefs.project(commons.slug)} aria-current={current === commons.slug ? "true" : undefined} title={COMMONS_HINT} className="flex items-center justify-between gap-2">
                <span className="flex min-w-0 items-center gap-2">
                  <ProjectAvatar slug={commons.slug} />
                  <span className="truncate">Commons</span>
                </span>
                <span className="ml-auto flex shrink-0 items-center gap-1">
                  {commons.questions_open ? <Badge variant="outline">{commons.questions_open} open</Badge> : null}
                  <Badge variant="outline">board</Badge>
                </span>
              </Link>
            </DropdownMenuItem>
          </>
        ) : null}
        <DropdownMenuSeparator className="mx-1 my-2" />
        <DropdownMenuItem asChild className={cn("min-h-11 rounded-xl px-3 py-2", pathname === "/" && "bg-primary/10 text-primary")}>
          <Link href="/" aria-current={pathname === "/" ? "page" : undefined} className="flex items-center gap-2">
            <FolderIcon aria-hidden="true" className="size-4 text-muted-foreground" />
            <span>All projects</span>
          </Link>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export function AppShell({ children }: { children: React.ReactNode }) {
  const pathname = usePathname();
  const currentPage = NAV.find((n) => n.match(pathname))?.label ?? (pathname.startsWith("/project") ? "Project" : "Workspace");
  const activeOverflow = MORE_NAV.find((n) => n.match(pathname));
  return (
    <div className="flex min-h-dvh min-w-0 flex-1 flex-col pl-[env(safe-area-inset-left)] pr-[env(safe-area-inset-right)] lg:pr-0 lg:pl-60">
      <a href="#main" className="sr-only focus:not-sr-only focus:fixed focus:top-3 focus:left-3 focus:z-[60] focus:rounded-xl focus:bg-primary focus:px-4 focus:py-3 focus:text-primary-foreground focus:shadow-pop">
        Skip to content
      </a>
      <aside aria-label="Workspace sidebar" className="surface-glass fixed inset-y-0 left-0 z-40 hidden w-60 flex-col border-r border-border/70 pt-[env(safe-area-inset-top)] lg:flex">
        <Link href="/" className="flex h-16 shrink-0 items-center gap-3 border-b border-border/70 px-5 outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring" aria-label="PULSE hub">
          <span className="inline-flex size-9 items-center justify-center rounded-xl bg-brand text-white shadow-card">
            <ActivityIcon aria-hidden="true" className="size-5" />
          </span>
          <span className="text-lg font-semibold tracking-tight">PULSE</span>
          <span className="ml-auto rounded-md border border-border/70 px-1.5 py-0.5 text-[10px] font-medium tracking-wide text-muted-foreground">HUB</span>
        </Link>
        <div className="flex min-h-0 flex-1 flex-col overflow-y-auto px-3 py-6">
          <nav aria-label="Primary" className="space-y-7">
            {NAV_GROUPS.map((group) => (
              <div key={group.label}>
                <p className="mb-2 px-3 text-[10px] font-semibold tracking-[0.16em] text-muted-foreground uppercase">{group.label}</p>
                <ul className="space-y-1">
                  {group.items.map((n) => {
                    const active = n.match(pathname);
                    return (
                      <li key={n.href}>
                        <Link href={n.href} aria-current={active ? "page" : undefined} className={cn("group flex min-h-11 items-center gap-3 rounded-xl px-3 text-sm font-medium text-muted-foreground outline-none transition-colors hover:bg-muted/70 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none", active && "bg-primary/10 text-primary ring-1 ring-primary/10 hover:bg-primary/10 hover:text-primary")}>
                          <n.icon aria-hidden="true" className="size-[18px] shrink-0" />
                          <span>{n.label}</span>
                          {active ? <span aria-hidden="true" className="ml-auto size-1.5 rounded-full bg-primary" /> : null}
                        </Link>
                      </li>
                    );
                  })}
                </ul>
              </div>
            ))}
          </nav>
          <div className="mt-auto pt-8">
            <div className="surface-hero rounded-2xl border border-border/60 p-4">
              <p className="flex items-center gap-2 text-xs font-semibold"><TerminalIcon aria-hidden="true" className="size-4 text-primary" /> Owner console</p>
              <p className="mt-2 text-xs leading-relaxed text-muted-foreground">Decisions stay with you. Agents work through the CLI.</p>
            </div>
          </div>
        </div>
      </aside>
      <header className="surface-glass sticky top-0 z-40 shrink-0 border-b border-border/70 pt-[env(safe-area-inset-top)]">
        <div className="mx-auto flex h-16 max-w-[1560px] items-center gap-2 px-3 sm:gap-3 sm:px-6 lg:px-8">
          <div className="flex min-w-0 flex-1 items-center gap-2 sm:gap-3">
            <Link href="/" className="flex h-11 min-w-11 shrink-0 items-center justify-center gap-2 rounded-xl font-semibold tracking-tight outline-none focus-visible:ring-2 focus-visible:ring-ring lg:hidden" aria-label="PULSE hub">
              <span className="inline-flex size-9 items-center justify-center rounded-xl bg-brand text-white shadow-card">
                <ActivityIcon aria-hidden="true" className="size-5" />
              </span>
              <span className="hidden md:inline">PULSE</span>
            </Link>
            <div className="hidden shrink-0 border-r border-border/70 pr-4 lg:block">
              <p className="text-[10px] font-medium tracking-[0.14em] text-muted-foreground uppercase">Workspace</p>
              <p className="mt-0.5 text-sm font-semibold">{currentPage}</p>
            </div>
            <div className="min-w-0 max-w-56 flex-1">
              <Suspense fallback={<div role="status" className="h-11 w-full rounded-xl bg-muted/60"><span className="sr-only">Loading project navigation</span></div>}>
                <ProjectSwitcher />
              </Suspense>
            </div>
          </div>
          <div className="ml-auto flex shrink-0 items-center gap-1 sm:gap-2">
            <LiveDot />
            <Suspense fallback={<div className="size-11 rounded-xl bg-muted/60 sm:w-44 xl:w-64" />}>
              <CommandPalette />
            </Suspense>
            <ThemeToggle />
          </div>
        </div>
      </header>
      <main id="main" tabIndex={-1} className="mx-auto w-full min-w-0 max-w-[1560px] flex-1 scroll-mt-[calc(5rem+env(safe-area-inset-top))] px-3 pt-0 pb-[calc(5.5rem+env(safe-area-inset-bottom))] outline-none sm:px-6 lg:px-8 lg:pb-8">
        {children}
      </main>
      <footer className="hidden border-t border-border/60 px-8 py-4 text-xs text-muted-foreground lg:block">
        <div className="mx-auto flex max-w-[1496px] flex-wrap items-center justify-between gap-2">
          <span className="font-medium">PULSE owner console</span>
          <span>Local to this machine. Agents write through the CLI.</span>
        </div>
      </footer>
      <nav aria-label="Primary (compact)" className="surface-glass fixed inset-x-0 bottom-0 z-40 border-t border-border/70 lg:hidden" style={{ paddingBottom: "env(safe-area-inset-bottom)", paddingLeft: "env(safe-area-inset-left)", paddingRight: "env(safe-area-inset-right)" }}>
        <div className="mx-auto grid h-16 max-w-2xl grid-cols-5 gap-1 px-2">
          {PRIMARY_NAV.map((n) => {
            const active = n.match(pathname);
            return (
              <Link key={n.href} href={n.href} aria-current={active ? "page" : undefined} className={cn("flex min-w-0 flex-col items-center justify-center gap-1 rounded-xl text-[11px] font-medium text-muted-foreground outline-none transition-colors hover:bg-muted/60 hover:text-foreground focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring motion-reduce:transition-none", active && "text-primary hover:text-primary")}>
                <span className={cn("inline-flex h-7 w-12 items-center justify-center rounded-xl", active && "bg-primary/10")}>
                  <n.icon aria-hidden="true" className="size-[18px]" />
                </span>
                {n.label}
              </Link>
            );
          })}
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button type="button" aria-label={activeOverflow ? `More navigation, current page: ${activeOverflow.label}` : "More navigation"} className={cn("flex min-w-0 flex-col items-center justify-center gap-1 rounded-xl text-[11px] font-medium text-muted-foreground outline-none transition-colors hover:bg-muted/60 hover:text-foreground focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring data-[state=open]:bg-muted/60 motion-reduce:transition-none", activeOverflow && "text-primary hover:text-primary")}>
                <span className={cn("inline-flex h-7 w-12 items-center justify-center rounded-xl", activeOverflow && "bg-primary/10")}>
                  {activeOverflow ? <activeOverflow.icon aria-hidden="true" className="size-[18px]" /> : <MoreHorizontalIcon aria-hidden="true" className="size-[18px]" />}
                </span>
                More
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent aria-label="More destinations" align="end" side="top" sideOffset={12} collisionPadding={16} className="w-64 max-w-[calc(100vw-2rem)] rounded-2xl p-2 shadow-pop">
              <DropdownMenuLabel className="px-3 py-2">Workspace tools</DropdownMenuLabel>
              {MORE_NAV.map((n) => {
                const active = n.match(pathname);
                return (
                  <DropdownMenuItem key={n.href} asChild className={cn("min-h-11 gap-3 rounded-xl px-3 py-2", active && "bg-primary/10 text-primary")}>
                    <Link href={n.href} aria-current={active ? "page" : undefined}>
                      <n.icon aria-hidden="true" className="size-[18px]" />
                      <span>{n.label}</span>
                      {active ? <span className="ml-auto text-xs">Current</span> : null}
                    </Link>
                  </DropdownMenuItem>
                );
              })}
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </nav>
    </div>
  );
}
