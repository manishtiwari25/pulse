import * as React from "react"
import { cn } from "@/lib/utils"

function Textarea({ className, ...props }: React.ComponentProps<"textarea">) {
  return (
    <textarea
      data-slot="textarea"
      className={cn(
        "anywhere flex field-sizing-content min-h-20 w-full min-w-0 rounded-lg border border-input bg-card px-3 py-2 text-base leading-relaxed transition-[color,background-color,border-color,box-shadow] duration-150 ease-out-soft outline-none placeholder:text-muted-foreground hover:border-[color-mix(in_oklch,var(--foreground)_22%,var(--input))] focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/25 disabled:cursor-not-allowed disabled:bg-muted disabled:opacity-60 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 md:text-[15px] dark:bg-background/40",
        className
      )}
      {...props}
    />
  )
}

export { Textarea }
