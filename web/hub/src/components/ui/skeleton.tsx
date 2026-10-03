import { cn } from "@/lib/utils"

function Skeleton({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="skeleton"
      className={cn("relative overflow-hidden rounded-xl bg-muted/70 before:absolute before:inset-0 before:-translate-x-full before:bg-[linear-gradient(90deg,transparent,oklch(1_0_0/.28),transparent)] motion-safe:before:animate-[shimmer_1.8s_ease-in-out_infinite] motion-reduce:before:animate-none dark:before:bg-[linear-gradient(90deg,transparent,oklch(1_0_0/.08),transparent)]", className)}
      {...props}
    />
  )
}

export { Skeleton }
