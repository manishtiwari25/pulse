// Href builders. Every route is a static page; state lives in search params
// so the export has no dynamic segments and the pulse hub serves one
// index.html per route.

export type ProjectTab = "overview" | "tasks" | "chat" | "threads" | "proposals" | "suggestions" | "amendments" | "evolution" | "timeline" | "notices" | "env";

export const PROJECT_TABS: { id: ProjectTab; label: string }[] = [
  { id: "overview", label: "Overview" },
  { id: "tasks", label: "Tasks" },
  { id: "chat", label: "Chat" },
  { id: "threads", label: "Threads" },
  { id: "proposals", label: "Proposals" },
  { id: "suggestions", label: "Suggestions" },
  { id: "amendments", label: "Amendments" },
  { id: "evolution", label: "Evolution" },
  { id: "timeline", label: "Timeline" },
  { id: "notices", label: "Notices" },
  { id: "env", label: "Environment" },
];

export function isProjectTab(s: string | null): s is ProjectTab {
  return !!s && PROJECT_TABS.some((t) => t.id === s);
}

function qs(params: Record<string, string | number | undefined | null>): string {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) if (v !== undefined && v !== null && v !== "") q.set(k, String(v));
  const s = q.toString();
  return s ? "?" + s : "";
}

export const hrefs = {
  home: () => "/",
  project: (slug: string, tab?: ProjectTab | string, extra: Record<string, string | undefined> = {}) => "/project/" + qs({ slug, tab: tab && tab !== "overview" ? tab : undefined, ...extra }),
  thread: (slug: string, key: string) => "/project/" + qs({ slug, tab: "threads", thread: key }),
  post: (slug: string, id: string) => "/project/" + qs({ slug, tab: "threads", post: id }),
  task: (slug: string, id: string) => "/project/" + qs({ slug, tab: "tasks", task: id }),
  chat: (slug: string, task?: string) => "/project/" + qs({ slug, tab: "chat", task }),
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
