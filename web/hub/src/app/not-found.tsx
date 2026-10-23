import Link from "next/link";
import { HomeIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { PulseMascot } from "@/components/pulse/mascots";

export default function NotFound() {
  return (
    <div className="mx-auto flex min-h-[60vh] max-w-lg items-center justify-center px-4 py-10 text-center">
      <section className="enter rounded-2xl border bg-card p-8">
        <PulseMascot mood="lost" size={120} className="float mx-auto" title="Pip, looking for the page with a magnifying glass" />
        <h1 className="mt-5 text-2xl font-semibold tracking-tight">There&apos;s no page here.</h1>
        <p className="mt-2 text-[15px] leading-relaxed text-muted-foreground">Pip looked everywhere. The link may be old, or the page moved. Home has everything that waits for you, your projects and what your agents are doing.</p>
        <Button asChild className="mt-6">
          <Link href="/">
            <HomeIcon /> Go to Home
          </Link>
        </Button>
      </section>
    </div>
  );
}
