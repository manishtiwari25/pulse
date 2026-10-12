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
  // Mounted only while its panel is open (see PanelSlot), so a closed dialog
  // that merely renders on the page never claims the slot.
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

// Rendered inside the Radix content, which exists only while the panel is
// open (or animating out). Claiming the slot from the content wrapper itself
// ran for every closed dialog on the page too, and the first one to render
// after an open sheet - a card's Cancel confirm, say - closed that sheet:
// a sheet opened from a link (?id=, ?task=) shut as soon as the list loaded.
export function PanelSlot({ nested }: { nested: boolean }) {
  usePanelSlot(React.useContext(PanelClose), nested)
  return null
}

// Focus goes back where it came from. Most panels open from state, not from a
// Radix Trigger, so Radix has no trigger to return focus to and it fell to
// <body>. The opener is whatever had focus when the panel opened; `self`
// focuses the panel itself on open (a reading sheet), so no ring lands on its
// close button. Caller handlers still run first and may preventDefault.
export function useFocusReturn(onOpenAutoFocus?: (e: Event) => void, onCloseAutoFocus?: (e: Event) => void, self = false) {
  const opener = React.useRef<HTMLElement | null>(null)
  return {
    onOpenAutoFocus: (e: Event) => {
      const active = document.activeElement
      opener.current = active instanceof HTMLElement && active !== document.body ? active : null
      onOpenAutoFocus?.(e)
      if (self && !e.defaultPrevented) {
        const panel = (e.currentTarget ?? e.target) as HTMLElement | null
        if (panel && typeof panel.focus === "function") {
          e.preventDefault()
          panel.focus({ preventScroll: true })
        }
      }
    },
    onCloseAutoFocus: (e: Event) => {
      onCloseAutoFocus?.(e)
      if (e.defaultPrevented) return
      const el = opener.current
      opener.current = null
      if (el && el.isConnected) {
        e.preventDefault()
        el.focus({ preventScroll: true })
      }
    },
  }
}

// The frame every panel shares: docked under the header at the right edge,
// its own scrollbar, no overlay.
// Under the phone header; the desktop has no header, so panels start near the top.
export const panelTop = "top-[calc(3.5rem+1px+env(safe-area-inset-top))] lg:top-3"
export const panelFrame =
  "fixed z-50 border bg-popover bg-clip-padding text-sm text-popover-foreground shadow-pop outline-none " +
  "data-open:animate-in data-open:fade-in-0 data-open:slide-in-from-right-4 data-closed:animate-out data-closed:fade-out-0 data-closed:slide-out-to-right-4 duration-150"
