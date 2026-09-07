"use client";

import { ThemeProvider } from "next-themes";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Toaster } from "@/components/ui/sonner";

export function Providers({ children }: { children: React.ReactNode }) {
  return (
    <ThemeProvider attribute="class" defaultTheme="system" enableSystem disableTransitionOnChange>
      <TooltipProvider delayDuration={200}>{children}</TooltipProvider>
      {/* On phones the bottom nav sits under the toasts; lift them above it. */}
      <Toaster position="bottom-right" closeButton richColors mobileOffset={{ bottom: 72 }} />
    </ThemeProvider>
  );
}
