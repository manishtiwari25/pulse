"use client";

import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { cn } from "@/lib/utils";

// Record bodies, briefs and chat messages are Markdown stored in SQL. Links
// open in the same tab when relative (the hub serves them) and in a new tab
// when absolute; raw HTML is never rendered.
export function Markdown({ source, className }: { source: string; className?: string }) {
  if (!source?.trim()) return <p className={cn("text-sm text-muted-foreground", className)}>No text yet.</p>;
  return (
    <div className={cn("prose-pulse", className)}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        skipHtml
        components={{
          a: ({ href, children }) => {
            const abs = /^https?:\/\//.test(href || "");
            return (
              <a href={href} target={abs ? "_blank" : undefined} rel={abs ? "noreferrer noopener" : undefined}>
                {children}
              </a>
            );
          },
        }}
      >
        {source}
      </ReactMarkdown>
    </div>
  );
}
