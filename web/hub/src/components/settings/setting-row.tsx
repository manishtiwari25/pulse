"use client";

import { useState } from "react";
import { EraserIcon, EyeOffIcon, Loader2Icon, RotateCcwIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { ConfirmDialog } from "@/components/pulse/confirm";
import { Mono, ToneBadge, When } from "@/components/pulse/who";
import { cn } from "@/lib/utils";
import { writes, type Setting } from "@/lib/api";

export function hint(s: Setting): string {
  if (s.secret) return "owner-private: stored, but never shown here, to agents or in events";
  switch (s.kind) {
    case "count":
      return "a whole number" + (s.min ? ", at least " + s.min : "");
    case "count-or-none":
      return "a whole number, or none";
    case "credits":
      return "a whole number, at least " + (s.min ?? 30);
    case "fraction":
      return "a fraction such as 2/3";
    case "command":
      return "a program name or path";
    case "identifier":
      return "a name, id or path - never a secret";
    default:
      return "";
  }
}

// One setting row: name + description, the editor for its kind, and the
// default / last change. Saving is confirmed, then POST /api/v1/settings.
export function SettingRow({ s, canEdit, labels }: { s: Setting; canEdit: boolean; labels?: { on: string; off: string } }) {
  const [draft, setDraft] = useState(s.value);
  const [pending, setPending] = useState<string | null>(null);
  const [busy] = useState(false);
  const dirty = draft !== s.value;
  const isSwitch = s.kind === "switch";
  const isChoice = s.kind === "choice";
  const isOn = s.value === "1";
  const secret = !!s.secret;
  const inputClass = "h-8 tnum";
  const wide = s.kind === "command" || s.kind === "text" || s.kind === "identifier";
  return (
    <div className="grid gap-1.5 border-t py-3 first:border-t-0 md:grid-cols-[minmax(200px,1fr)_minmax(280px,2fr)_minmax(200px,1fr)] md:items-start md:gap-4">
      <div className="min-w-0">
        <div className="flex flex-wrap items-center gap-1.5">
          <Mono className="text-[13px] font-semibold">{s.name}</Mono>
          {secret ? (
            <ToneBadge tone="warn" className="gap-1">
              <EyeOffIcon className="size-3" /> secret
            </ToneBadge>
          ) : null}
        </div>
        <p className="mt-0.5 text-[12.5px] text-muted-foreground anywhere">{s.description}</p>
      </div>
      <div className="min-w-0">
        {isSwitch ? (
          <div className="flex items-center gap-3">
            <Switch id={"sw-" + s.name} checked={isOn} disabled={!canEdit || busy} onCheckedChange={(v) => setPending(v ? "1" : "0")} aria-label={s.name} />
            <label htmlFor={"sw-" + s.name} className={cn("text-[13px]", isOn ? (s.name === "stop" ? "text-bad" : "text-ok") : "text-muted-foreground")}>
              {isOn ? labels?.on ?? "on" : labels?.off ?? "off"}
            </label>
          </div>
        ) : (
          <div className="flex flex-wrap items-center gap-2">
            {isChoice ? (
              <Select value={draft} onValueChange={setDraft} disabled={!canEdit || busy}>
                <SelectTrigger aria-label={s.name} className="h-8 w-44">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {(s.choices || []).map((c) => (
                    <SelectItem key={c} value={c}>
                      {c}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            ) : (
              <Input
                type={secret ? "password" : "text"}
                inputMode={s.kind === "count" || s.kind === "credits" ? "numeric" : undefined}
                aria-label={s.name}
                value={secret && !dirty ? "" : draft}
                disabled={!canEdit || busy}
                placeholder={secret ? (s.value ? "(set - value never shown)" : "(not set)") : s.default || "empty"}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && dirty && canEdit) setPending(draft);
                  if (e.key === "Escape") setDraft(s.value);
                }}
                className={cn(inputClass, wide ? "min-w-[260px] flex-1" : "w-32")}
              />
            )}
            <Button size="sm" disabled={!canEdit || busy || !dirty} onClick={() => setPending(draft)}>
              {busy ? <Loader2Icon className="animate-spin" /> : null}
              Save
            </Button>
            {dirty && !busy ? (
              <Button size="sm" variant="ghost" onClick={() => setDraft(s.value)} aria-label="Undo">
                <RotateCcwIcon /> Undo
              </Button>
            ) : null}
            {secret && s.value && !dirty && !busy && canEdit ? (
              <Button size="sm" variant="ghost" onClick={() => setPending("")} aria-label={"Clear " + s.name}>
                <EraserIcon /> Clear
              </Button>
            ) : null}
          </div>
        )}
        {hint(s) ? <p className="mt-1 text-[12px] text-muted-foreground">{hint(s)}</p> : null}
      </div>
      <div className="text-[12.5px] text-muted-foreground anywhere">
        <div>
          {"default "}
          <Mono>{s.default || "(empty)"}</Mono>
        </div>
        {s.updated_by === "default" || !s.updated_at ? (
          <div>not set yet - the default is in effect</div>
        ) : (
          <div>
            {"changed "}
            <When d={s.updated_at} />
            {s.updated_by ? " by " : ""}
            {s.updated_by ? <Mono>{s.updated_by}</Mono> : null}
          </div>
        )}
      </div>
      <ConfirmDialog
        open={pending !== null}
        onOpenChange={(o) => !o && setPending(null)}
        title={"Change " + s.name + "?"}
        description={
          <span>
            {isSwitch ? (pending === "1" ? "Turn it on" : "Turn it off") : secret ? (
              pending ? (s.value ? "Replace the private value (never shown here)" : "Set the private value (never shown here)") : "Clear the private value"
            ) : <>
              from <Mono>{s.value || "(empty)"}</Mono> to <Mono>{pending || "(empty)"}</Mono>
            </>}
            . Saved to the store at once, recorded with who made it; the daemon reads it on its next loop.
          </span>
        }
        action="Save"
        success={s.name + " saved."}
        onConfirm={() => writes.setSetting(s.name, pending ?? "")}
        invalidates={(p) => p.startsWith("settings") || p.startsWith("runner") || p.startsWith("compute") || p.startsWith("events") || p.startsWith("org")}
      />
    </div>
  );
}
