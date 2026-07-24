import type {
  DaemonActivationMode,
  DaemonRunLifecycle,
  DaemonRunStatus,
  DaemonScheduleReadiness,
} from "@/shared/api/tauriDaemons";

export const schedulePresets = [
  { label: "Hourly", value: "0 * * * *" },
  { label: "Weekday morning (09:00 UTC)", value: "0 9 * * 1-5" },
  { label: "Daily (09:00 UTC)", value: "0 9 * * *" },
] as const;

export function activationModeLabel(mode: DaemonActivationMode): string {
  if (mode === "watch_only") return "Watch only";
  if (mode === "schedule_only") return "Scheduled";
  return "Watch + schedule";
}

export function readinessLabel(readiness: DaemonScheduleReadiness): string {
  const labels: Record<DaemonScheduleReadiness, string> = {
    ready: "Ready",
    disabled: "Schedule disabled",
    watch_only: "Manual runs available",
    missing_package: "Daemon package is missing",
    invalid_schedule: "Fix the schedule in DAEMON.md",
    missing_agent: "Choose a managed agent",
    relay_mismatch: "Agent is connected to a different relay",
    unsupported_runtime: "Choose a supported local agent",
    agent_not_ready: "Start or repair the managed agent",
    invalid_context: "Choose an available context folder",
  };
  return labels[readiness];
}

export function runStateLabel(
  lifecycle: DaemonRunLifecycle,
  status: DaemonRunStatus | null,
): string {
  if (!status) {
    return {
      reserved: "Reserved",
      running: "Running",
      publishing: "Publishing",
      terminal: "Finishing",
    }[lifecycle];
  }
  return {
    succeeded: "Succeeded",
    failed: "Failed",
    cancelled: "Canceled",
    interrupted: "Interrupted",
    missed: "Missed",
    skipped_overlap: "Skipped: already running",
    skipped_unready: "Skipped: setup not ready",
  }[status];
}

export function formatUtcAndLocal(value: string | null): string {
  if (!value) return "Not scheduled";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  const utc = new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
    timeZone: "UTC",
  }).format(date);
  const local = new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
  return `${utc} UTC · ${local} local`;
}

export function durationLabel(start: string | null, end: string | null) {
  if (!start || !end) return null;
  const milliseconds = new Date(end).getTime() - new Date(start).getTime();
  if (!Number.isFinite(milliseconds) || milliseconds < 0) return null;
  if (milliseconds < 1_000) return `${milliseconds} ms`;
  if (milliseconds < 60_000) return `${Math.round(milliseconds / 1_000)} sec`;
  return `${Math.round(milliseconds / 60_000)} min`;
}
