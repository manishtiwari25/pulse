"use client";

import { ThemeProvider } from "next-themes";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Toaster } from "@/components/ui/sonner";
import { HeldoutHost } from "@/components/pulse/heldout";

export function Providers({ children }: { children: React.ReactNode }) {
  return (
    <ThemeProvider attribute="class" defaultTheme="system" enableSystem disableTransitionOnChange>
      <TooltipProvider delayDuration={200}>
        {children}
        {/* The held-out fixtures dialog (ADR-032) lives here, not on the gate card that opens it: the card leaves the inbox before the dialog is done. */}
        <HeldoutHost />
      </TooltipProvider>
      {/* On phones the bottom nav sits under the toasts; lift them above it. */}
      <Toaster position="bottom-right" closeButton richColors mobileOffset={{ bottom: 72 }} />
    </ThemeProvider>
  );
}
