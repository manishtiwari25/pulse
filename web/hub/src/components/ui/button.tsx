import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { cn } from "@/lib/utils"
import { Slot } from "radix-ui"

const buttonVariants = cva(
  "group/button inline-flex shrink-0 items-center justify-center rounded-xl border border-transparent bg-clip-padding text-sm font-medium whitespace-nowrap transition-[color,background-color,border-color,box-shadow,transform] duration-200 ease-out-soft outline-none select-none focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background motion-safe:active:not-aria-[haspopup]:translate-y-px disabled:pointer-events-none disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 aria-invalid:focus-visible:ring-destructive [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default:
          "glossy bg-[linear-gradient(135deg,var(--primary-strong),color-mix(in_oklch,var(--brand-sky)_55%,var(--primary)))] text-white shadow-[inset_0_1px_0_oklch(1_0_0/.32),0_8px_20px_-10px_color-mix(in_oklch,var(--primary)_55%,transparent)] hover:-translate-y-0.5 hover:shadow-[inset_0_1px_0_oklch(1_0_0/.4),0_14px_28px_-14px_color-mix(in_oklch,var(--primary)_70%,transparent)] active:translate-y-0 active:scale-[.995] motion-reduce:hover:translate-y-0",
        outline:
          "surface-glass ring-soft border-transparent bg-card/70 shadow-xs hover:bg-muted/70 hover:text-foreground hover:shadow-card aria-expanded:bg-muted aria-expanded:text-foreground dark:bg-card/45 dark:hover:bg-muted/60",
        secondary:
          "surface-glass ring-soft border-transparent bg-secondary/70 text-secondary-foreground shadow-xs hover:bg-[color-mix(in_oklch,var(--secondary),var(--foreground)_5%)] hover:shadow-card aria-expanded:bg-secondary aria-expanded:text-secondary-foreground",
        ghost:
          "hover:bg-muted hover:text-foreground aria-expanded:bg-muted aria-expanded:text-foreground",
        destructive:
          "glossy border-destructive/20 bg-[linear-gradient(135deg,color-mix(in_oklch,var(--destructive)_92%,black),var(--destructive))] text-white shadow-[inset_0_1px_0_oklch(1_0_0/.28),0_8px_20px_-12px_color-mix(in_oklch,var(--destructive)_60%,transparent)] hover:-translate-y-0.5 hover:shadow-[inset_0_1px_0_oklch(1_0_0/.36),0_14px_28px_-14px_color-mix(in_oklch,var(--destructive)_70%,transparent)] active:translate-y-0 active:scale-[.995] focus-visible:border-destructive focus-visible:ring-destructive motion-reduce:hover:translate-y-0",
        link: "text-primary underline-offset-4 hover:underline",
      },
      size: {
        default:
          "h-9 gap-2 px-3 has-data-[icon=inline-end]:pr-2.5 has-data-[icon=inline-start]:pl-2.5",
        xs: "h-6 gap-1 rounded-[min(var(--radius-md),10px)] px-2 text-xs in-data-[slot=button-group]:rounded-lg has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 [&_svg:not([class*='size-'])]:size-3",
        sm: "h-7 gap-1 rounded-[min(var(--radius-md),12px)] px-2.5 text-[0.8rem] in-data-[slot=button-group]:rounded-lg has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 [&_svg:not([class*='size-'])]:size-3.5",
        lg: "h-10 gap-2 px-4 has-data-[icon=inline-end]:pr-3 has-data-[icon=inline-start]:pl-3",
        icon: "size-9",
        "icon-xs":
          "size-6 rounded-[min(var(--radius-md),10px)] in-data-[slot=button-group]:rounded-lg [&_svg:not([class*='size-'])]:size-3",
        "icon-sm":
          "size-7 rounded-[min(var(--radius-md),12px)] in-data-[slot=button-group]:rounded-lg",
        "icon-lg": "size-10",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
)

function Button({
  className,
  variant = "default",
  size = "default",
  asChild = false,
  ...props
}: React.ComponentProps<"button"> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean
  }) {
  const Comp = asChild ? Slot.Root : "button"

  return (
    <Comp
      data-slot="button"
      data-variant={variant}
      data-size={size}
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  )
}

export { Button, buttonVariants }
