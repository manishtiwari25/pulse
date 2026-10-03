import * as React from "react"
import { cn } from "@/lib/utils"

function Input({ className, type, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        "h-9 w-full min-w-0 rounded-xl border border-input bg-card/70 px-3 py-1 text-base shadow-[inset_0_1px_2px_oklch(0_0_0/.04),inset_0_1px_0_oklch(1_0_0/.55)] transition-[color,background-color,border-color,box-shadow] duration-150 ease-out-soft outline-none file:inline-flex file:h-6 file:border-0 file:bg-transparent file:text-sm file:font-medium file:text-foreground placeholder:text-muted-foreground hover:bg-card focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:pointer-events-none disabled:cursor-not-allowed disabled:bg-input/50 disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-2 aria-invalid:ring-destructive/20 aria-invalid:focus-visible:ring-destructive md:text-sm dark:bg-card/45 dark:shadow-[inset_0_1px_2px_oklch(0_0_0/.25),inset_0_1px_0_oklch(1_0_0/.08)] dark:disabled:bg-input/80",
        className
      )}
      {...props}
    />
  )
}

export { Input }
