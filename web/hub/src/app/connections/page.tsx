"use client";

import { Suspense } from "react";
import { ConnectionsPageClient } from "@/components/connections/connections";
import { SkeletonPage } from "@/components/pulse/panel";

export default function ConnectionsPage() {
  return (
    <Suspense fallback={<SkeletonPage rows={3} />}>
      <ConnectionsPageClient />
    </Suspense>
  );
}
