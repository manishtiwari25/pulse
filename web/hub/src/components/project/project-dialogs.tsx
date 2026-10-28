"use client";

import { useEffect, useRef, useState } from "react";
import { ArrowUpIcon, ChevronRightIcon, FolderIcon, FolderSearchIcon, GitBranchIcon, HomeIcon, Loader2Icon, PencilLineIcon, PlusIcon, RotateCcwIcon, Trash2Icon, XIcon } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { showApiError } from "@/components/pulse/confirm";
import { Disclosure, Notice } from "@/components/pulse/panel";
import { Mono } from "@/components/pulse/who";
import { api, enc, errorText, writes, type FoldersPayload, type InitResult, type RemovePreview } from "@/lib/api";
import { invalidate } from "@/lib/use-api";
import { cn } from "@/lib/utils";

// The project lifecycle from the console (ADR-027): register a checkout
// (`pulse init PATH`), rename a project (its display name; the slug in every
// URL, job and post stays), remove a project or an executor after a preview
// (`pulse remove … --dry-run`, then for real), and restore an inactive
// project. Every write goes through the hub; nothing here touches disk.

type Size = "xs" | "sm" | "default";
type Variant = "default" | "outline" | "ghost";

// ----- add project -------------------------------------------------------

export function AddProjectButton({ size = "default", variant = "default", onRegistered }: { size?: Size; variant?: Variant; onRegistered?: (r: InitResult) => void }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button size={size} variant={variant} onClick={() => setOpen(true)}>
        <PlusIcon /> Add project
      </Button>
      <AddProjectDialog open={open} onOpenChange={setOpen} onRegistered={onRegistered} />
    </>
  );
}

const pathLooksAbsolute = (p: string) => p.startsWith("/") || p.startsWith("~");

export function AddProjectDialog({ open, onOpenChange, onRegistered }: { open: boolean; onOpenChange: (o: boolean) => void; onRegistered?: (r: InitResult) => void }) {
  const [path, setPath] = useState("");
  const [name, setName] = useState("");
  const [verify, setVerify] = useState("");
  const [busy, setBusy] = useState(false);
  const [browsing, setBrowsing] = useState(false);
  const trimmed = path.trim();
  const valid = trimmed.length > 0 && pathLooksAbsolute(trimmed);

  const submit = async () => {
    setBusy(true);
    const id = toast.loading("Adding " + trimmed + "…");
    try {
      const r = await writes.registerProject({ path: trimmed, name: name.trim() || undefined, verify: verify.trim() || undefined });
      const job = r.job ? (r.job_queued ? " An agent sets it up as soon as the runner is on." : " Its setup is already waiting for the runner.") : "";
      toast.success((r.created ? "Added " : "Refreshed ") + r.project.slug + "." + job, { id });
      invalidate((p) => p === "org" || p === "projects" || p.startsWith("events") || p.startsWith("projects/") || p.startsWith("tasks"));
      onOpenChange(false);
      setPath("");
      setName("");
      setVerify("");
      setBrowsing(false);
      onRegistered?.(r);
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "Could not add the project");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={(o) => !busy && onOpenChange(o)}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Add a project</DialogTitle>
          <DialogDescription>
            Pick the folder of a git checkout on this machine. PULSE adds it as a project, and its lead agent starts on the tasks you give it.
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-3">
          <div className="grid gap-1.5">
            <Label htmlFor="ap-path">Folder</Label>
            <div className="flex gap-2">
              <Input
                id="ap-path"
                value={path}
                onChange={(e) => setPath(e.target.value)}
                placeholder="/Users/you/src/my-app"
                className="min-w-0 flex-1 font-mono text-[12.5px]"
                autoFocus
                spellCheck={false}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && valid && !busy) void submit();
                }}
              />
              <Button type="button" variant={browsing ? "secondary" : "outline"} onClick={() => setBrowsing((b) => !b)} aria-pressed={browsing} aria-controls="ap-browser">
                <FolderSearchIcon /> Browse
              </Button>
            </div>
            <p className="text-[13px] text-muted-foreground">The full path, starting with / or ~. Or press Browse to find it.</p>
            {browsing ? (
              <FolderBrowser
                id="ap-browser"
                start={valid ? trimmed : ""}
                onPick={(p) => {
                  setPath(p);
                  setBrowsing(false);
                }}
                onClose={() => setBrowsing(false)}
              />
            ) : null}
          </div>
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="grid gap-1.5">
              <Label htmlFor="ap-name">Name <span className="font-normal text-muted-foreground">(optional)</span></Label>
              <Input id="ap-name" value={name} onChange={(e) => setName(e.target.value)} placeholder="my-app" maxLength={100} />
              <p className="text-[13px] text-muted-foreground">Leave it empty to use the folder&apos;s name.</p>
            </div>
            <div className="grid gap-1.5">
              <Label htmlFor="ap-verify">Test command <span className="font-normal text-muted-foreground">(optional)</span></Label>
              <Input id="ap-verify" value={verify} onChange={(e) => setVerify(e.target.value)} placeholder="go test ./... or npm test" className="font-mono text-[12.5px]" spellCheck={false} />
              <p className="text-[13px] text-muted-foreground">Checks an agent&apos;s work before it is merged.</p>
            </div>
          </div>
          <div className="rounded-xl border bg-muted/40 px-4 py-3">
            <div className="mb-1.5 text-[13px] font-medium text-foreground">What happens next</div>
            <ol className="grid list-decimal gap-1 pl-4 text-[13px] leading-relaxed text-muted-foreground marker:text-muted-foreground/70">
              <li>PULSE adds the folder as a project. If it already knows the folder, it refreshes it instead.</li>
              <li>
                If the checkout has <Mono>docs/context/environment.md</Mono>, agents read it to learn how to build and run the project.
              </li>
              <li>An agent sets the project up as soon as the runner is on. Nothing in the folder changes until you give a task.</li>
            </ol>
          </div>
          <Disclosure summary="Prefer the terminal?" contentClassName="mt-1.5 text-[13px] text-muted-foreground">
            <p>
              Run <Mono>pulse init</Mono> in the folder. It does the same thing.
            </p>
          </Disclosure>
        </div>
        <DialogFooter className="items-center gap-2">
          {trimmed && !valid ? <span className="mr-auto text-[13px] text-bad">Start the path with / or ~.</span> : null}
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={busy}>
            Cancel
          </Button>
          <Button onClick={() => void submit()} disabled={busy || !valid}>
            {busy ? <Loader2Icon className="animate-spin" /> : <PlusIcon />}
            Add project
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ----- folder browser -----------------------------------------------------

// The machine's folders, one level at a time, from the hub (WU-023). A row
// opens a folder; the footer lets the owner take the folder they are in.
// Two shapes: `checkout` (Add project) puts "Use" on git checkouts only and
// warns when the folder is not one; `folder` (the held-out set, ADR-032)
// puts "Use" on every folder and warns only when the folder is a registered
// project or sits inside one - the hub refuses those for a private set.
export type FolderBrowserMode = "checkout" | "folder";

export function FolderBrowser({ id, start, mode = "checkout", onPick, onClose }: { id?: string; start: string; mode?: FolderBrowserMode; onPick: (path: string) => void; onClose: () => void }) {
  const [cursor, setCursor] = useState(start);
  const [data, setData] = useState<FoldersPayload | undefined>();
  const [error, setError] = useState<{ what: string; why: string } | null>(null);
  // The cursor the last answer was for; while it lags the cursor, we are loading.
  const [answered, setAnswered] = useState<string | null>(null);
  const seq = useRef(0);
  const loading = answered !== cursor;
  const folderMode = mode === "folder";
  // The project a folder belongs to, in folder mode: registered on it, or the
  // one the current listing sits inside (the hub reports that for the current
  // path; a child of a project folder is inside it too).
  const owner = (e: { registered: string }) => e.registered || data?.inside || data?.registered || "";
  // Whether a row's "Use" is offered: a checkout that is not yet registered
  // in checkout mode; any folder that is not a project's in folder mode.
  const usable = (e: { checkout: boolean; registered: string }) => (folderMode ? !owner(e) : e.checkout && !e.registered);
  const hereOwner = data ? data.registered || data.inside || "" : "";

  useEffect(() => {
    const n = ++seq.current;
    api<FoldersPayload>("folders" + (cursor ? "?path=" + enc(cursor) : ""))
      .then((d) => {
        if (seq.current !== n) return;
        setData(d);
        setError(null);
        setAnswered(cursor);
      })
      .catch((e: unknown) => {
        if (seq.current !== n) return;
        const t = errorText(e);
        setError({ what: t.error, why: t.why });
        setAnswered(cursor);
      });
  }, [cursor]);

  const here = data?.path ?? cursor;
  const segments = here.split("/").filter(Boolean);
  const crumbPath = (i: number) => "/" + segments.slice(0, i + 1).join("/");

  return (
    <div id={id} className="grid gap-2 rounded-xl border bg-muted/30 p-2" aria-label="Folder browser">
      <div className="flex flex-wrap items-center gap-1">
        {data?.roots.map((r) => (
          <Button key={r.path} type="button" size="xs" variant={here === r.path ? "secondary" : "ghost"} onClick={() => setCursor(r.path)} title={r.path}>
            {r.label === "Home" ? <HomeIcon /> : <FolderIcon />} {r.label}
          </Button>
        ))}
        <span className="flex-1" />
        <Button type="button" size="xs" variant="ghost" onClick={() => data?.parent && setCursor(data.parent)} disabled={!data?.parent || loading} title="Up one level">
          <ArrowUpIcon /> Up
        </Button>
        <Button type="button" size="xs" variant="ghost" onClick={onClose} aria-label="Close the folder browser">
          <XIcon />
        </Button>
      </div>

      <nav aria-label="Current folder" className="flex items-center gap-0.5 overflow-x-auto whitespace-nowrap font-mono text-xs text-muted-foreground">
        <button type="button" className="rounded px-1 hover:bg-muted hover:text-foreground" onClick={() => setCursor("/")}>
          /
        </button>
        {segments.map((seg, i) => (
          <span key={crumbPath(i)} className="flex items-center gap-0.5">
            {i > 0 ? <ChevronRightIcon className="size-3 shrink-0 opacity-60" /> : null}
            <button type="button" className={cn("rounded px-1 hover:bg-muted hover:text-foreground", i === segments.length - 1 && "text-foreground")} onClick={() => setCursor(crumbPath(i))}>
              {seg}
            </button>
          </span>
        ))}
      </nav>

      <div className="max-h-56 overflow-y-auto rounded-lg border bg-card">
        {error ? (
          <div className="p-2">
            <Notice tone="warn">
              <div className="text-[13px] font-medium">{error.what}</div>
              {error.why ? <div className="text-xs text-muted-foreground">{error.why}</div> : null}
              <div className="mt-1.5 flex gap-1.5">
                <Button type="button" size="xs" variant="outline" onClick={() => setCursor(data?.home || "~")}>
                  <HomeIcon /> Go to your home folder
                </Button>
              </div>
            </Notice>
          </div>
        ) : loading && !data ? (
          <ul className="divide-y">
            {[0, 1, 2, 3].map((i) => (
              <li key={i} className="flex items-center gap-2 px-2 py-1.5">
                <Skeleton className="size-4 rounded" />
                <Skeleton className="h-3.5 w-28" />
              </li>
            ))}
          </ul>
        ) : data && data.entries.length === 0 ? (
          <p className="px-3 py-4 text-center text-[13px] text-muted-foreground">This folder has no folders inside.</p>
        ) : (
          <ul className={cn("divide-y", loading && "opacity-60")}>
            {data?.entries.map((e) => (
              <li key={e.path} className="flex items-center gap-1.5 pr-1.5">
                <button
                  type="button"
                  className="flex min-w-0 flex-1 items-center gap-2 px-2 py-2 text-left text-sm hover:bg-muted/60"
                  onClick={() => setCursor(e.path)}
                  onDoubleClick={() => usable(e) && onPick(e.path)}
                  title={e.path}
                >
                  {e.checkout ? <GitBranchIcon className="size-4 shrink-0 text-ok" /> : <FolderIcon className="size-4 shrink-0 text-muted-foreground" />}
                  <span className="truncate">{e.name}</span>
                  {e.registered ? (
                    <Badge variant="secondary" className="h-5 shrink-0 px-1.5 text-xs font-normal">
                      {"Added as " + e.registered}
                    </Badge>
                  ) : folderMode && owner(e) ? (
                    <Badge variant="secondary" className="h-5 shrink-0 px-1.5 text-xs font-normal">
                      {"Inside " + owner(e)}
                    </Badge>
                  ) : e.checkout ? (
                    <Badge variant="outline" className="h-5 shrink-0 px-1.5 text-xs font-normal text-ok">
                      Git checkout
                    </Badge>
                  ) : null}
                  <ChevronRightIcon className="ml-auto size-3.5 shrink-0 opacity-40" />
                </button>
                {folderMode ? (
                  <Button type="button" size="xs" variant="outline" onClick={() => onPick(e.path)} disabled={!usable(e)} title={usable(e) ? undefined : "This folder belongs to a project. Pick one outside every project."}>
                    Use
                  </Button>
                ) : usable(e) ? (
                  <Button type="button" size="xs" variant="outline" onClick={() => onPick(e.path)}>
                    Use
                  </Button>
                ) : null}
              </li>
            ))}
            {data?.truncated ? <li className="px-2 py-1.5 text-xs text-muted-foreground">Showing the first {data.entries.length} folders.</li> : null}
          </ul>
        )}
      </div>

      {data && !error ? (
        <div className="flex flex-wrap items-center gap-2">
          <span className="min-w-0 flex-1 text-[13px] text-muted-foreground">
            {folderMode ? (
              data.registered ? (
                <>
                  This is the project <span className="font-medium text-foreground">{data.registered}</span>. Pick a folder outside every project.
                </>
              ) : data.inside ? (
                <>
                  This folder is inside the project <span className="font-medium text-foreground">{data.inside}</span>. Pick one outside every project.
                </>
              ) : (
                "This folder works for private tests."
              )
            ) : data.registered ? (
              <>
                Already added as <span className="font-medium text-foreground">{data.registered}</span>. Using it again refreshes that project.
              </>
            ) : data.checkout ? (
              "This folder is a git checkout. You can add it."
            ) : (
              "This folder isn't a git checkout. Open the folder that holds the repository."
            )}
          </span>
          {folderMode ? (
            <Button type="button" size="sm" variant={hereOwner ? "outline" : "default"} onClick={() => onPick(data.path)} disabled={!data.path || !!hereOwner} title={hereOwner ? "This folder belongs to a project. Pick one outside every project." : undefined}>
              <FolderIcon /> Use this folder
            </Button>
          ) : (
            <Button type="button" size="sm" variant={data.checkout ? "default" : "outline"} onClick={() => onPick(data.path)} disabled={!data.path}>
              {data.checkout ? <GitBranchIcon /> : <FolderIcon />} Use this folder
            </Button>
          )}
        </div>
      ) : null}
    </div>
  );
}

// ----- rename ------------------------------------------------------------

export function RenameDialog({ open, onOpenChange, slug, name, onRenamed }: { open: boolean; onOpenChange: (o: boolean) => void; slug: string; name: string; onRenamed?: (name: string) => void }) {
  const [busy, setBusy] = useState(false);
  return (
    <Dialog open={open} onOpenChange={(o) => !busy && onOpenChange(o)}>
      <DialogContent className="sm:max-w-md">
        <RenameBody slug={slug} name={name} busy={busy} setBusy={setBusy} onOpenChange={onOpenChange} onRenamed={onRenamed} />
      </DialogContent>
    </Dialog>
  );
}

function RenameBody({ slug, name, busy, setBusy, onOpenChange, onRenamed }: { slug: string; name: string; busy: boolean; setBusy: (b: boolean) => void; onOpenChange: (o: boolean) => void; onRenamed?: (name: string) => void }) {
  const [draft, setDraft] = useState(name);
  const next = draft.trim();
  const unchanged = next === name.trim();
  const tooLong = next.length > 120;
  const apply = async () => {
    if (!next || unchanged || tooLong) return;
    setBusy(true);
    const id = toast.loading("Renaming " + slug + "…");
    try {
      const r = await writes.renameProject(slug, next);
      toast.success("Renamed to " + r.name + ".", { id });
      invalidate();
      onOpenChange(false);
      onRenamed?.(r.name);
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, slug + " wasn't renamed");
    } finally {
      setBusy(false);
    }
  };
  return (
    <form
      className="contents"
      onSubmit={(e) => {
        e.preventDefault();
        void apply();
      }}
    >
      <DialogHeader>
        <DialogTitle className="anywhere">{"Rename " + (name || slug)}</DialogTitle>
        <DialogDescription className="anywhere">
          The name is what the console calls this project. Its id <Mono>{slug}</Mono>, used in every link, job and post, stays the same.
        </DialogDescription>
      </DialogHeader>
      <div className="grid gap-1.5">
        <Label htmlFor="rename-project-name">Name</Label>
        <Input id="rename-project-name" value={draft} onChange={(e) => setDraft(e.target.value)} autoFocus maxLength={160} disabled={busy} aria-invalid={tooLong || undefined} />
        {tooLong ? <p className="text-[13px] text-bad">A name can be at most 120 characters.</p> : null}
      </div>
      <DialogFooter>
        <Button type="button" variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>
          Keep the old name
        </Button>
        <Button type="submit" disabled={busy || !next || unchanged || tooLong}>
          {busy ? <Loader2Icon className="animate-spin" /> : <PencilLineIcon />}
          Rename
        </Button>
      </DialogFooter>
    </form>
  );
}

// ----- remove (project or executor) ---------------------------------------

type RemoveKind = "project" | "executor";

const REMOVE_TEXT: Record<RemoveKind, { title: string; description: string; action: string; keep: string; done: string }> = {
  project: {
    title: "Remove project",
    description: "Nothing is deleted. PULSE stops working on this project: jobs waiting to start are cancelled and no machine takes its work. Its folder, posts, runs and records all stay, and you can restore it later from the project list.",
    action: "Remove project",
    keep: "Keep the project",
    done: "You can restore it from the project list any time.",
  },
  executor: {
    title: "Remove machine",
    description: "PULSE forgets this machine and sends it no more work. Runs it made keep its name, and nothing on the machine itself changes.",
    action: "Remove machine",
    keep: "Keep the machine",
    done: "Nothing on the machine itself changed.",
  },
};

const removeWrite = (kind: RemoveKind, name: string, dryRun: boolean) => (kind === "project" ? writes.removeProject(name, dryRun) : writes.removeExecutor(name, dryRun));

export function RemoveDialog({ open, onOpenChange, kind, name, onApplied }: { open: boolean; onOpenChange: (o: boolean) => void; kind: RemoveKind; name: string; onApplied?: (p: RemovePreview) => void }) {
  // The body holds the preview and the busy flag; it mounts with the content,
  // so each opening starts from a fresh preview and the fetch runs once.
  const [busy, setBusy] = useState(false);
  return (
    <Dialog open={open} onOpenChange={(o) => !busy && onOpenChange(o)}>
      <DialogContent className="sm:max-w-lg">
        <RemoveBody kind={kind} name={name} busy={busy} setBusy={setBusy} onOpenChange={onOpenChange} onApplied={onApplied} />
      </DialogContent>
    </Dialog>
  );
}

function RemoveBody({ kind, name, busy, setBusy, onOpenChange, onApplied }: { kind: RemoveKind; name: string; busy: boolean; setBusy: (b: boolean) => void; onOpenChange: (o: boolean) => void; onApplied?: (p: RemovePreview) => void }) {
  const text = REMOVE_TEXT[kind];
  const [preview, setPreview] = useState<RemovePreview | null>(null);
  const [refusal, setRefusal] = useState<{ error: string; why?: string; fix?: string } | null>(null);

  // Preview first: the same write with dry_run so the owner reads what changes
  // and what stays before the destructive button does anything.
  useEffect(() => {
    let alive = true;
    removeWrite(kind, name, true)
      .then((p) => {
        if (alive) setPreview(p);
      })
      .catch((e: unknown) => {
        if (alive) setRefusal(errorText(e));
      });
    return () => {
      alive = false;
    };
  }, [kind, name]);

  const apply = async () => {
    setBusy(true);
    const id = toast.loading("Removing " + name + "…");
    try {
      const p = await removeWrite(kind, name, false);
      toast.success("Removed " + name + ". " + text.done, { id });
      invalidate();
      onOpenChange(false);
      onApplied?.(p);
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, name + " wasn't removed");
    } finally {
      setBusy(false);
    }
  };

  const leased = preview?.leased_jobs?.length ?? 0;
  return (
    <>
      <DialogHeader>
        <DialogTitle className="anywhere">{"Remove " + name + "?"}</DialogTitle>
        <DialogDescription className="anywhere">{text.description}</DialogDescription>
      </DialogHeader>
      {refusal ? (
        <Notice tone="bad" className="text-[13px]">
          <div className="font-medium">{refusal.error || "PULSE couldn't work out what removing it would change."}</div>
          {refusal.why ? <div className="mt-0.5 text-muted-foreground">Why: {refusal.why}</div> : null}
          {refusal.fix ? <div className="mt-0.5 text-muted-foreground">What to do: {refusal.fix}</div> : null}
        </Notice>
      ) : preview ? (
        <div className="grid gap-3" aria-live="polite">
          <PreviewList title="What stops" items={preview.changes} empty="Nothing is running or waiting, so nothing stops." tone="stop" />
          <PreviewList title="What stays" items={preview.stays} empty="Nothing else is touched." tone="stay" />
          {preview.yours?.length ? (
            <section>
              <PreviewTitle>Left for you to do</PreviewTitle>
              <p className="mb-1.5 text-[13px] text-muted-foreground">PULSE doesn&apos;t run these for you. Run them in a terminal if they apply.</p>
              <ul className="grid gap-1">
                {preview.yours.map((cmd, i) => (
                  <li key={i}>
                    <code className="block rounded-md bg-muted px-2 py-1 font-mono text-[12.5px] anywhere">{cmd}</code>
                  </li>
                ))}
              </ul>
            </section>
          ) : null}
          {leased ? <Notice tone="warn" className="text-[13px]">{(leased === 1 ? "1 job has" : leased + " jobs have") + " already started. " + (leased === 1 ? "It finishes" : "They finish") + " on its own; nothing new starts."}</Notice> : null}
        </div>
      ) : (
        <div className="grid gap-2" aria-busy="true" aria-label="Working out what changes">
          <Skeleton className="h-4 w-24" />
          <Skeleton className="h-8" />
          <Skeleton className="h-4 w-20" />
          <Skeleton className="h-8" />
        </div>
      )}
      <DialogFooter>
        <Button variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>
          {text.keep}
        </Button>
        <Button variant="destructive" disabled={busy || !preview} onClick={() => void apply()}>
          {busy ? <Loader2Icon className="animate-spin" /> : <Trash2Icon />}
          {text.action}
        </Button>
      </DialogFooter>
    </>
  );
}

function PreviewTitle({ children }: { children: React.ReactNode }) {
  return <div className="mb-1.5 text-[13px] font-semibold text-foreground">{children}</div>;
}

// The preview speaks the CLI's words; the owner reads the console's.
const plainWords = (s: string) =>
  s
    .replace(/\s+->\s+/g, " to ")
    .replace(/\bqueued\b/g, "waiting")
    .replace(/\bleased\b/g, "started")
    .replace(/\bexecutors?\b/g, (m) => (m.endsWith("s") ? "machines" : "machine"));

function PreviewList({ title, items, empty, tone }: { title: string; items: RemovePreview["changes"]; empty: string; tone: "stop" | "stay" }) {
  return (
    <section className="rounded-xl border px-4 py-3">
      <PreviewTitle>{title}</PreviewTitle>
      {items?.length ? (
        <ul className="grid gap-1.5 text-sm leading-snug">
          {items.map((it, i) => {
            // The CLI indents a job under its "N queued jobs" line; such a line is an id, kept small and set in mono.
            const sub = /^\s+/.test(it.label);
            return (
              <li key={i} className={cn("flex min-w-0 items-start gap-2 anywhere", sub && "pl-4")}>
                {sub ? null : <span className={cn("mt-1.5 size-1.5 shrink-0 rounded-full", tone === "stop" ? "bg-bad" : "bg-ok")} aria-hidden />}
                <span className="min-w-0">
                  {sub ? <Mono className="text-xs text-muted-foreground">{it.label.trim()}</Mono> : <span className="text-foreground">{sentenceCase(plainWords(it.label))}</span>}
                  {it.detail ? <span className="text-muted-foreground">{": " + plainWords(it.detail)}</span> : null}
                </span>
              </li>
            );
          })}
        </ul>
      ) : (
        <p className="text-[13px] text-muted-foreground">{empty}</p>
      )}
    </section>
  );
}

const sentenceCase = (s: string) => (s ? s.charAt(0).toUpperCase() + s.slice(1) : s);

// "Remove…" in a project header or at the end of an executor row. Executor
// rows are one line, so there it is an icon button with a tooltip.
export function RemoveMenuButton({ kind, name, onApplied, compact }: { kind: RemoveKind; name: string; onApplied?: (p: RemovePreview) => void; compact?: boolean }) {
  const [open, setOpen] = useState(false);
  const label = (kind === "executor" ? "Remove machine " : "Remove project ") + name;
  return (
    <>
      {compact ? (
        <Tooltip>
          <TooltipTrigger asChild>
            <Button variant="ghost" size="icon-xs" aria-label={label} className="text-muted-foreground hover:text-destructive" onClick={() => setOpen(true)}>
              <Trash2Icon />
            </Button>
          </TooltipTrigger>
          <TooltipContent>{kind === "executor" ? "Remove machine…" : "Remove project…"}</TooltipContent>
        </Tooltip>
      ) : (
        <Button variant="outline" size="sm" aria-label={label} className="text-muted-foreground hover:text-foreground" onClick={() => setOpen(true)}>
          <Trash2Icon /> {kind === "executor" ? "Remove machine…" : "Remove project…"}
        </Button>
      )}
      <RemoveDialog open={open} onOpenChange={setOpen} kind={kind} name={name} onApplied={onApplied} />
    </>
  );
}

// ----- restore ------------------------------------------------------------

export function RestoreButton({ slug, onRestored }: { slug: string; onRestored?: (r: InitResult) => void }) {
  const [busy, setBusy] = useState(false);
  const run = async () => {
    setBusy(true);
    const id = toast.loading("Restoring " + slug + "…");
    try {
      const r = await writes.restoreProject(slug);
      const job = r.job ? (r.job_queued ? " An agent sets it up again as soon as the runner is on." : " Its setup is already waiting for the runner.") : "";
      toast.success(slug + " is back." + job, { id });
      invalidate();
      onRestored?.(r);
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, slug + " wasn't restored");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Button variant="outline" size="sm" onClick={() => void run()} disabled={busy}>
      {busy ? <Loader2Icon className="animate-spin" /> : <RotateCcwIcon />}
      Restore
    </Button>
  );
}
