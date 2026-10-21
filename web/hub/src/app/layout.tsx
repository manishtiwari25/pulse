import type { Metadata, Viewport } from "next";
import "./globals.css";
import { AppShell } from "@/components/shell/app-shell";
import { Providers } from "@/components/shell/providers";

export const metadata: Metadata = {
  title: { default: "PULSE", template: "%s · PULSE" },
  description: "Your console for PULSE: what waits for your answer, the tasks you gave, and what your agents are doing on every project on this machine.",
  applicationName: "PULSE hub",
};

export const viewport: Viewport = {
  width: "device-width",
  initialScale: 1,
  viewportFit: "cover",
  themeColor: [
    { media: "(prefers-color-scheme: light)", color: "#f3f7f6" },
    { media: "(prefers-color-scheme: dark)", color: "#0f1726" },
  ],
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning className="h-full scroll-smooth antialiased">
      <head>
        {/* Atkinson Hyperlegible is vendored in public/fonts (OFL) so the export needs no network at build or run time. */}
        <link rel="preload" href="/fonts/atkinson-next-latin.woff2" as="font" type="font/woff2" crossOrigin="anonymous" />
      </head>
      <body className="flex min-h-full flex-col bg-background text-foreground">
        <Providers>
          <AppShell>{children}</AppShell>
        </Providers>
      </body>
    </html>
  );
}
