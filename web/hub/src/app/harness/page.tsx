"use client";

import { Suspense } from "react";
import { HarnessPageClient } from "@/components/harness/harness";
import { SkeletonPage } from "@/components/pulse/panel";

export default function HarnessPage() {
  return (
    <Suspense fallback={<SkeletonPage rows={3} />}>
      <HarnessPageClient />
    </Suspense>
  );
}
