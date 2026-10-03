import Link from "next/link";
import { SearchXIcon } from "lucide-react";
import { Button } from "@/components/ui/button";

export default function NotFound() {
  return (
    <div className="mx-auto flex min-h-[60vh] max-w-lg items-center justify-center px-4 py-10 text-center">
      <section className="surface-raised rounded-2xl border p-8 shadow-pop">
        <span className="mx-auto inline-flex size-12 items-center justify-center rounded-2xl bg-primary/10 text-primary">
          <SearchXIcon className="size-5" />
        </span>
        <h1 className="mt-5 text-2xl font-semibold tracking-tight">That page isn&apos;t here.</h1>
        <p className="mt-2 text-sm leading-6 text-muted-foreground">Try Home to find the owner inbox, projects, records and tools.</p>
        <Button asChild className="glossy mt-6 rounded-xl">
          <Link href="/">Back home</Link>
        </Button>
      </section>
    </div>
  );
}
