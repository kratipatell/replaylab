import { Suspense } from "react";
import AgentDetailView from "@/components/AgentDetailView";

async function AgentPageContent({
  params,
}: {
  params: Promise<{ id: string }>;
}) {
  const { id } = await params;
  return <AgentDetailView id={id} />;
}

function LoadingAgent() {
  return (
    <div className="flex max-w-6xl flex-col gap-3" aria-label="Loading agent">
      <div className="h-8 w-48 animate-pulse rounded bg-[var(--surface)]" />
      <div className="h-28 animate-pulse rounded border border-[var(--border)] bg-[var(--surface)]" />
      <div className="h-64 animate-pulse rounded border border-[var(--border)] bg-[var(--surface)]" />
    </div>
  );
}

export default function Page({ params }: { params: Promise<{ id: string }> }) {
  return (
    <Suspense fallback={<LoadingAgent />}>
      <AgentPageContent params={params} />
    </Suspense>
  );
}
