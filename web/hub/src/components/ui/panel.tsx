"use client"

import * as React from "react"

// Popups in the console are panels, not modals: nothing dims or locks the
// page behind them, the page stays readable and clickable, and a panel closes
// with its own controls, Escape or - for read-only detail panels - a click on
// the page. Two rules keep that from turning into a pile of cards at the
// right edge: one top-level panel is open at a time (opening another closes
// the first), and a panel opened from inside a panel stacks on top of it.

export const PanelDepth = React.createContext(0)

// The nearest Dialog/Sheet root's `onOpenChange(false)`; null when the root
// is uncontrolled and cannot be closed from outside.
export const PanelClose = React.createContext<(() => void) | null>(null)

interface Slot {
  token: object
  close: () => void
}

let current: Slot | null = null

export function usePanelSlot(close: (() => void) | null, nested: boolean) {
  const token = React.useRef<object>({})
  const closeRef = React.useRef(close)
  // The latest close function, so the slot never calls a stale one.
  React.useEffect(() => {
    closeRef.current = close
  }, [close])
  React.useEffect(() => {
    if (nested) return
    const me = token.current
    if (current && current.token !== me) current.close()
    current = { token: me, close: () => closeRef.current?.() }
    return () => {
      if (current?.token === me) current = null
    }
  }, [nested])
}

// The frame every panel shares: docked under the header at the right edge,
// its own scrollbar, no overlay.
export const panelTop = "top-[calc(4rem+1px)]"
export const panelFrame =
  "fixed z-50 border bg-popover bg-clip-padding text-sm text-popover-foreground shadow-pop outline-none " +
  "data-open:animate-in data-open:fade-in-0 data-open:slide-in-from-right-4 data-closed:animate-out data-closed:fade-out-0 data-closed:slide-out-to-right-4 duration-150"
