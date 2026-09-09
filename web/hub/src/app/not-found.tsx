import Link from "next/link";
import { SearchXIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { hrefs } from "@/lib/nav";

export default function NotFound() {
  return (
    <div className="mx-auto flex min-h-[60vh] max-w-md flex-col items-center justify-center px-4 text-center">
      <span className="inline-flex size-12 items-center justify-center rounded-full bg-primary/10 text-primary">
        <SearchXIcon className="size-5" />
      </span>
      <h1 className="mt-4 text-lg font-semibold tracking-tight">This page does not exist</h1>
      <p className="mt-1 text-sm text-muted-foreground">Every screen lives under Hub, Tasks, Votes, Records, Compute, Agents or Settings; a project opens from the switcher or with ⌘K.</p>
      <div className="mt-5 flex flex-wrap items-center justify-center gap-2">
        <Button asChild>
          <Link href="/">Back to the hub</Link>
        </Button>
        <Button asChild variant="outline">
          <Link href={hrefs.tasks()}>Open tasks</Link>
        </Button>
      </div>
    </div>
  );
}
