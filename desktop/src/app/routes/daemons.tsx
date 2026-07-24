import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";

import { ViewLoadingFallback } from "@/shared/ui/ViewLoadingFallback";

export const Route = createFileRoute("/daemons")({
  component: DaemonsRoute,
});

const DaemonsView = React.lazy(async () => {
  const module = await import("@/features/daemons/ui/DaemonsView");
  return { default: module.DaemonsView };
});

function DaemonsRoute() {
  return (
    <React.Suspense fallback={<ViewLoadingFallback kind="agents" />}>
      <DaemonsView />
    </React.Suspense>
  );
}
