import * as React from "react";
import {
  ArrowLeft,
  Download,
  FolderOpen,
  Pencil,
  Play,
  Trash2,
  X,
} from "lucide-react";
import { toast } from "sonner";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import { useManagedAgentRuntimesQuery } from "@/features/agents/managedAgentRuntimeHooks";
import { useManagedAgentsQuery } from "@/features/agents/hooks";
import { useChannelsQuery } from "@/features/channels/hooks";
import { useCommunities } from "@/features/communities/useCommunities";
import {
  useCancelDaemonMutation,
  useDaemonBindingsQuery,
  useDaemonHistoryQuery,
  useDaemonPackageQuery,
  useDaemonScheduleQuery,
  useDeleteDaemonBindingMutation,
  useDeleteDaemonPackageMutation,
  useRunDaemonMutation,
  useSaveDaemonBindingMutation,
  useUpdateDaemonPackageMutation,
} from "@/features/daemons/hooks";
import { suggestedWakeInstruction } from "@/features/daemons/lib/daemonTemplate";
import {
  activationModeLabel,
  daemonOutputAction,
  durationLabel,
  formatDaemonActionError,
  formatUtcAndLocal,
  readinessLabel,
  runStateLabel,
} from "@/features/daemons/lib/presentation";
import { ChannelCombobox } from "@/features/workflows/ui/ChannelCombobox";
import {
  exportDaemonPackageWithPicker,
  openDaemonLibraryFolder,
  pickDaemonContextFolder,
  type DaemonBinding,
  type DaemonPackageDetail,
  type DaemonRunRecord,
  type LegacyDaemonRunRecord,
} from "@/shared/api/tauriDaemons";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui/alert-dialog";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { PageHeader, SectionHeader } from "@/shared/ui/PageHeader";
import { Switch } from "@/shared/ui/switch";
import { Textarea } from "@/shared/ui/textarea";

const selectClassName =
  "h-9 w-full rounded-md border border-input bg-background px-3 text-sm focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50";

export function DaemonDetailView({ daemonId }: { daemonId: string }) {
  const { goChannel, goDaemons } = useAppNavigation();
  const packageQuery = useDaemonPackageQuery(daemonId);
  const bindingsQuery = useDaemonBindingsQuery();
  const historyQuery = useDaemonHistoryQuery();
  const binding =
    bindingsQuery.data?.find((item) => item.daemonId === daemonId) ?? null;
  const scheduleQuery = useDaemonScheduleQuery(binding?.id ?? "");
  const deletePackage = useDeleteDaemonPackageMutation();
  const deleteBinding = useDeleteDaemonBindingMutation();
  const [editOpen, setEditOpen] = React.useState(false);
  const [deleteOpen, setDeleteOpen] = React.useState(false);
  const [exporting, setExporting] = React.useState(false);
  const [openingFolder, setOpeningFolder] = React.useState(false);

  if (packageQuery.isLoading) {
    return (
      <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
        Loading daemon…
      </div>
    );
  }
  if (packageQuery.error || !packageQuery.data) {
    return (
      <div className="flex flex-1 items-center justify-center p-6">
        <div className="max-w-md rounded-2xl border border-border p-6 text-center">
          <h1 className="text-lg font-semibold">Daemon not found</h1>
          <p className="mt-2 text-sm text-muted-foreground">
            It may have been deleted or moved outside the managed daemon
            library.
          </p>
          <Button
            className="mt-4"
            onClick={() => void goDaemons()}
            variant="outline"
          >
            Back to Daemons
          </Button>
        </div>
      </div>
    );
  }

  const daemon = packageQuery.data;
  const daemonHistory = (historyQuery.data ?? []).filter(
    (record) => record.daemonId === daemonId,
  );

  async function handleDelete() {
    try {
      if (binding) await deleteBinding.mutateAsync(binding.id);
      await deletePackage.mutateAsync(daemonId);
      toast.success(`Deleted ${daemonId}`);
      void goDaemons({ replace: true });
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    }
  }

  async function handleExport() {
    if (exporting) return;
    setExporting(true);
    try {
      const exported = await exportDaemonPackageWithPicker(daemonId);
      if (exported) toast.success("Daemon exported");
    } catch (error) {
      toast.error(formatDaemonActionError("Export daemon", error));
    } finally {
      setExporting(false);
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

  return (
    <div
      className="flex min-h-0 flex-1 flex-col overflow-y-auto"
      data-testid="daemon-detail-view"
    >
      <div className="mx-auto flex w-full max-w-6xl flex-col gap-6 px-6 py-8">
        <Button
          className="w-fit"
          onClick={() => void goDaemons()}
          size="sm"
          variant="ghost"
        >
          <ArrowLeft className="h-4 w-4" />
          Daemons
        </Button>
        <PageHeader
          action={
            <div className="flex flex-wrap justify-end gap-2">
              <Button
                onClick={() => setEditOpen(true)}
                size="sm"
                variant="outline"
              >
                <Pencil className="h-4 w-4" />
                Edit DAEMON.md
              </Button>
              <Button
                disabled={exporting}
                onClick={() => void handleExport()}
                size="sm"
                variant="outline"
              >
                <Download className="h-4 w-4" />
                {exporting ? "Exporting…" : "Export"}
              </Button>
              <Button
                disabled={openingFolder}
                onClick={() => void handleOpenFolder()}
                size="sm"
                variant="outline"
              >
                <FolderOpen className="h-4 w-4" />
                {openingFolder ? "Opening…" : "Open folder"}
              </Button>
              <Button
                onClick={() => setDeleteOpen(true)}
                size="sm"
                variant="destructive"
              >
                <Trash2 className="h-4 w-4" />
                Delete
              </Button>
            </div>
          }
          description={daemon.purpose}
          title={daemon.id}
        />

        <div className="grid gap-4 lg:grid-cols-3">
          <SummaryCard
            label="Activation"
            value={activationModeLabel(daemon.activationMode)}
          />
          <SummaryCard
            label="Setup"
            value={binding ? "Primary binding configured" : "Setup needed"}
          />
          <SummaryCard
            label="Schedule readiness"
            value={
              scheduleQuery.data
                ? readinessLabel(scheduleQuery.data.readiness)
                : daemon.activationMode === "watch_only"
                  ? "Manual runs available"
                  : "Configure setup"
            }
          />
        </div>

        {daemon.hasScripts || daemon.hasReferences ? (
          <fieldset className="flex flex-wrap gap-2">
            <legend className="sr-only">Package contents</legend>
            {daemon.hasScripts ? (
              <Badge variant="outline">Scripts</Badge>
            ) : null}
            {daemon.hasReferences ? (
              <Badge variant="outline">References</Badge>
            ) : null}
          </fieldset>
        ) : null}

        <SetupPanel
          binding={binding}
          daemonId={daemonId}
          watchOnly={daemon.activationMode === "watch_only"}
        />

        {binding ? (
          <section className="rounded-2xl border border-border bg-card p-5">
            <SectionHeader
              description="The scheduler uses UTC and runs only while Buzz is open. Missed times are recorded, not replayed."
              title="Schedule"
            />
            {daemon.activationMode === "watch_only" ? (
              <p className="mt-4 text-sm text-muted-foreground">
                This package has watch intent but no schedule. It can run
                manually; automatic watch activation is not enabled in this MVP.
              </p>
            ) : scheduleQuery.isLoading ? (
              <p className="mt-4 text-sm text-muted-foreground">
                Checking schedule…
              </p>
            ) : scheduleQuery.data ? (
              <div className="mt-4 space-y-4">
                {scheduleQuery.data.readiness === "channel_unavailable" ? (
                  <p
                    className="rounded-xl border border-amber-500/30 bg-amber-500/10 p-3 text-sm"
                    role="alert"
                  >
                    The selected output channel could not be validated. Buzz
                    must reach the active relay before setup can be considered
                    ready. Reconnect to the relay or choose an available
                    channel, then save setup again.
                  </p>
                ) : scheduleQuery.data.readinessReason ? (
                  <p className="rounded-xl border border-border bg-muted/50 p-3 text-sm">
                    {scheduleQuery.data.readinessReason}
                  </p>
                ) : null}
                <dl className="grid gap-4 text-sm sm:grid-cols-2">
                  <Info
                    label="Cron"
                    value={scheduleQuery.data.schedule ?? "Not configured"}
                  />
                  <Info
                    label="Readiness"
                    value={readinessLabel(scheduleQuery.data.readiness)}
                  />
                  <Info
                    label="Next occurrence"
                    value={formatUtcAndLocal(
                      scheduleQuery.data.nextOccurrenceUtc,
                    )}
                  />
                  <Info
                    label="Last scheduler decision"
                    value={
                      scheduleQuery.data.lastDecision
                        ? `${scheduleQuery.data.lastDecision.kind} · ${formatUtcAndLocal(scheduleQuery.data.lastDecision.decidedAtUtc)}`
                        : "No decision yet"
                    }
                  />
                </dl>
                {scheduleQuery.data.lastDecision?.diagnostic ? (
                  <details className="text-sm">
                    <summary className="cursor-pointer text-muted-foreground">
                      Scheduler diagnostic
                    </summary>
                    <pre className="mt-2 max-h-40 overflow-auto whitespace-pre-wrap rounded-xl bg-muted p-3 text-xs">
                      {scheduleQuery.data.lastDecision.diagnostic}
                    </pre>
                  </details>
                ) : null}
              </div>
            ) : null}
          </section>
        ) : null}

        <RunPanel
          bindingId={binding?.id ?? null}
          daemon={daemon}
          history={daemonHistory}
        />
        <HistoryPanel
          onViewOutput={(channelId, eventId) =>
            void goChannel(channelId, { messageId: eventId })
          }
          records={daemonHistory}
        />
      </div>
      <DaemonEditor
        daemonId={daemonId}
        initialValue={daemon.daemonMd}
        onOpenChange={setEditOpen}
        open={editOpen}
      />
      <AlertDialog onOpenChange={setDeleteOpen} open={deleteOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete {daemonId}?</AlertDialogTitle>
            <AlertDialogDescription>
              This removes the managed package and its primary binding. Export
              first if you may need it later.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              onClick={() => void handleDelete()}
            >
              Delete daemon
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function SummaryCard({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-2xl border border-border bg-card p-4">
      <p className="text-xs font-medium text-muted-foreground">{label}</p>
      <p className="mt-1 text-sm font-semibold">{value}</p>
    </div>
  );
}

function Info({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <dt className="text-xs font-medium text-muted-foreground">{label}</dt>
      <dd className="mt-1">{value}</dd>
    </div>
  );
}

function SetupPanel({
  binding,
  daemonId,
  watchOnly,
}: {
  binding: DaemonBinding | null;
  daemonId: string;
  watchOnly: boolean;
}) {
  const { activeCommunity } = useCommunities();
  const agentsQuery = useManagedAgentsQuery();
  const runtimesQuery = useManagedAgentRuntimesQuery();
  const channelsQuery = useChannelsQuery();
  const agents = (agentsQuery.data ?? []).filter(
    (agent) => agent.relayUrl === activeCommunity?.relayUrl,
  );
  const channels = (channelsQuery.data ?? []).filter(
    (channel) => channel.isMember,
  );
  const defaultAgent = agents.length === 1 ? agents[0]?.pubkey : "";
  const defaultChannel = channels.length === 1 ? channels[0]?.id : "";
  const [agentPubkey, setAgentPubkey] = React.useState(
    binding?.agentPubkey ?? defaultAgent ?? "",
  );
  const [channelId, setChannelId] = React.useState(
    binding?.channelId ?? defaultChannel ?? "",
  );
  const [contextDirectory, setContextDirectory] = React.useState<
    string | null | undefined
  >(undefined);
  const [contextConfigured, setContextConfigured] = React.useState(
    binding?.contextConfigured ?? false,
  );
  const [scheduleEnabled, setScheduleEnabled] = React.useState(
    binding?.scheduleEnabled ?? !watchOnly,
  );
  const [pickingContext, setPickingContext] = React.useState(false);
  const save = useSaveDaemonBindingMutation(binding?.id);
  const remove = useDeleteDaemonBindingMutation();

  React.useEffect(() => {
    setAgentPubkey(binding?.agentPubkey ?? defaultAgent ?? "");
    setChannelId(binding?.channelId ?? defaultChannel ?? "");
    setContextConfigured(binding?.contextConfigured ?? false);
    setContextDirectory(undefined);
    setScheduleEnabled(binding?.scheduleEnabled ?? !watchOnly);
  }, [binding, defaultAgent, defaultChannel, watchOnly]);

  async function handleSave() {
    if (!activeCommunity?.relayUrl || !agentPubkey || !channelId) return;
    try {
      if (binding) {
        await save.mutateAsync({
          id: binding.id,
          agentPubkey,
          relayUrl: activeCommunity.relayUrl,
          channelId,
          ...(contextDirectory !== undefined ? { contextDirectory } : {}),
          scheduleEnabled: watchOnly ? false : scheduleEnabled,
        });
      } else {
        await save.mutateAsync({
          daemonId,
          agentPubkey,
          relayUrl: activeCommunity.relayUrl,
          channelId,
          contextDirectory: contextDirectory ?? null,
          scheduleEnabled: watchOnly ? false : scheduleEnabled,
        });
      }
      toast.success(binding ? "Daemon setup updated" : "Daemon setup complete");
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    }
  }

  const selectedRuntime = runtimesQuery.data?.find(
    (runtime) =>
      runtime.pubkey === agentPubkey &&
      runtime.relayUrl === activeCommunity?.relayUrl,
  );
  const ready =
    selectedRuntime?.localSetup &&
    ["ready", "listening", "waking"].includes(selectedRuntime.lifecycle);

  return (
    <section
      className="rounded-2xl border border-border bg-card p-5"
      data-testid="daemon-setup-panel"
    >
      <SectionHeader
        description="One primary binding connects this package to a local managed agent, relay, and channel."
        title="Setup"
      />
      <div className="mt-5 grid gap-4 sm:grid-cols-2">
        <label
          className="space-y-1.5 text-sm font-medium"
          htmlFor="daemon-agent"
        >
          Managed agent
          <select
            className={selectClassName}
            id="daemon-agent"
            onChange={(event) => setAgentPubkey(event.target.value)}
            value={agentPubkey}
          >
            <option value="">Select a local agent…</option>
            {agents.map((agent) => {
              const runtime = runtimesQuery.data?.find(
                (item) =>
                  item.pubkey === agent.pubkey &&
                  item.relayUrl === activeCommunity?.relayUrl,
              );
              const usable =
                runtime?.localSetup &&
                runtime.lifecycle !== "failed" &&
                runtime.lifecycle !== "stopped";
              return (
                <option key={agent.pubkey} value={agent.pubkey}>
                  {agent.name}
                  {usable ? "" : " — not ready"}
                </option>
              );
            })}
          </select>
        </label>
        <div className="space-y-1.5 text-sm font-medium">
          <span>Relay</span>
          <div className="flex h-9 items-center rounded-md border border-input px-3 font-normal">
            {activeCommunity?.name ?? "No active community"}
          </div>
        </div>
        <div className="space-y-1.5 text-sm font-medium">
          <label htmlFor="daemon-channel">Channel</label>
          <ChannelCombobox
            channels={channels}
            id="daemon-channel"
            onChange={setChannelId}
            value={channelId}
          />
        </div>
        <div className="space-y-1.5 text-sm font-medium">
          <span>Context folder</span>
          <div className="flex gap-2">
            <Button
              disabled={pickingContext}
              onClick={async () => {
                if (pickingContext) return;
                setPickingContext(true);
                try {
                  const path = await pickDaemonContextFolder();
                  if (path) {
                    setContextDirectory(path);
                    setContextConfigured(true);
                  }
                } catch (error) {
                  toast.error(
                    formatDaemonActionError("Choose context folder", error),
                  );
                } finally {
                  setPickingContext(false);
                }
              }}
              type="button"
              variant="outline"
            >
              {pickingContext
                ? "Choosing…"
                : contextConfigured
                  ? "Change folder"
                  : "Choose folder"}
            </Button>
            {contextConfigured ? (
              <Button
                aria-label="Clear context folder"
                onClick={() => {
                  setContextDirectory(null);
                  setContextConfigured(false);
                }}
                size="icon"
                type="button"
                variant="ghost"
              >
                <X className="h-4 w-4" />
              </Button>
            ) : null}
          </div>
          <p className="text-xs font-normal text-muted-foreground">
            {contextConfigured
              ? "A native folder is configured. The renderer never receives an editable path."
              : "Optional working context, selected with the native folder picker."}
          </p>
        </div>
      </div>
      {!watchOnly ? (
        <label
          className="mt-5 flex items-center justify-between rounded-xl border border-border p-3 text-sm"
          htmlFor="daemon-schedule-enabled"
        >
          <span>
            <span className="font-medium">Enable schedule</span>
            <span className="block text-xs text-muted-foreground">
              Runs only while Buzz is open.
            </span>
          </span>
          <Switch
            checked={scheduleEnabled}
            id="daemon-schedule-enabled"
            onCheckedChange={setScheduleEnabled}
          />
        </label>
      ) : null}
      {agentPubkey && !ready ? (
        <p className="mt-4 rounded-xl border border-amber-500/30 bg-amber-500/10 p-3 text-sm">
          {selectedRuntime?.error ??
            "Start this managed agent and finish its local runtime setup before scheduled runs can become ready."}
        </p>
      ) : null}
      {save.error instanceof Error ? (
        <p className="mt-4 text-sm text-destructive" role="alert">
          {save.error.message}
        </p>
      ) : null}
      <div className="mt-5 flex flex-wrap justify-end gap-2">
        {binding ? (
          <Button
            disabled={remove.isPending}
            onClick={() =>
              void remove
                .mutateAsync(binding.id)
                .then(() => toast.success("Binding removed"))
            }
            variant="ghost"
          >
            Remove binding
          </Button>
        ) : null}
        <Button
          disabled={
            !activeCommunity?.relayUrl ||
            !agentPubkey ||
            !channelId ||
            save.isPending
          }
          onClick={() => void handleSave()}
        >
          {save.isPending
            ? "Saving…"
            : binding
              ? "Save setup"
              : "Complete setup"}
        </Button>
      </div>
    </section>
  );
}

type ManagedHistoryRecord = DaemonRunRecord & { recordType: "managed" };
type HistoryRecord = ManagedHistoryRecord | LegacyDaemonRunRecord;

function RunPanel({
  bindingId,
  daemon,
  history,
}: {
  bindingId: string | null;
  daemon: DaemonPackageDetail;
  history: HistoryRecord[];
}) {
  const run = useRunDaemonMutation(bindingId ?? "");
  const cancel = useCancelDaemonMutation();
  const [wakeInstruction, setWakeInstruction] = React.useState(() =>
    suggestedWakeInstruction(daemon.purpose, daemon.routineCount),
  );
  const active =
    history.find(
      (record): record is ManagedHistoryRecord =>
        record.recordType === "managed" && record.status === null,
    ) ?? null;

  async function handleRun() {
    if (!bindingId) return;
    try {
      await run.mutateAsync({
        runId: crypto.randomUUID(),
        bindingId,
        wakeInstruction: wakeInstruction.trim(),
        trigger: "manual",
      });
      toast.success("Daemon run started");
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    }
  }

  return (
    <section className="rounded-2xl border border-border bg-card p-5">
      <SectionHeader
        description="Manual runs work for scheduled, disabled, and watch-only daemons once setup is valid."
        title="Run now"
      />
      <Textarea
        className="mt-4"
        disabled={!bindingId || Boolean(active)}
        onChange={(event) => setWakeInstruction(event.target.value)}
        rows={3}
        value={wakeInstruction}
      />
      {!bindingId ? (
        <p className="mt-2 text-xs text-muted-foreground">
          Complete setup before running this daemon.
        </p>
      ) : null}
      <div className="mt-4 flex justify-end gap-2">
        {active ? (
          <>
            <Badge variant="secondary">
              {runStateLabel(active.lifecycle, active.status)}
            </Badge>
            <Button
              disabled={cancel.isPending}
              onClick={() => void cancel.mutateAsync(active.runId)}
              variant="outline"
            >
              Cancel run
            </Button>
          </>
        ) : (
          <Button
            disabled={!bindingId || !wakeInstruction.trim() || run.isPending}
            onClick={() => void handleRun()}
          >
            <Play className="h-4 w-4" />
            {run.isPending ? "Starting…" : "Run now"}
          </Button>
        )}
      </div>
    </section>
  );
}

function HistoryPanel({
  onViewOutput,
  records,
}: {
  onViewOutput: (channelId: string, eventId: string) => void;
  records: HistoryRecord[];
}) {
  return (
    <section className="rounded-2xl border border-border bg-card p-5">
      <SectionHeader
        description="Recent manual and scheduled decisions from the durable local run ledger."
        title="Run history"
      />
      {records.length === 0 ? (
        <p className="mt-4 text-sm text-muted-foreground">No runs yet.</p>
      ) : (
        <div className="mt-4 divide-y divide-border">
          {records.map((record) => {
            const managed = record.recordType === "managed";
            const outputAction = daemonOutputAction({
              status: record.status,
              outputEventId: record.outputEventId,
              channelId: managed ? record.binding.channelId : null,
            });
            const started = managed
              ? (record.startedAt ?? record.reservedAt)
              : record.startedAt;
            const duration = durationLabel(started, record.completedAt);
            return (
              <div className="py-4 first:pt-0" key={record.runId}>
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div className="flex items-center gap-2">
                    <Badge
                      variant={
                        record.status === "succeeded"
                          ? "success"
                          : record.status === "no_op"
                            ? "info"
                            : record.status === "failed"
                              ? "destructive"
                              : record.status === "missed"
                                ? "warning"
                                : "secondary"
                      }
                    >
                      {managed
                        ? runStateLabel(record.lifecycle, record.status)
                        : runStateLabel("terminal", record.status)}
                    </Badge>
                    <span className="text-sm font-medium">
                      {managed ? record.trigger : "legacy"}
                    </span>
                  </div>
                  <span className="text-xs text-muted-foreground">
                    {formatUtcAndLocal(started)}
                    {duration ? ` · ${duration}` : ""}
                  </span>
                </div>
                {managed && record.scheduledForUtc ? (
                  <p className="mt-2 text-xs text-muted-foreground">
                    Scheduled for {formatUtcAndLocal(record.scheduledForUtc)}
                  </p>
                ) : null}
                {record.diagnostic ? (
                  <details className="mt-2 text-sm">
                    <summary className="cursor-pointer text-muted-foreground">
                      Diagnostic
                    </summary>
                    <pre className="mt-2 max-h-40 overflow-auto whitespace-pre-wrap rounded-xl bg-muted p-3 text-xs">
                      {record.diagnostic}
                    </pre>
                  </details>
                ) : null}
                {outputAction.kind === "view" ? (
                  <Button
                    className="mt-2"
                    onClick={() =>
                      onViewOutput(outputAction.channelId, outputAction.eventId)
                    }
                    size="sm"
                    variant="outline"
                  >
                    View output
                  </Button>
                ) : outputAction.kind === "event_id" ? (
                  <details className="mt-2 text-sm">
                    <summary className="cursor-pointer text-muted-foreground">
                      Output event ID
                    </summary>
                    <div className="mt-2 flex flex-wrap items-center gap-2 rounded-xl bg-muted p-3">
                      <code className="break-all text-xs">
                        {outputAction.eventId}
                      </code>
                      <Button
                        onClick={() => {
                          void navigator.clipboard
                            .writeText(outputAction.eventId)
                            .then(() => toast.success("Output event ID copied"))
                            .catch((error) =>
                              toast.error(
                                formatDaemonActionError(
                                  "Copy output event ID",
                                  error,
                                ),
                              ),
                            );
                        }}
                        size="sm"
                        variant="outline"
                      >
                        Copy event ID
                      </Button>
                    </div>
                    <p className="mt-2 text-xs text-muted-foreground">
                      This older record does not include a channel snapshot, so
                      Buzz cannot focus the message directly.
                    </p>
                  </details>
                ) : null}
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}

function DaemonEditor({
  daemonId,
  initialValue,
  onOpenChange,
  open,
}: {
  daemonId: string;
  initialValue: string;
  onOpenChange: (open: boolean) => void;
  open: boolean;
}) {
  const [value, setValue] = React.useState(initialValue);
  const update = useUpdateDaemonPackageMutation(daemonId);
  React.useEffect(() => {
    if (open) {
      setValue(initialValue);
      update.reset();
    }
  }, [initialValue, open, update.reset]);
  async function handleSave() {
    try {
      await update.mutateAsync({ daemonId, daemonMd: value });
      toast.success("DAEMON.md updated");
      onOpenChange(false);
    } catch {
      /* Query mutation exposes parser detail. */
    }
  }
  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent className="flex max-h-[88vh] flex-col overflow-hidden sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>Edit DAEMON.md</DialogTitle>
          <DialogDescription>
            Strict validation runs before save. Scripts, references, and other
            support files remain intact.
          </DialogDescription>
        </DialogHeader>
        <Textarea
          className="min-h-0 flex-1 font-mono text-sm"
          onChange={(event) => setValue(event.target.value)}
          rows={24}
          value={value}
        />
        {update.error instanceof Error ? (
          <p
            className="rounded-xl border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive"
            role="alert"
          >
            {update.error.message}
          </p>
        ) : null}
        <div className="flex justify-end gap-2">
          <Button onClick={() => onOpenChange(false)} variant="outline">
            Cancel
          </Button>
          <Button
            disabled={!value.trim() || update.isPending}
            onClick={() => void handleSave()}
          >
            {update.isPending ? "Validating…" : "Save DAEMON.md"}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
