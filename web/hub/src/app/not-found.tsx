import Link from "next/link";
import { Button } from "@/components/ui/button";

export default function NotFound() {
  return (
    <div className="mx-auto mt-16 max-w-md text-center">
      <p className="text-5xl font-semibold tracking-tight">404</p>
      <h1 className="mt-2 text-lg font-medium">No such page</h1>
      <p className="mt-1 text-sm text-muted-foreground">Every screen lives under Hub, Tasks, Records, Compute, Agents or Settings; a project opens from the switcher or with ⌘K.</p>
      <Button asChild className="mt-4">
        <Link href="/">Back to the hub</Link>
      </Button>
    </div>
  );
}
