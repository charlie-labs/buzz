import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";

import { ViewLoadingFallback } from "@/shared/ui/ViewLoadingFallback";

export const Route = createFileRoute("/daemons/$daemonId")({
  component: DaemonDetailRoute,
});

const DaemonDetailView = React.lazy(async () => {
  const module = await import("@/features/daemons/ui/DaemonDetailView");
  return { default: module.DaemonDetailView };
});

function DaemonDetailRoute() {
  const { daemonId } = Route.useParams();
  return (
    <React.Suspense fallback={<ViewLoadingFallback kind="agents" />}>
      <DaemonDetailView daemonId={daemonId} />
    </React.Suspense>
  );
}
