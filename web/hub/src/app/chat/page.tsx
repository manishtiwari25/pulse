"use client";

import { Suspense } from "react";
import { ChatPageClient } from "@/components/chat/chat";
import { SkeletonPage } from "@/components/pulse/panel";

export default function ChatPage() {
  return (
    <Suspense fallback={<SkeletonPage rows={3} />}>
      <ChatPageClient />
    </Suspense>
  );
}
