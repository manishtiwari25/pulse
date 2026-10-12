"use client"

import * as React from "react"
import { cn } from "@/lib/utils"
import { Dialog as DialogPrimitive } from "radix-ui"

import { Button } from "@/components/ui/button"
import { PanelClose, PanelDepth, PanelSlot, panelFrame, panelTop, useFocusReturn } from "@/components/ui/panel"
import { XIcon } from "lucide-react"

// A dialog is a card docked at the top right, under the header. It is not
// modal: the page behind stays as it was - readable, scrollable, clickable -
// and a form or a decision stays open while you look around, until Cancel,
// the X, Escape or the action itself closes it (`stay`); a palette closes on
// a click outside (`stay={false}`).
function Dialog({
  modal = false,
  onOpenChange,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Root>) {
  const close = React.useMemo(() => (onOpenChange ? () => onOpenChange(false) : null), [onOpenChange])
  return (
    <PanelClose.Provider value={close}>
      <DialogPrimitive.Root data-slot="dialog" modal={modal} onOpenChange={onOpenChange} {...props} />
    </PanelClose.Provider>
  )
}

function DialogTrigger({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Trigger>) {
  return <DialogPrimitive.Trigger data-slot="dialog-trigger" {...props} />
}

function DialogPortal({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Portal>) {
  return <DialogPrimitive.Portal data-slot="dialog-portal" {...props} />
}

function DialogClose({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Close>) {
  return <DialogPrimitive.Close data-slot="dialog-close" {...props} />
}

function DialogContent({
  className,
  children,
  showCloseButton = true,
  placement = "panel",
  stay = true,
  onInteractOutside,
  onOpenAutoFocus,
  onCloseAutoFocus,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Content> & {
  showCloseButton?: boolean
  // panel: docked top right under the header; float: centred near the top (the command palette).
  placement?: "panel" | "float"
  // Keep the dialog open when the page behind it is clicked or focused.
  stay?: boolean
}) {
  const depth = React.useContext(PanelDepth)
  const focus = useFocusReturn(onOpenAutoFocus, onCloseAutoFocus)
  return (
    <DialogPortal>
      <PanelDepth.Provider value={depth + 1}>
        <DialogPrimitive.Content
          data-slot="dialog-content"
          data-placement={placement}
          onOpenAutoFocus={focus.onOpenAutoFocus}
          onCloseAutoFocus={focus.onCloseAutoFocus}
          onInteractOutside={(e) => {
            onInteractOutside?.(e)
            if (stay) e.preventDefault()
          }}
          className={cn(
            panelFrame,
            "scroll-thin grid gap-4 overflow-y-auto rounded-2xl p-5",
            placement === "panel" &&
              cn(panelTop, "inset-x-2 max-h-[calc(100dvh-3.5rem-1px-1rem)] sm:inset-x-auto sm:right-3 sm:w-full sm:max-w-md lg:max-h-[calc(100dvh-1.5rem)]"),
            placement === "float" &&
              "top-[10dvh] left-1/2 w-[calc(100%-1rem)] max-h-[80dvh] -translate-x-1/2 data-open:slide-in-from-right-0 data-closed:slide-out-to-right-0 data-open:zoom-in-95 data-closed:zoom-out-95 sm:max-w-lg",
            className
          )}
          {...props}
        >
          <PanelSlot nested={depth > 0} />
          {children}
          {showCloseButton && (
            <DialogPrimitive.Close data-slot="dialog-close" asChild>
              <Button
                variant="ghost"
                className="absolute top-2 right-2"
                size="icon-sm"
              >
                <XIcon
                />
                <span className="sr-only">Close</span>
              </Button>
            </DialogPrimitive.Close>
          )}
        </DialogPrimitive.Content>
      </PanelDepth.Provider>
    </DialogPortal>
  )
}

function DialogHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-header"
      className={cn("flex flex-col gap-2 pr-8", className)}
      {...props}
    />
  )
}

function DialogFooter({
  className,
  showCloseButton = false,
  children,
  ...props
}: React.ComponentProps<"div"> & {
  showCloseButton?: boolean
}) {
  return (
    <div
      data-slot="dialog-footer"
      className={cn(
        "-mx-5 -mb-5 flex flex-col-reverse gap-2 rounded-b-2xl border-t bg-muted/50 px-5 py-4 sm:flex-row sm:justify-end",
        className
      )}
      {...props}
    >
      {children}
      {showCloseButton && (
        <DialogPrimitive.Close asChild>
          <Button variant="outline">Close</Button>
        </DialogPrimitive.Close>
      )}
    </div>
  )
}

function DialogTitle({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Title>) {
  return (
    <DialogPrimitive.Title
      data-slot="dialog-title"
      className={cn(
        "font-heading text-lg leading-snug font-semibold tracking-tight",
        className
      )}
      {...props}
    />
  )
}

function DialogDescription({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Description>) {
  return (
    <DialogPrimitive.Description
      data-slot="dialog-description"
      className={cn(
        "text-sm leading-relaxed text-muted-foreground *:[a]:underline *:[a]:underline-offset-3 *:[a]:hover:text-foreground",
        className
      )}
      {...props}
    />
  )
}

export {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogPortal,
  DialogTitle,
  DialogTrigger,
}
