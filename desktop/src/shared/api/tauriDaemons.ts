import { invokeTauri } from "./tauri";

export type DaemonActivationMode = "watch_only" | "schedule_only" | "hybrid";
export type DaemonRunTrigger = "manual" | "watch" | "schedule";
export type DaemonRunLifecycle =
  | "reserved"
  | "running"
  | "publishing"
  | "terminal";
export type DaemonRunStatus =
  | "succeeded"
  | "no_op"
  | "failed"
  | "cancelled"
  | "interrupted"
  | "missed"
  | "skipped_overlap"
  | "skipped_unready";
export type DaemonScheduleReadiness =
  | "ready"
  | "disabled"
  | "watch_only"
  | "missing_package"
  | "invalid_schedule"
  | "missing_agent"
  | "relay_mismatch"
  | "channel_unavailable"
  | "unsupported_runtime"
  | "agent_not_ready"
  | "invalid_context";
export type DaemonSchedulerDecisionKind =
  | "ready"
  | "executed"
  | "missed"
  | "skipped_overlap"
  | "skipped_unready"
  | "disabled"
  | "watch_only";

export interface DaemonPackageSummary {
  id: string;
  purpose: string;
  activationMode: DaemonActivationMode;
  packageHash: string;
  watchCount: number;
  routineCount: number;
  hasScripts: boolean;
  hasReferences: boolean;
}

export interface DaemonPackageDetail extends DaemonPackageSummary {
  daemonMd: string;
}

export interface DaemonBinding {
  id: string;
  daemonId: string;
  agentPubkey: string;
  relayUrl: string;
  channelId: string;
  contextConfigured: boolean;
  scheduleEnabled: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface DaemonRunRecord {
  runId: string;
  bindingId: string;
  daemonId: string;
  packageHash: string | null;
  policyHash: string | null;
  trigger: DaemonRunTrigger;
  scheduledForUtc: string | null;
  lifecycle: DaemonRunLifecycle;
  status: DaemonRunStatus | null;
  reservedAt: string;
  startedAt: string | null;
  publishingAt: string | null;
  completedAt: string | null;
  acpSessionId: string | null;
  outputEventId: string | null;
  diagnostic: string | null;
}

export interface LegacyDaemonRunRecord {
  recordType: "legacy";
  runId: string;
  daemonId: string;
  status: DaemonRunStatus;
  startedAt: string;
  completedAt: string;
  acpSessionId: string | null;
  outputEventId: string | null;
  diagnostic: string | null;
}

export interface DaemonScheduleStatus {
  bindingId: string;
  schedule: string | null;
  scheduleHash: string | null;
  readiness: DaemonScheduleReadiness;
  readinessReason: string | null;
  nextOccurrenceUtc: string | null;
  lastDecision: {
    kind: DaemonSchedulerDecisionKind;
    decidedAtUtc: string;
    scheduledForUtc: string | null;
    diagnostic: string | null;
  } | null;
}

export const listDaemonPackages = () =>
  invokeTauri<DaemonPackageSummary[]>("list_daemon_packages");

export const getDaemonPackage = (daemonId: string) =>
  invokeTauri<DaemonPackageDetail>("get_daemon_package", { daemonId });

export const createDaemonPackage = (request: {
  daemonMd: string;
  files?: Array<{ path: string; bytes: number[]; executable?: boolean }>;
}) => invokeTauri<DaemonPackageSummary>("create_daemon_package", { request });

export const updateDaemonPackage = (request: {
  daemonId: string;
  daemonMd: string;
}) => invokeTauri<DaemonPackageSummary>("update_daemon_package", { request });

export const pickAndImportDaemonMd = (replace = false) =>
  invokeTauri<DaemonPackageSummary | null>("pick_and_import_daemon_md", {
    replace,
  });

export const pickAndImportDaemonFolder = (replace = false) =>
  invokeTauri<DaemonPackageSummary | null>("pick_and_import_daemon_folder", {
    replace,
  });

export const pickDaemonContextFolder = () =>
  invokeTauri<string | null>("pick_daemon_context_folder");

export const exportDaemonPackageWithPicker = (daemonId: string) =>
  invokeTauri<boolean>("export_daemon_package_with_picker", { daemonId });

export const deleteDaemonPackage = (daemonId: string) =>
  invokeTauri<void>("delete_daemon_package", { daemonId });

export const openDaemonLibraryFolder = () =>
  invokeTauri<void>("open_daemon_library_folder");

export const listDaemonBindings = () =>
  invokeTauri<DaemonBinding[]>("list_daemon_bindings");

export const getDaemonBinding = (bindingId: string) =>
  invokeTauri<DaemonBinding>("get_daemon_binding", { bindingId });

export const getDaemonScheduleStatus = (bindingId: string) =>
  invokeTauri<DaemonScheduleStatus>("get_daemon_schedule_status", {
    bindingId,
  });

export const createDaemonBinding = (request: {
  daemonId: string;
  agentPubkey: string;
  relayUrl: string;
  channelId: string;
  contextDirectory?: string | null;
  scheduleEnabled: boolean;
}) => invokeTauri<DaemonBinding>("create_daemon_binding", { request });

export const updateDaemonBinding = (request: {
  id: string;
  daemonId?: string;
  agentPubkey?: string;
  relayUrl?: string;
  channelId?: string;
  contextDirectory?: string | null;
  scheduleEnabled?: boolean;
}) => invokeTauri<DaemonBinding>("update_daemon_binding", { request });

export const deleteDaemonBinding = (bindingId: string) =>
  invokeTauri<void>("delete_daemon_binding", { bindingId });

export const runManagedDaemon = (request: {
  runId: string;
  bindingId: string;
  wakeInstruction: string;
  trigger?: DaemonRunTrigger;
  scheduledForUtc?: string;
}) => invokeTauri<DaemonRunRecord>("run_managed_daemon", { request });

export const cancelManagedDaemon = (runId: string) =>
  invokeTauri<void>("cancel_managed_daemon", { runId });

export const listDaemonRunHistory = () =>
  invokeTauri<
    Array<(DaemonRunRecord & { recordType: "managed" }) | LegacyDaemonRunRecord>
  >("list_daemon_run_history");
