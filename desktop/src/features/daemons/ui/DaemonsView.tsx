import * as React from "react";
import { Clock3, FolderOpen, Import, Plus, Search } from "lucide-react";
import { toast } from "sonner";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import {
  useDaemonBindingsQuery,
  useDaemonPackagesQuery,
  useImportDaemonMutation,
} from "@/features/daemons/hooks";
import {
  activationModeLabel,
  formatDaemonActionError,
} from "@/features/daemons/lib/presentation";
import { openDaemonLibraryFolder } from "@/shared/api/tauriDaemons";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";
import { PageHeader } from "@/shared/ui/PageHeader";
import { DaemonCreateDialog } from "./DaemonCreateDialog";

export function DaemonsView() {
  const { goDaemon } = useAppNavigation();
  const packagesQuery = useDaemonPackagesQuery();
  const bindingsQuery = useDaemonBindingsQuery();
  const importFile = useImportDaemonMutation("file");
  const importFolder = useImportDaemonMutation("folder");
  const [createOpen, setCreateOpen] = React.useState(false);
  const [search, setSearch] = React.useState("");
  const [openingFolder, setOpeningFolder] = React.useState(false);
  const packages = packagesQuery.data ?? [];
  const bindings = bindingsQuery.data ?? [];
  const filtered = packages.filter((item) =>
    `${item.id} ${item.purpose}`
      .toLowerCase()
      .includes(search.trim().toLowerCase()),
  );

  async function handleImport(kind: "file" | "folder", replace = false) {
    const mutation = kind === "file" ? importFile : importFolder;
    try {
      const imported = await mutation.mutateAsync(replace);
      if (!imported) return;
      toast.success(`${replace ? "Replaced" : "Imported"} ${imported.id}`);
      void goDaemon(imported.id);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (!replace && /already exists|retry with replace/i.test(message)) {
        toast.error(message, {
          action: {
            label: "Replace",
            onClick: () => void handleImport(kind, true),
          },
          duration: 10_000,
        });
        return;
      }
      toast.error(message);
    }
  }

  async function handleOpenFolder() {
    if (openingFolder) return;
    setOpeningFolder(true);
    try {
      await openDaemonLibraryFolder();
    } catch (error) {
      toast.error(formatDaemonActionError("Open daemon folder", error));
    } finally {
      setOpeningFolder(false);
    }
  }

  if (packagesQuery.isLoading) {
    return (
      <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
        Loading daemon library…
      </div>
    );
  }

  return (
    <div
      className="flex min-h-0 flex-1 flex-col overflow-y-auto"
      data-testid="daemons-view"
    >
      <div className="mx-auto flex w-full max-w-6xl flex-col gap-6 px-6 py-8">
        <PageHeader
          action={
            <div className="flex flex-wrap justify-end gap-2">
              <Button onClick={() => setCreateOpen(true)} size="sm">
                <Plus className="h-4 w-4" />
                New daemon
              </Button>
              <Button
                disabled={importFile.isPending}
                onClick={() => void handleImport("file")}
                size="sm"
                variant="outline"
              >
                <Import className="h-4 w-4" />
                Import DAEMON.md
              </Button>
              <Button
                disabled={importFolder.isPending}
                onClick={() => void handleImport("folder")}
                size="sm"
                variant="outline"
              >
                <FolderOpen className="h-4 w-4" />
                Import folder
              </Button>
            </div>
          }
          description="Create, import, set up, and run portable daemon packages."
          title="Daemons"
        />

        {packagesQuery.error instanceof Error ? (
          <div
            className="rounded-xl border border-destructive/30 bg-destructive/10 p-4 text-sm text-destructive"
            role="alert"
          >
            <p className="font-medium">Couldn’t load the daemon library.</p>
            <p className="mt-1">{packagesQuery.error.message}</p>
            <Button
              className="mt-3"
              onClick={() => void packagesQuery.refetch()}
              size="sm"
              variant="outline"
            >
              Retry
            </Button>
          </div>
        ) : packages.length === 0 ? (
          <div className="rounded-2xl border border-dashed border-border bg-card/40 px-6 py-12 text-center">
            <Clock3 className="mx-auto h-10 w-10 text-muted-foreground" />
            <h2 className="mt-4 text-lg font-semibold">No daemons yet</h2>
            <p className="mx-auto mt-2 max-w-lg text-sm text-muted-foreground">
              Create a guided first daemon or import a portable DAEMON.md
              package. Files stay on this computer.
            </p>
            <div className="mt-5 flex flex-wrap justify-center gap-2">
              <Button onClick={() => setCreateOpen(true)}>
                <Plus className="h-4 w-4" />
                New daemon
              </Button>
              <Button
                disabled={importFile.isPending}
                onClick={() => void handleImport("file")}
                variant="outline"
              >
                Import DAEMON.md
              </Button>
              <Button
                disabled={importFolder.isPending}
                onClick={() => void handleImport("folder")}
                variant="outline"
              >
                Import folder
              </Button>
              <Button
                disabled={openingFolder}
                onClick={() => void handleOpenFolder()}
                variant="ghost"
              >
                {openingFolder ? "Opening…" : "Open daemon folder"}
              </Button>
            </div>
          </div>
        ) : (
          <>
            <div className="flex items-center gap-2">
              <div className="relative max-w-md flex-1">
                <Search className="pointer-events-none absolute left-3 top-2.5 h-4 w-4 text-muted-foreground" />
                <Input
                  aria-label="Search daemons"
                  className="pl-9"
                  onChange={(event) => setSearch(event.target.value)}
                  placeholder="Search daemons"
                  value={search}
                />
              </div>
              <Button
                disabled={openingFolder}
                onClick={() => void handleOpenFolder()}
                size="sm"
                variant="ghost"
              >
                {openingFolder ? "Opening…" : "Open folder"}
              </Button>
            </div>
            <div className="grid gap-3 md:grid-cols-2">
              {filtered.map((daemon) => {
                const binding = bindings.find(
                  (item) => item.daemonId === daemon.id,
                );
                return (
                  <button
                    className="rounded-2xl border border-border bg-card p-5 text-left transition-colors hover:border-primary/40 hover:bg-accent/30 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
                    data-testid={`daemon-card-${daemon.id}`}
                    key={daemon.id}
                    onClick={() => void goDaemon(daemon.id)}
                    type="button"
                  >
                    <div className="flex items-start justify-between gap-3">
                      <div>
                        <h2 className="font-semibold">{daemon.id}</h2>
                        <p className="mt-1 line-clamp-2 text-sm text-muted-foreground">
                          {daemon.purpose}
                        </p>
                      </div>
                      <Badge variant="secondary">
                        {activationModeLabel(daemon.activationMode)}
                      </Badge>
                    </div>
                    <div className="mt-4 flex flex-wrap gap-2 text-xs text-muted-foreground">
                      <span>
                        {daemon.routineCount} routine
                        {daemon.routineCount === 1 ? "" : "s"}
                      </span>
                      <span>·</span>
                      <span>
                        {binding
                          ? binding.scheduleEnabled
                            ? "Set up and enabled"
                            : "Set up"
                          : "Setup needed"}
                      </span>
                      {daemon.hasScripts ? (
                        <>
                          <span>·</span>
                          <span>Scripts</span>
                        </>
                      ) : null}
                      {daemon.hasReferences ? (
                        <>
                          <span>·</span>
                          <span>References</span>
                        </>
                      ) : null}
                    </div>
                  </button>
                );
              })}
            </div>
          </>
        )}
      </div>
      <DaemonCreateDialog
        onCreated={(daemonId) => void goDaemon(daemonId)}
        onOpenChange={setCreateOpen}
        open={createOpen}
      />
    </div>
  );
}
