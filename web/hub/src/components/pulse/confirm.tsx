"use client";

import { useState } from "react";
import { Loader2Icon } from "lucide-react";
import { toast } from "sonner";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { errorText } from "@/lib/api";
import { invalidate } from "@/lib/use-api";
import { cn } from "@/lib/utils";

// Every owner write goes through here: a confirm dialog, an optional
// "words" field (the human's reason is recorded on the board), an optimistic
// toast, and the error surfaced as what / why / fix.
export interface ConfirmProps {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  title: React.ReactNode;
  description?: React.ReactNode;
  action: string;
  destructive?: boolean;
  words?: { label: string; placeholder?: string; required?: boolean; initial?: string };
  onConfirm: (words: string) => Promise<unknown>;
  success: string;
  invalidates?: string | RegExp | ((p: string) => boolean);
  children?: React.ReactNode;
}

export function showApiError(e: unknown, what = "That did not work") {
  const t = errorText(e);
  toast.error(t.error || what, {
    description: [t.why && "why: " + t.why, t.fix && "fix: " + t.fix].filter(Boolean).join("\n") || undefined,
    duration: 9000,
  });
}

export function ConfirmDialog({ open, onOpenChange, title, description, action, destructive, words, onConfirm, success, invalidates, children }: ConfirmProps) {
  const [text, setText] = useState(words?.initial ?? "");
  const [busy, setBusy] = useState(false);
  const missing = !!words?.required && !text.trim();
  const run = async () => {
    setBusy(true);
    const id = toast.loading(action + "…");
    try {
      await onConfirm(text.trim());
      toast.success(success, { id });
      if (invalidates === undefined) invalidate();
      else invalidate(invalidates);
      onOpenChange(false);
      setText(words?.initial ?? "");
    } catch (e) {
      toast.dismiss(id);
      showApiError(e);
    } finally {
      setBusy(false);
    }
  };
  return (
    <AlertDialog open={open} onOpenChange={(o) => !busy && onOpenChange(o)}>
      <AlertDialogContent size="default" className="rounded-xl sm:max-w-lg">
        <AlertDialogHeader>
          <AlertDialogTitle className="text-base font-semibold tracking-tight">{title}</AlertDialogTitle>
          {description ? <AlertDialogDescription className="leading-relaxed anywhere">{description}</AlertDialogDescription> : null}
        </AlertDialogHeader>
        {children}
        {words ? (
          <div className="grid gap-1.5">
            <Label htmlFor="confirm-words" className="text-sm font-medium">{words.label}</Label>
            <Textarea
              id="confirm-words"
              value={text}
              onChange={(e) => setText(e.target.value)}
              placeholder={words.placeholder}
              rows={3}
              autoFocus
              onKeyDown={(e) => {
                if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && !missing && !busy) void run();
              }}
            />
            <p className="text-xs text-muted-foreground">{words.required ? "Required - your words are recorded with the decision." : "Optional - recorded with the decision."} ⌘↩ confirms.</p>
          </div>
        ) : null}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={busy}>Cancel</AlertDialogCancel>
          <AlertDialogAction disabled={busy || missing} onClick={(e) => { e.preventDefault(); void run(); }} className={cn(destructive && "bg-destructive text-white shadow-xs hover:bg-destructive/90 focus-visible:ring-destructive/30")}>
            {busy ? <Loader2Icon className="animate-spin" /> : null}
            {action}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
