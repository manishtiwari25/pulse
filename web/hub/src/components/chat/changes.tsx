"use client";

// What the agent produced, shown the way the Copilot and Claude Code apps
// show it: a compact list of files in the left column, and the diff or the
// artifact itself in the main area where there is room to read it. The
// transcript hands the main area over while a file is open and takes it
// back the moment the agent asks a question.

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { ArrowLeftIcon, ChevronDownIcon, DownloadIcon, FileDiffIcon, FileIcon, FileTextIcon, GitBranchIcon, ImageIcon, RefreshCwIcon, XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Markdown } from "@/components/pulse/markdown";
import { SkeletonRows } from "@/components/pulse/panel";
import { When } from "@/components/pulse/who";
import type { ChatArtifact, ChatChanges } from "@/lib/api";
import { chatArtifactHref, enc } from "@/lib/api";
import { type DiffFile, type DiffLineKind, type NumberedLine, diffTotals, dirOf, numberLines, splitDiff } from "@/lib/diff";
import { num, plural } from "@/lib/format";
import { useApi } from "@/lib/use-api";
import { cn } from "@/lib/utils";

// ----- files ----------------------------------------------------------------

export type FileKind = "image" | "html" | "markdown" | "text" | "pdf" | "other";

export function fileKind(name: string): FileKind {
  const ext = (name.split(".").pop() || "").toLowerCase();
  if (["png", "jpg", "jpeg", "gif", "webp", "svg", "avif"].includes(ext)) return "image";
  if (["html", "htm"].includes(ext)) return "html";
  if (["md", "markdown"].includes(ext)) return "markdown";
  if (ext === "pdf") return "pdf";
  if (["txt", "log", "json", "csv", "tsv", "yaml", "yml", "toml", "xml", "js", "ts", "tsx", "jsx", "py", "rs", "go", "sh", "css", "sql", "diff", "patch", "mjs", "cjs"].includes(ext)) return "text";
  return "other";
}

export function bytesWord(n: number): string {
  if (n < 1024) return n + " B";
  if (n < 1024 * 1024) return (n / 1024).toFixed(n < 10240 ? 1 : 0) + " KB";
  return (n / (1024 * 1024)).toFixed(1) + " MB";
}

export function fileIcon(name: string) {
  const kind = fileKind(name);
  return kind === "image" ? ImageIcon : kind === "markdown" || kind === "text" ? FileTextIcon : FileIcon;
}

export function FileKindIcon({ name, className }: { name: string; className?: string }) {
  const kind = fileKind(name);
  if (kind === "image") return <ImageIcon className={className} aria-hidden />;
  if (kind === "markdown" || kind === "text") return <FileTextIcon className={className} aria-hidden />;
  return <FileIcon className={className} aria-hidden />;
}

// What the main area shows instead of the transcript: one file of the diff
// (or the whole diff), or one artifact.
export type ChatView = { kind: "changes"; file?: number } | { kind: "artifact"; name: string };

// ----- the diff, loaded once per branch state -------------------------------

export interface ChatDiff {
  files: DiffFile[] | null;
  totals: ReturnType<typeof diffTotals> | null;
  error: string;
  loading: boolean;
  refresh: () => void;
}

// The branch state each loaded diff described, by request path, so a commit
// or a newly dirty file during a turn refreshes the view once and a
// conversation that remounts keeps the cached diff instead of fetching it
// again.
const diffKeys = new Map<string, string>();

export function useChatDiff(id: string, changes: ChatChanges | null): ChatDiff {
  const want = !!changes && (changes.commits > 0 || changes.dirty.length > 0);
  const path = want ? "chat/" + enc(id) + "/diff" : null;
  const key = changes ? changes.branch + ":" + changes.commits + ":" + changes.dirty.join("|") : "";
  const { data, error, loading, reload } = useApi<{ id: string; branch: string; diff: string }>(path, { refresh: false });
  useEffect(() => {
    if (!path) return;
    const seen = diffKeys.get(path);
    if (seen === undefined) {
      if (data) diffKeys.set(path, key);
    } else if (seen !== key) {
      diffKeys.set(path, key);
      void reload();
    }
  }, [path, key, data, reload]);
  const files = useMemo(() => (data && path ? splitDiff(data.diff) : null), [data, path]);
  const totals = useMemo(() => (files ? diffTotals(files) : null), [files]);
  const refresh = useCallback(() => {
    if (!path) return;
    diffKeys.set(path, key);
    void reload();
  }, [path, key, reload]);
  return { files, totals, error, loading, refresh };
}

export const FILE_STATUS: Record<DiffFile["status"], { letter: string; word: string; cls: string }> = {
  added: { letter: "A", word: "added", cls: "bg-ok/15 text-ok" },
  deleted: { letter: "D", word: "deleted", cls: "bg-bad/15 text-bad" },
  renamed: { letter: "R", word: "renamed", cls: "bg-primary/15 text-primary" },
  modified: { letter: "M", word: "modified", cls: "bg-muted text-muted-foreground" },
  binary: { letter: "B", word: "binary", cls: "bg-muted text-muted-foreground" },
};

function StatusLetter({ status, className }: { status: DiffFile["status"]; className?: string }) {
  const st = FILE_STATUS[status];
  return (
    <span className={cn("grid size-4 shrink-0 place-items-center rounded text-[10px] leading-none font-bold", st.cls, className)} role="img" aria-label={st.word} title={st.word}>
      {st.letter}
    </span>
  );
}

function Counts({ f, className }: { f: { added: number; removed: number }; className?: string }) {
  if (!f.added && !f.removed) return null;
  return (
    <span className={cn("flex shrink-0 items-center gap-1.5 text-[11px] tnum", className)}>
      {f.added ? <span className="text-ok">+{num(f.added)}</span> : null}
      {f.removed ? <span className="text-bad">−{num(f.removed)}</span> : null}
    </span>
  );
}

function fileTitle(f: DiffFile): string {
  if (!f.path) return "(notes)";
  return f.oldPath && f.oldPath !== f.path ? f.oldPath + " → " + f.path : f.path;
}

function UncommittedDivider({ className }: { className?: string }) {
  return (
    <div className={cn("flex items-center gap-2 text-[11px] font-medium text-you-ink", className)} role="separator" aria-label="Not committed yet">
      <span className="h-px flex-1 bg-you/40" /> not committed yet <span className="h-px flex-1 bg-you/40" />
    </div>
  );
}

// ----- left column: the changed files ---------------------------------------

export function ChangeList({ diff, view, onView }: { diff: ChatDiff; view: ChatView | null; onView: (v: ChatView) => void }) {
  const { files, totals, error, loading, refresh } = diff;
  if (!files) return error ? <p className="px-1 text-xs text-bad">{error}</p> : <SkeletonRows n={3} />;
  const selected = view?.kind === "changes" && view.file !== undefined ? view.file : -1;
  return (
    <div className="grid min-w-0 gap-1.5">
      {totals ? (
        <div className="flex min-w-0 items-center gap-x-2 px-1 text-xs text-muted-foreground">
          <span>{plural(totals.files, "file")}</span>
          <Counts f={totals} className="font-medium" />
          <button type="button" onClick={refresh} disabled={loading} className="ml-auto flex items-center gap-1 underline-offset-4 hover:underline" title="Load the diff again">
            <RefreshCwIcon className={cn("size-3", loading && "animate-spin")} /> Reload
          </button>
        </div>
      ) : null}
      {error ? <p className="px-1 text-xs text-bad">{error}</p> : null}
      {!files.length ? <p className="px-1 text-[13px] text-muted-foreground">The diff is empty.</p> : null}
      <ul className="grid min-w-0 gap-px" role="list" aria-label="Changed files">
        {files.map((f, i) => {
          const divider = f.uncommitted && (i === 0 || !files[i - 1].uncommitted);
          const name = f.path ? f.path.slice(f.path.lastIndexOf("/") + 1) : "(notes)";
          const dir = dirOf(f.path);
          return (
            <li key={i} className="grid min-w-0 gap-px">
              {divider ? <UncommittedDivider className="mt-1.5 mb-0.5 px-1" /> : null}
              <button
                type="button"
                onClick={() => onView({ kind: "changes", file: i })}
                aria-current={selected === i ? "true" : undefined}
                className="flex w-full min-w-0 items-center gap-2 rounded-md px-1.5 py-1 text-left text-[12.5px] transition-colors hover:bg-muted/60 aria-[current=true]:bg-primary/10 aria-[current=true]:hover:bg-primary/15"
                title={fileTitle(f) + " - open the diff"}
              >
                <StatusLetter status={f.status} />
                <span className="min-w-0 flex-1 truncate font-mono">
                  <span className="text-foreground">{name}</span>
                  {dir ? <span className="text-muted-foreground"> {dir}</span> : null}
                </span>
                <Counts f={f} />
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

// ----- main area: the diff, file by file ------------------------------------

export function ChangesViewer({ diff, changes, view, onClose }: { diff: ChatDiff; changes: ChatChanges; view: Extract<ChatView, { kind: "changes" }>; onClose: () => void }) {
  const { files, totals, error, loading, refresh } = diff;
  const scroller = useRef<HTMLDivElement | null>(null);
  const loaded = !!files;
  // Scroll to the file the owner picked; each pick is a new `view` object,
  // so picking the same file again scrolls back to it.
  useEffect(() => {
    if (view.file === undefined || !loaded) return;
    scroller.current?.querySelector<HTMLElement>('[data-file="' + view.file + '"]')?.scrollIntoView({ block: "start" });
  }, [view, loaded]);
  return (
    <div className="relative flex min-h-0 min-w-0 flex-col overflow-hidden rounded-2xl border bg-card shadow-card lg:flex-1" aria-label="Changes">
      <ViewerBar onClose={onClose} tools={<RefreshButton onClick={refresh} loading={loading} />}>
        <GitBranchIcon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
        <code className="min-w-0 truncate rounded bg-muted px-1 py-px font-mono text-[11px] text-foreground" title={changes.branch}>
          {changes.branch}
        </code>
        {totals ? (
          <span className="flex min-w-0 shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
            <span>{plural(totals.files, "file")}</span>
            <Counts f={totals} className="font-medium" />
            {totals.uncommitted ? <span className="hidden text-you-ink sm:inline">{num(totals.uncommitted)} not committed</span> : null}
          </span>
        ) : null}
      </ViewerBar>
      <div ref={scroller} className="scroll-thin max-h-[calc(100dvh-24rem)] min-h-[20rem] min-w-0 flex-1 overflow-y-auto bg-background/50 lg:max-h-none lg:min-h-0">
        {!files ? (
          error ? (
            <p className="p-4 text-sm text-bad">{error}</p>
          ) : (
            <SkeletonRows n={6} className="p-4" />
          )
        ) : !files.length ? (
          <p className="p-4 text-sm text-muted-foreground">The diff is empty.</p>
        ) : (
          files.map((f, i) => <FileDiff key={i} i={i} f={f} selected={view.file === i} divider={f.uncommitted && (i === 0 || !files[i - 1].uncommitted)} />)
        )}
      </div>
    </div>
  );
}

const LINE_BG: Record<DiffLineKind, string> = {
  add: "bg-ok/[0.1]",
  del: "bg-bad/[0.09]",
  hunk: "bg-primary/[0.05]",
  meta: "",
  context: "",
};

const SIGN: Record<DiffLineKind, string> = { add: "+", del: "-", hunk: "", meta: "", context: " " };

const BIG_FILE = 400;

function FileDiff({ i, f, selected, divider }: { i: number; f: DiffFile; selected: boolean; divider: boolean }) {
  const rows = useMemo(() => numberLines(f.lines).filter((l) => l.kind !== "meta"), [f.lines]);
  const [open, setOpen] = useState(selected || f.lines.length <= BIG_FILE);
  // Picking a folded file in the list unfolds it.
  const [wasSelected, setWasSelected] = useState(selected);
  if (selected !== wasSelected) {
    setWasSelected(selected);
    if (selected) setOpen(true);
  }
  return (
    <section data-file={i} className="border-b last:border-b-0" aria-label={fileTitle(f)}>
      {divider ? <UncommittedDivider className="bg-you/[0.07] px-4 py-1.5" /> : null}
      <header className={cn("sticky top-0 z-10 flex min-w-0 items-center gap-2 border-b bg-card/95 px-3 py-1.5 text-xs backdrop-blur-sm", selected && "shadow-[inset_3px_0_0_0_var(--primary)]")}>
        <button type="button" onClick={() => setOpen((o) => !o)} aria-expanded={open} className="flex min-w-0 flex-1 items-center gap-1.5 rounded-md text-left hover:text-foreground" title={open ? "Fold this file" : "Unfold this file"}>
          <ChevronDownIcon className={cn("size-3.5 shrink-0 text-muted-foreground transition-transform", !open && "-rotate-90")} aria-hidden />
          <StatusLetter status={f.status} />
          <span className="min-w-0 truncate font-mono font-medium text-foreground" title={fileTitle(f)}>
            {fileTitle(f)}
          </span>
          {!open ? <span className="shrink-0 text-muted-foreground">{plural(f.lines.length, "line")}</span> : null}
        </button>
        <Counts f={f} className="ml-auto" />
      </header>
      {!open ? null : f.status === "binary" ? (
        <p className="px-4 py-3 text-xs text-muted-foreground">Binary file - nothing to show line by line.</p>
      ) : !rows.length ? (
        <p className="px-4 py-3 text-xs text-muted-foreground">No lines changed - a mode or rename only.</p>
      ) : (
        <div className="scroll-thin overflow-x-auto">
          <table className="w-full border-collapse font-mono text-[12px] leading-[1.6]" style={{ tabSize: 4 }}>
            <tbody>
              {rows.map((l, j) => (
                <DiffRow key={j} l={l} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

function DiffRow({ l }: { l: NumberedLine }) {
  if (l.kind === "hunk") {
    return (
      <tr className={LINE_BG.hunk}>
        <td colSpan={4} className="px-3 py-0.5 text-[11px] whitespace-pre text-primary/80 select-none">
          {l.body}
        </td>
      </tr>
    );
  }
  const note = l.old === null && l.new === null;
  return (
    <tr className={LINE_BG[l.kind]}>
      <td className="w-[1%] min-w-[3ch] pr-1.5 pl-2.5 text-right align-top text-muted-foreground/60 select-none tnum">{l.old ?? ""}</td>
      <td className="w-[1%] min-w-[3ch] pr-1.5 pl-1 text-right align-top text-muted-foreground/60 select-none tnum">{l.new ?? ""}</td>
      <td className={cn("w-[1%] px-1.5 text-center align-top select-none", l.kind === "add" ? "text-ok" : l.kind === "del" ? "text-bad" : "text-transparent")}>{note ? "" : SIGN[l.kind]}</td>
      <td className={cn("pr-4 whitespace-pre", note ? "text-muted-foreground italic" : l.kind === "context" ? "text-foreground/75" : "text-foreground")}>{l.body || " "}</td>
    </tr>
  );
}

// ----- main area: one artifact ----------------------------------------------

export function ArtifactList({ id, artifacts, view, onView }: { id: string; artifacts: ChatArtifact[]; view: ChatView | null; onView: (v: ChatView) => void }) {
  return (
    <ul className="grid min-w-0 gap-px" role="list" aria-label="Artifacts">
      {artifacts.map((a) => {
        const current = view?.kind === "artifact" && view.name === a.name;
        return (
          <li key={a.name} className="flex min-w-0 items-center gap-1">
            <button
              type="button"
              onClick={() => onView({ kind: "artifact", name: a.name })}
              aria-current={current ? "true" : undefined}
              className="flex min-w-0 flex-1 items-center gap-1.5 rounded-md px-1.5 py-1 text-left text-[12.5px] transition-colors hover:bg-muted/60 aria-[current=true]:bg-primary/10 aria-[current=true]:hover:bg-primary/15"
              title={a.name + " - open it"}
            >
              <FileKindIcon name={a.name} className="size-3.5 shrink-0 text-muted-foreground" />
              <span className="min-w-0 truncate font-medium text-foreground">{a.name}</span>
              <span className="ml-auto shrink-0 text-[11px] text-muted-foreground tnum">{bytesWord(a.size)}</span>
            </button>
            <a href={chatArtifactHref(id, a.name)} download={a.name} className="rounded-md p-1 text-muted-foreground hover:bg-muted/60 hover:text-foreground" aria-label={"Download " + a.name} title="Download">
              <DownloadIcon className="size-3.5" />
            </a>
          </li>
        );
      })}
    </ul>
  );
}

export function ArtifactViewer({ id, artifact, onClose }: { id: string; artifact: ChatArtifact; onClose: () => void }) {
  const kind = fileKind(artifact.name);
  const href = chatArtifactHref(id, artifact.name);
  return (
    <div className="relative flex min-h-0 min-w-0 flex-col overflow-hidden rounded-2xl border bg-card shadow-card lg:flex-1" aria-label="Artifact">
      <ViewerBar
        onClose={onClose}
        tools={
          <Button asChild size="xs" variant="outline">
            <a href={href} download={artifact.name}>
              <DownloadIcon /> Download
            </a>
          </Button>
        }
      >
        <FileKindIcon name={artifact.name} className="size-3.5 shrink-0 text-muted-foreground" />
        <span className="min-w-0 truncate text-sm font-medium text-foreground" title={artifact.name}>
          {artifact.name}
        </span>
        <span className="shrink-0 text-xs text-muted-foreground tnum">{bytesWord(artifact.size)}</span>
        <When d={artifact.modified} className="hidden shrink-0 text-xs text-muted-foreground sm:inline" />
      </ViewerBar>
      <div className={cn("scroll-thin max-h-[calc(100dvh-24rem)] min-h-[20rem] min-w-0 flex-1 overflow-auto bg-background/50 lg:max-h-none lg:min-h-0", kind === "html" || kind === "pdf" ? "p-0" : "p-3 sm:p-4")}>
        <ArtifactBody key={href} href={href} name={artifact.name} kind={kind} />
      </div>
    </div>
  );
}

function ArtifactBody({ href, name, kind }: { href: string; name: string; kind: FileKind }) {
  const [text, setText] = useState<string | null>(null);
  const [err, setErr] = useState("");
  useEffect(() => {
    if (kind !== "markdown" && kind !== "text") return;
    let alive = true;
    fetch(href, { credentials: "same-origin" })
      .then(async (r) => {
        if (!r.ok) throw new Error("HTTP " + r.status);
        const t = await r.text();
        if (alive) setText(t.length > 200000 ? t.slice(0, 200000) + "\n… (truncated)" : t);
      })
      .catch((e) => {
        if (alive) setErr("Could not open it (" + (e instanceof Error ? e.message : String(e)) + "). Download it instead.");
      });
    return () => {
      alive = false;
    };
  }, [href, kind]);
  if (kind === "image") {
    return (
      <div className="grid min-h-full place-items-center">
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img src={href} alt={name} className="pop max-h-[calc(100dvh-16rem)] w-auto max-w-full rounded-lg border bg-muted/30 object-contain shadow-card" />
      </div>
    );
  }
  if (kind === "html") return <iframe src={href} title={name} sandbox="allow-scripts" className="block h-full min-h-[28rem] w-full bg-white" />;
  if (kind === "pdf") return <iframe src={href} title={name} className="block h-full min-h-[28rem] w-full bg-white" />;
  if (kind === "markdown" || kind === "text") {
    if (err) return <p className="text-sm text-bad">{err}</p>;
    if (text === null) return <SkeletonRows n={6} />;
    return kind === "markdown" ? (
      <article className="pop mx-auto w-full max-w-3xl rounded-xl border bg-card px-5 py-4 text-sm shadow-card">
        <Markdown source={text} />
      </article>
    ) : (
      <pre className="pop min-w-0 rounded-xl border bg-card px-4 py-3 font-mono text-[12px] leading-relaxed whitespace-pre-wrap text-foreground/90 anywhere">{text}</pre>
    );
  }
  return (
    <p className="text-sm text-muted-foreground">
      Nothing to preview for this kind of file.{" "}
      <a href={href} download={name} className="text-primary underline-offset-4 hover:underline">
        Download {name}
      </a>
    </p>
  );
}

// ----- shared bits -----------------------------------------------------------

function ViewerBar({ children, tools, onClose }: { children: ReactNode; tools?: ReactNode; onClose: () => void }) {
  return (
    <div className="flex min-w-0 items-center gap-2 border-b bg-card px-2 py-1.5">
      <Button type="button" size="xs" variant="ghost" onClick={onClose} className="shrink-0 text-muted-foreground hover:text-foreground" title="Back to the conversation (Esc)">
        <ArrowLeftIcon /> <span className="hidden sm:inline">Conversation</span>
      </Button>
      <span className="h-4 w-px shrink-0 bg-border" aria-hidden />
      <div className="flex min-w-0 flex-1 items-center gap-2">{children}</div>
      <div className="ml-auto flex shrink-0 items-center gap-1">
        {tools}
        <Button type="button" size="icon-xs" variant="ghost" onClick={onClose} aria-label="Close" title="Back to the conversation (Esc)">
          <XIcon />
        </Button>
      </div>
    </div>
  );
}

function RefreshButton({ onClick, loading }: { onClick: () => void; loading: boolean }) {
  return (
    <Button type="button" size="icon-xs" variant="ghost" onClick={onClick} disabled={loading} aria-label="Load the diff again" title="Load the diff again">
      <RefreshCwIcon className={cn(loading && "animate-spin")} />
    </Button>
  );
}

// The button the side panel shows under the file list.
export function ReviewAllButton({ active, onClick }: { active: boolean; onClick: () => void }) {
  return (
    <Button type="button" size="sm" variant={active ? "secondary" : "outline"} onClick={onClick} title="Open the whole diff in the main area">
      <FileDiffIcon /> Review all
    </Button>
  );
}
