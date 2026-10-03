// Href builders. Every route is a static page; state lives in search params
// so the export has no dynamic segments and the pulse hub serves one
// index.html per route.

export type ProjectCanonicalTab = "overview" | "for-you" | "work" | "board" | "evolution" | "history";
export type ProjectLegacyTab = "tasks" | "chat" | "threads" | "proposals" | "suggestions" | "amendments" | "timeline" | "notices" | "env";
export type ProjectTab = ProjectCanonicalTab | ProjectLegacyTab;
export type ProjectTabResolution = { tab: ProjectCanonicalTab; sub?: string };

export const PROJECT_TABS: { id: ProjectTab; label: string }[] = [
  { id: "overview", label: "Overview" },
  { id: "for-you", label: "For you" },
  { id: "work", label: "Work" },
  { id: "board", label: "Board" },
  { id: "evolution", label: "Self-improvement" },
  { id: "history", label: "History" },
];

export function isProjectTab(s: string | null): s is ProjectTab {
  return !!s && !!resolveProjectTab(s);
}

export function resolveProjectTab(raw: string | null): ProjectTabResolution {
  switch (raw) {
    case "tasks":
    case "chat":
      return { tab: "work" };
    case "threads":
      return { tab: "board", sub: "threads" };
    case "suggestions":
      return { tab: "board", sub: "suggestions" };
    case "amendments":
      return { tab: "board", sub: "amendments" };
    case "notices":
      return { tab: "board", sub: "notices" };
    case "proposals":
      return { tab: "for-you" };
    case "timeline":
      return { tab: "history", sub: "timeline" };
    case "env":
      return { tab: "history", sub: "env" };
    case "for-you":
    case "work":
    case "board":
    case "evolution":
    case "history":
    case "overview":
      return { tab: raw };
    default:
      return { tab: "overview" };
  }
}

function qs(params: Record<string, string | number | undefined | null>): string {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) if (v !== undefined && v !== null && v !== "") q.set(k, String(v));
  const s = q.toString();
  return s ? "?" + s : "";
}

export const hrefs = {
  home: () => "/",
  project: (slug: string, tab?: ProjectTab | string, extra: Record<string, string | undefined> = {}) => {
    const r = resolveProjectTab(tab ?? null);
    return "/project/" + qs({ slug, tab: r.tab !== "overview" ? r.tab : undefined, sub: r.sub, ...extra });
  },
  thread: (slug: string, key: string) => "/project/" + qs({ slug, tab: "board", sub: "threads", thread: key }),
  post: (slug: string, id: string) => "/project/" + qs({ slug, tab: "board", sub: "threads", post: id }),
  task: (slug: string, id: string) => "/project/" + qs({ slug, tab: "work", task: id }),
  chat: (slug: string, task?: string) => "/project/" + qs({ slug, tab: "work", task }),
  records: (f: { kind?: string; status?: string; project?: string } = {}) => "/records/" + qs(f),
  record: (id: string, project?: string) => "/records/" + qs({ id, project }),
  codeDeliveries: (f: { project?: string } = {}) => "/code-deliveries/" + qs({ project: f.project }),
  compute: () => "/compute/",
  settings: (section?: string) => "/settings/" + (section ? "#" + section : ""),
  agents: () => "/agents/",
  harness: (f: { id?: string; project?: string; status?: string; agent?: string } = {}) => "/harness/" + qs({ id: f.id, project: f.project, status: f.status, agent: f.agent }),
  votes: (f: { project?: string; status?: string; kind?: string } = {}) => "/votes/" + qs(f),
  tasks: (f: { project?: string; status?: string } = {}) => "/tasks/" + qs(f),
};
