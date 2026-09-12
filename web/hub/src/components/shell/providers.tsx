"use client";

import { ThemeProvider } from "next-themes";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Toaster } from "@/components/ui/sonner";
import { HeldoutHost } from "@/components/pulse/heldout";

const toastOffset = { bottom: "var(--pulse-toast-bottom)" };

export function Providers({ children }: { children: React.ReactNode }) {
  return (
    <ThemeProvider attribute="class" defaultTheme="system" enableSystem disableTransitionOnChange>
      <TooltipProvider delayDuration={200}>
        {children}
        {/* The held-out fixtures dialog (ADR-032) lives here, not on the gate card that opens it: the card leaves the inbox before the dialog is done. */}
        <HeldoutHost />
      </TooltipProvider>
      {/* The compact nav stays visible through tablet widths, not just phones. */}
      <Toaster
        position="bottom-right"
        closeButton
        richColors
        className="toaster group [--pulse-toast-bottom:calc(5rem+env(safe-area-inset-bottom))] lg:[--pulse-toast-bottom:2rem]"
        offset={toastOffset}
        mobileOffset={toastOffset}
      />
    </ThemeProvider>
  );
}
