"use client";

import Link from "next/link";
import { usePathname, useSearchParams } from "next/navigation";
import { useCallback, useEffect, useId, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { BotMessageSquareIcon, EraserIcon, Loader2Icon, SendIcon, SparklesIcon } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Textarea } from "@/components/ui/textarea";
import { Markdown } from "@/components/pulse/markdown";
import { errorText, writes, type AskSource, type AskStatus, type Org } from "@/lib/api";
import { ASK_MAX_QUESTION, ASK_STARTERS, type AskMessage, answerMessage, elapsedWord, factsOnlyHint, historyForRequest, readHistory, trimHistory, writeHistory } from "@/lib/ask";
import { hrefs } from "@/lib/nav";
import { useApi } from "@/lib/use-api";
import { cn } from "@/lib/utils";
import { recordKindWord } from "@/lib/words";

const ALL = "__all__";
const focusRing = "outline-none focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:ring-offset-2 focus-visible:ring-offset-background";

// One Ask panel per page. The host (mounted once by the shell) owns the sheet
// and the ⌘J / Ctrl-J shortcut; the sidebar button, the phone header's icon
// and the search palette open it, optionally with a question already typed.
const listeners = new Set<() => void>();
let isOpen = false;
let prefill = "";
let opener: HTMLElement | null = null;
export function openAsk(question = "", from?: HTMLElement | null) {
  if (!isOpen) opener = from ?? (document.activeElement instanceof HTMLElement && document.activeElement !== document.body ? document.activeElement : null);
  prefill = question;
  isOpen = true;
  listeners.forEach((l) => l());
}
function setAskOpen(open: boolean) {
  isOpen = open;
  if (!open) prefill = "";
  listeners.forEach((l) => l());
}
function subscribe(cb: () => void) {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}
const useAskOpen = () => useSyncExternalStore(subscribe, () => isOpen, () => false);

// A touch screen has no Shift+Enter: there the return key starts a new line
// and only the Send button sends.
const COARSE = "(pointer: coarse)";
function subscribeCoarse(cb: () => void) {
  if (typeof window === "undefined" || !window.matchMedia) return () => {};
  const mq = window.matchMedia(COARSE);
  mq.addEventListener("change", cb);
  return () => mq.removeEventListener("change", cb);
}
function useCoarsePointer(): boolean {
  return useSyncExternalStore(
    subscribeCoarse,
    () => typeof window !== "undefined" && !!window.matchMedia && window.matchMedia(COARSE).matches,
    () => false,
  );
}

// The console page that shows a cited source, or null when there is none.
export function sourceHref(s: AskSource): string | null {
  switch (s.source) {
    case "post":
      return s.project ? hrefs.post(s.project, s.id) : null;
    case "record":
      return hrefs.record(s.id, s.project || undefined);
    case "task":
      return s.project ? hrefs.task(s.project, s.id) : null;
    case "job":
      return s.project ? hrefs.project(s.project, "work") : null;
    case "run":
      return hrefs.harness({ id: s.id, project: s.project || undefined });
    default:
      return null;
  }
}

function sourceWord(s: AskSource): string {
  if (s.source === "post") return s.kind || "post";
  if (s.source === "record") return recordKindWord(s.kind);
  return s.source;
}

// The trigger: `sidebar` is the full-width button under Search in the desktop
// sidebar; `icon` is the speech bubble in the phone header.
export function AskTrigger({ variant = "sidebar" }: { variant?: "sidebar" | "icon" }) {
  const open = useAskOpen();
  return (
    <Button
      data-ask-trigger=""
      variant={variant === "icon" ? "ghost" : "outline"}
      size={variant === "icon" ? "icon" : "default"}
      className={cn(variant === "icon" ? "size-10 text-muted-foreground" : "h-9 w-full justify-start gap-2 px-2.5 font-normal text-muted-foreground")}
      onClick={(event) => openAsk("", event.currentTarget)}
      aria-label="Ask about your projects (Control or Command J)"
      aria-haspopup="dialog"
      aria-expanded={open}
      aria-keyshortcuts="Control+j Meta+j"
    >
      <BotMessageSquareIcon aria-hidden="true" className="size-4" />
      {variant === "sidebar" ? (
        <>
          <span>Ask</span>
          <kbd aria-hidden="true" className="pointer-events-none ml-auto inline-flex shrink-0 items-center rounded border bg-muted px-1.5 py-0.5 font-mono text-[10.5px] text-muted-foreground">⌘J</kbd>
        </>
      ) : null}
    </Button>
  );
}

// ⌘J / Ctrl-J: a quick, cited answer about a project from what the store knows.
export function AskHost() {
  const open = useAskOpen();
  const pathname = usePathname();
  const params = useSearchParams();
  const pageSlug = pathname.startsWith("/project") ? params.get("slug") || "" : "";
  const { data: org } = useApi<Org>("org", { refresh: 60000 });
  const projects = useMemo(() => [...(org?.projects ?? []), ...(org?.commons ? [org.commons] : [])], [org]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.isComposing && !e.repeat && (e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "j") {
        e.preventDefault();
        if (isOpen) setAskOpen(false);
        else openAsk();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // On close, back to whatever opened it, or to the trigger that is on screen.
  const wasOpen = useRef(false);
  useEffect(() => {
    if (open) {
      wasOpen.current = true;
      return;
    }
    if (!wasOpen.current) return;
    wasOpen.current = false;
    const target = opener;
    opener = null;
    const visibleTrigger = [...document.querySelectorAll<HTMLElement>("[data-ask-trigger]")].find((el) => el.offsetParent !== null);
    (target?.isConnected ? target : visibleTrigger)?.focus({ preventScroll: true });
  }, [open]);

  return (
    <Sheet open={open} onOpenChange={setAskOpen}>
      <SheetContent side="right" className="flex flex-col gap-0 overflow-hidden rounded-l-xl p-0 data-[side=right]:w-full data-[side=right]:sm:max-w-xl">
        {open ? <AskConversation pageProject={pageSlug} projects={projects} commonsSlug={org?.commons?.slug ?? ""} /> : null}
      </SheetContent>
    </Sheet>
  );
}

// Mounted fresh each time the panel opens: the project starts as the page's
// project ("" means every project) and the question box holds whatever the
// palette passed in.
function AskConversation({ pageProject, projects, commonsSlug }: { pageProject: string; projects: { slug: string }[]; commonsSlug: string }) {
  const [project, setProject] = useState(pageProject);
  const [draft, setDraft] = useState(() => prefill);
  const { data: status } = useApi<AskStatus>("ask", { refresh: false });
  const scope = project ? (project === commonsSlug ? "the shared board" : project) : "all projects";
  const modelWord = status?.model || "the default model";
  const signedOut = status ? !status.logged_in : false;

  return (
    <>
      <SheetHeader className="shrink-0 gap-2 border-b px-5 pt-5 pb-4 pr-14">
        <SheetTitle className="flex items-center gap-2 text-[17px]">
          <BotMessageSquareIcon aria-hidden className="size-5 text-primary" />
          Ask
        </SheetTitle>
        <SheetDescription>A quick answer about {scope}, from what PULSE knows right now, with the posts, tasks and jobs it drew on. Nothing is changed by asking.</SheetDescription>
        <div className="flex flex-wrap items-center gap-2 pt-1">
          <Select value={project || ALL} onValueChange={(v) => setProject(v === ALL ? "" : v)}>
            <SelectTrigger className="h-8 w-auto min-w-40 max-w-full" aria-label="Which project to ask about">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={ALL}>All projects</SelectItem>
              {projects.map((p) => (
                <SelectItem key={p.slug} value={p.slug}>
                  {p.slug === commonsSlug ? "Shared board" : p.slug}
                </SelectItem>
              ))}
              {project && !projects.some((p) => p.slug === project) ? <SelectItem value={project}>{project}</SelectItem> : null}
            </SelectContent>
          </Select>
          {status ? (
            signedOut ? (
              <Badge variant="outline" className="border-warn/50 text-warn">
                No model signed in: facts only
              </Badge>
            ) : (
              <Badge variant="outline" className="max-w-full truncate font-mono text-[11px]" title={"Answers come from " + modelWord + " via " + (status.provider.label || status.provider.name)}>
                {modelWord}
              </Badge>
            )
          ) : null}
        </div>
      </SheetHeader>
      <AskThread key={project} project={project} scope={scope} signedOut={signedOut} draft={draft} setDraft={setDraft} />
    </>
  );
}

// One conversation per project in this tab; a new project mounts a new thread.
function AskThread({ project, scope, signedOut, draft, setDraft }: { project: string; scope: string; signedOut: boolean; draft: string; setDraft: (d: string) => void }) {
  const coarse = useCoarsePointer();
  const hintId = useId();
  const [messages, setMessages] = useState<AskMessage[]>(() => readHistory(typeof window !== "undefined" ? window.sessionStorage : null, project));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const listRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const busyRef = useRef(false);

  useEffect(() => {
    writeHistory(window.sessionStorage, project, messages);
  }, [project, messages]);
  useEffect(() => {
    const el = listRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, busy]);
  useEffect(() => {
    inputRef.current?.focus({ preventScroll: true });
  }, []);

  const send = useCallback(
    async (text?: string) => {
      const question = (text ?? draft).trim();
      if (!question || busyRef.current) return;
      if (question.length > ASK_MAX_QUESTION) {
        setError("That question is too long: " + question.length + " characters, and Ask takes up to " + ASK_MAX_QUESTION + ".");
        return;
      }
      busyRef.current = true;
      setBusy(true);
      setError("");
      setDraft("");
      const at = new Date().toISOString();
      const history = historyForRequest(messages);
      setMessages((m) => trimHistory([...m, { role: "user", content: question, at }]));
      try {
        const answer = await writes.ask({ project: project || undefined, all_projects: !project, question, history });
        setMessages((m) => trimHistory([...m, answerMessage(answer, new Date().toISOString())]));
      } catch (e) {
        const t = errorText(e);
        setError([t.error, t.why, t.fix].filter(Boolean).join(" "));
        // The question stays in the box so the owner can fix and resend it.
        setDraft(question);
        setMessages((m) => (m.length && m[m.length - 1].role === "user" && m[m.length - 1].at === at ? m.slice(0, -1) : m));
      } finally {
        busyRef.current = false;
        setBusy(false);
        inputRef.current?.focus({ preventScroll: true });
      }
    },
    [draft, messages, project, setDraft],
  );

  const clear = () => {
    setMessages([]);
    setError("");
    toast.success("Ask history cleared for " + scope + ".");
    inputRef.current?.focus({ preventScroll: true });
  };

  return (
    <>
      <div ref={listRef} className="scroll-thin min-h-0 flex-1 overflow-y-auto px-5 py-4" aria-live="polite" aria-busy={busy}>
        {messages.length ? (
          <ol className="grid gap-4">
            <li className="-mb-2 flex justify-end">
              <Button type="button" variant="ghost" size="xs" className="text-muted-foreground" onClick={clear} disabled={busy}>
                <EraserIcon /> Clear this conversation
              </Button>
            </li>
            {messages.map((m, i) => (
              <li key={m.at + i} className={cn("grid gap-1.5", m.role === "user" && "justify-items-end")}>
                {m.role === "user" ? (
                  <div className="max-w-[88%] rounded-2xl rounded-br-md bg-primary px-3.5 py-2 text-sm whitespace-pre-wrap text-primary-foreground anywhere">{m.content}</div>
                ) : (
                  <AnswerBubble m={m} />
                )}
              </li>
            ))}
            {busy ? (
              <li className="flex items-center gap-2 text-sm text-muted-foreground" role="status">
                <Loader2Icon aria-hidden className="size-4 animate-spin" /> Looking through {scope}…
              </li>
            ) : null}
          </ol>
        ) : (
          <div className="grid gap-4">
            <div className="rounded-xl border bg-card/60 p-4 text-sm leading-relaxed text-muted-foreground">
              <p className="flex items-start gap-2">
                <SparklesIcon aria-hidden className="mt-0.5 size-4 shrink-0 text-primary" />
                <span>
                  Ask in your own words. Answers are short, name what they are based on, and link to it. {signedOut ? "No model is signed in right now, so you get the matching facts; sign in on the Runs page for worded answers." : "A follow-up question keeps the thread."}
                </span>
              </p>
            </div>
            <div className="grid gap-1.5">
              <p className="px-1 text-xs font-medium text-muted-foreground">Try one</p>
              <ul className="grid gap-1.5">
                {ASK_STARTERS.map((q) => (
                  <li key={q}>
                    <button type="button" onClick={() => void send(q)} disabled={busy} className={cn("w-full rounded-lg border bg-card px-3 py-2 text-left text-sm transition-colors hover:bg-muted disabled:opacity-60", focusRing)}>
                      {q}
                    </button>
                  </li>
                ))}
              </ul>
            </div>
            {busy ? (
              <p className="flex items-center gap-2 text-sm text-muted-foreground" role="status">
                <Loader2Icon aria-hidden className="size-4 animate-spin" /> Looking through {scope}…
              </p>
            ) : null}
          </div>
        )}
      </div>

      <form
        className="shrink-0 border-t bg-card p-3"
        onSubmit={(e) => {
          e.preventDefault();
          void send();
        }}
      >
        {error ? (
          <p role="alert" className="mb-2 rounded-lg bg-bad/10 px-3 py-2 text-[13px] text-bad anywhere">
            {error}
          </p>
        ) : null}
        <div className="flex items-end gap-2 rounded-xl border bg-background/40 p-1.5 pl-3 focus-within:border-ring focus-within:ring-2 focus-within:ring-ring/25">
          <Textarea
            ref={inputRef}
            aria-label={"Ask about " + scope}
            aria-describedby={hintId}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && !coarse && !e.nativeEvent.isComposing) {
                e.preventDefault();
                void send();
              }
            }}
            placeholder={"Ask about " + scope + "…"}
            enterKeyHint={coarse ? "enter" : "send"}
            rows={2}
            maxLength={ASK_MAX_QUESTION}
            className="max-h-40 min-h-10 flex-1 resize-none border-0 bg-transparent px-0 py-1.5 text-sm shadow-none focus-visible:ring-0 dark:bg-transparent"
            disabled={busy}
          />
          <Button type="submit" disabled={busy || !draft.trim()}>
            {busy ? <Loader2Icon className="animate-spin" /> : <SendIcon />}
            Ask
          </Button>
        </div>
        <p id={hintId} className="mt-1.5 px-1 text-xs text-muted-foreground">
          {coarse ? "Tap Ask when your question is ready." : "Enter asks, Shift+Enter starts a new line."} Only you see this conversation; it clears when the tab closes.
        </p>
      </form>
    </>
  );
}

function AnswerBubble({ m }: { m: Extract<AskMessage, { role: "assistant" }> }) {
  return (
    <div className="grid max-w-full gap-2 rounded-2xl rounded-bl-md border bg-card px-4 py-3">
      {m.facts_only ? (
        <p className="text-xs text-warn">{factsOnlyHint(m.note)}</p>
      ) : null}
      <Markdown source={m.content} className="text-sm" />
      {m.sources.length ? (
        <div className="grid gap-1 border-t pt-2">
          <p className="text-[11px] font-medium text-muted-foreground">Based on</p>
          <ul className="grid gap-1">
            {m.sources.map((s) => {
              const href = sourceHref(s);
              const label = (
                <>
                  <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 font-mono text-[10.5px] text-muted-foreground">{sourceWord(s)}</span>
                  <span className="min-w-0 flex-1 truncate">{s.summary || s.id}</span>
                  <span className="shrink-0 font-mono text-[10.5px] text-muted-foreground">{s.date ? s.date.slice(0, 10) : s.id}</span>
                </>
              );
              return (
                <li key={s.source + ":" + s.id}>
                  {href ? (
                    <Link href={href} title={s.id} className={cn("flex items-center gap-2 rounded-md px-1 py-0.5 text-[13px] hover:bg-muted", focusRing)}>
                      {label}
                    </Link>
                  ) : (
                    <span title={s.id} className="flex items-center gap-2 px-1 py-0.5 text-[13px]">
                      {label}
                    </span>
                  )}
                </li>
              );
            })}
          </ul>
        </div>
      ) : null}
      {!m.facts_only && (m.model || m.elapsed_ms) ? (
        <p className="text-[11px] text-muted-foreground">
          {[m.model, elapsedWord(m.elapsed_ms)].filter(Boolean).join(" · ")}
        </p>
      ) : null}
    </div>
  );
}
