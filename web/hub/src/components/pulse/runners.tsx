"use client";

import Link from "next/link";
import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Notice } from "./panel";
import { Mono, ToneBadge, When } from "./who";
import { errorText, writes, type RunnerNote, type RunnersNote } from "@/lib/api";
import { andList, num, plural, sizeText } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { invalidate } from "@/lib/use-api";

export const RUNNERS_SETTING = "daemon.sweep_runners";

// The words of `daemon.sweep_runners`, lower-cased.
const words = (setting: string) =>
  setting
    .split(/[,\s;]+/)
    .map((w) => w.trim().toLowerCase())
    .filter(Boolean);

// The ids of the programs PULSE can prune and found on this machine.
export const sweepable = (note: RunnersNote) => note.runners.filter((r) => r.sweeps).map((r) => r.id);

// The setting after the owner flips one program: `all` becomes the explicit
// list first, `none` and the empty answer become that one program, and a
// list that empties out says `none` so the hub stops asking.
export function toggled(note: RunnersNote, id: string, on: boolean): string {
  const current = words(note.setting);
  let ids = current.includes("all") ? sweepable(note) : current.filter((w) => w !== "none" && w !== "all");
  ids = on ? [...new Set([...ids, id])] : ids.filter((w) => w !== id);
  return ids.length ? ids.join(",") : "none";
}

function save(value: string) {
  return writes
    .setSetting(RUNNERS_SETTING, value)
    .then(() => {
      invalidate("hygiene");
      invalidate("settings");
    })
    .catch((e) => {
      const t = errorText(e);
      toast.error(t.error, { description: [t.why, t.fix].filter(Boolean).join(" ") });
    });
}

// One program: what is installed, what it has on disk, what PULSE did.
function RunnerRow({ r, note, busy, onToggle }: { r: RunnerNote; note: RunnersNote; busy: boolean; onToggle: (on: boolean) => void }) {
  const installed = !!r.binary;
  const size = r.bytes ? (r.capped ? "at least " : "") + sizeText(r.bytes) : note.measured_at ? sizeText(0) : "";
  const sessions = !r.dir_exists
    ? "No data on disk yet."
    : r.sessions
      ? plural(r.sessions, "session") + (r.stale ? ", " + num(r.stale) + " older than " + plural(note.keep_days, "day") : "") + (r.live ? ", " + num(r.live) + " in use" : "") + "."
      : note.at
        ? "No sessions on disk."
        : "";
  return (
    <li className="min-w-0 rounded-xl border bg-muted/30 px-3 py-2.5 text-[13px]">
      <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
        <span className="font-medium text-foreground">{r.label}</span>
        {installed ? r.version ? <span className="text-muted-foreground">{r.version}</span> : null : <ToneBadge tone="neutral">Not installed, data left behind</ToneBadge>}
        {size ? <span className="tnum ml-auto text-muted-foreground">{size}</span> : null}
        {r.sweeps ? (
          <label className={"flex shrink-0 items-center gap-2 text-muted-foreground " + (size ? "" : "ml-auto")}>
            <span>{r.managed ? "PULSE cleans up" : "Left alone"}</span>
            <Switch checked={r.managed} disabled={busy} onCheckedChange={onToggle} aria-label={"Let PULSE remove old " + r.label + " sessions"} />
          </label>
        ) : null}
      </div>
      <div className="mt-1 text-muted-foreground">
        {sessions}
        {r.sessions && r.oldest ? (
          <>
            {" Oldest "}
            <When d={r.oldest} />
            {"."}
          </>
        ) : null}
        {r.removed ? (
          <>
            {" Last cleanup removed " + plural(r.removed, "session") + (r.freed ? " (" + sizeText(r.freed) + " freed)" : "")}
            {r.removed_at ? (
              <>
                {" "}
                <When d={r.removed_at} />
              </>
            ) : null}
            {"."}
          </>
        ) : null}
        {r.note ? " " + r.label + " " + r.note + "." : null}
      </div>
      <div className="mt-1 min-w-0 truncate" title={r.sessions_dir || r.dir}>
        <Mono className="whitespace-nowrap text-xs [overflow-wrap:normal]">{r.sessions_dir || r.dir}</Mono>
      </div>
    </li>
  );
}

// The other agent programs on this machine (ADR-068 companion to the sweep):
// PULSE finds what is installed, asks once whether their old sessions should
// follow the same cleanup, and then the ten-minute sweep looks after them.
// Nothing renders when no other program is here.
export function RunnersSection({ note }: { note: RunnersNote | null | undefined }) {
  const [busy, setBusy] = useState(false);
  if (!note || !note.runners.length) return null;
  const unmanaged = note.runners.filter((r) => r.sweeps && !r.managed);
  const asking = !note.decided && unmanaged.length > 0;
  const act = (value: string) => {
    setBusy(true);
    save(value).finally(() => setBusy(false));
  };
  const managed = note.runners.filter((r) => r.managed).map((r) => r.label);
  return (
    <div className="mt-4 border-t border-border/70 pt-3" aria-label="Other agent programs">
      <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
        <p className="text-[13px] font-medium text-foreground">Other agent programs on this machine</p>
        <p className="text-[13px] text-muted-foreground">
          {note.at ? (
            <>
              {"Checked "}
              <When d={note.at} />
            </>
          ) : (
            "Not checked by the runner yet"
          )}
        </p>
      </div>
      <p className="mt-0.5 max-w-3xl text-[13px] leading-relaxed text-muted-foreground">
        {(managed.length ? "PULSE removes " + andList(managed) + " sessions older than " + plural(note.keep_days, "day") + ", never one that is open. " : "") +
          "Settings, logins and each program's own index stay as they are. " +
          (note.decided ? "" : "Say so below and the cleanup every 10 minutes looks after them too.")}{" "}
        <Link href={hrefs.settings("daemon")} className="underline-offset-2 hover:text-foreground hover:underline">
          Change the window
        </Link>
      </p>
      {asking ? (
        <Notice tone="warn" className="mt-3">
          <p className="font-medium">{"Should PULSE clean up after " + andList(unmanaged.map((r) => r.label)) + " too?"}</p>
          <p className="mt-1 text-[13px] text-muted-foreground">{"Their old sessions pile up like PULSE's did. Saying yes removes sessions untouched for " + plural(note.keep_days, "day") + "; a session a running program holds open is always left alone. You can change this per program at any time."}</p>
          <div className="mt-2 flex flex-wrap gap-2">
            <Button size="sm" disabled={busy} onClick={() => act(sweepable(note).join(","))}>
              Yes, all of them
            </Button>
            <Button size="sm" variant="outline" disabled={busy} onClick={() => act("none")}>
              Not now
            </Button>
          </div>
        </Notice>
      ) : null}
      <ul className="mt-2 grid gap-1.5">
        {note.runners.map((r) => (
          <RunnerRow key={r.id} r={r} note={note} busy={busy} onToggle={(on) => act(toggled(note, r.id, on))} />
        ))}
      </ul>
    </div>
  );
}
