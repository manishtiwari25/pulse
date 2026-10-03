"use client";

import Link from "next/link";
import { Suspense, useCallback } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { FileDiffIcon } from "lucide-react";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { CodeDeliveriesReport } from "@/components/code-delivery/report";
import { PageTitle, SkeletonPage } from "@/components/pulse/panel";
import { hrefs } from "@/lib/nav";
import { registeredProjects, type Org } from "@/lib/api";
import { useApi } from "@/lib/use-api";

export default function CodeDeliveriesPage() {
  return (
    <Suspense fallback={<SkeletonPage rows={3} />}>
      <CodeDeliveriesScreen />
    </Suspense>
  );
}

function CodeDeliveriesScreen() {
  const sp = useSearchParams();
  const router = useRouter();
  const pathname = usePathname();
  const project = sp.get("project") || "";
  const org = useApi<Org>("org", { refresh: 60000 });
  const projects = registeredProjects(org.data);
  const setProject = useCallback(
    (slug: string) => {
      const q = new URLSearchParams(sp.toString());
      if (slug) q.set("project", slug);
      else q.delete("project");
      const s = q.toString();
      router.push(pathname + (s ? "?" + s : ""), { scroll: false });
    },
    [sp, router, pathname],
  );
  return (
    <>
      <PageTitle
        icon={FileDiffIcon}
        crumb={
          <>
            <Link href={hrefs.home()} className="hover:underline">
              Home
            </Link>{" "}
            / code delivered
          </>
        }
        sub="What the agents shipped to your projects, week by week."
        bar={
          <div className="flex w-max items-center gap-2 md:w-auto md:flex-wrap">
            <Select value={project || "__all"} onValueChange={(v) => setProject(v === "__all" ? "" : v)}>
              <SelectTrigger size="sm" aria-label="Project" className="w-44">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="__all">all projects</SelectItem>
                {projects.map((p) => (
                  <SelectItem key={p.slug} value={p.slug}>
                    {p.name || p.slug}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {org.error ? <span role="alert" className="text-xs text-bad">Project filters unavailable: {org.error}</span> : null}
          </div>
        }
      >
        Code delivered
      </PageTitle>
      <CodeDeliveriesReport key={project || "__all"} project={project || undefined} />
    </>
  );
}
