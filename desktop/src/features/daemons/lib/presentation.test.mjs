import assert from "node:assert/strict";
import test from "node:test";

import {
  activationModeLabel,
  daemonOutputAction,
  durationLabel,
  formatDaemonActionError,
  isManualDaemonRunEligible,
  manualDaemonRunDisabledReason,
  readinessLabel,
  runStateLabel,
} from "./presentation.ts";

test("manual runs require a backend-confirmed eligible schedule status", () => {
  for (const readiness of ["ready", "disabled", "watch_only"]) {
    assert.equal(isManualDaemonRunEligible(readiness), true);
  }
  for (const readiness of [
    "missing_package",
    "invalid_schedule",
    "missing_agent",
    "relay_mismatch",
    "channel_unavailable",
    "unsupported_runtime",
    "agent_not_ready",
    "invalid_context",
  ]) {
    assert.equal(isManualDaemonRunEligible(readiness), false);
    assert.ok(readinessLabel(readiness));
  }
  assert.equal(isManualDaemonRunEligible(null), false);
  assert.equal(isManualDaemonRunEligible(undefined), false);
});

test("manual run ineligibility always exposes an explanation", () => {
  const base = {
    bindingId: "binding-1",
    scheduleLoading: false,
    scheduleError: false,
    scheduleStatus: undefined,
  };
  assert.match(
    manualDaemonRunDisabledReason({ ...base, bindingId: null }),
    /Complete setup/,
  );
  assert.match(
    manualDaemonRunDisabledReason({ ...base, scheduleLoading: true }),
    /Checking/,
  );
  assert.match(
    manualDaemonRunDisabledReason({ ...base, scheduleError: true }),
    /could not verify/,
  );
  assert.match(manualDaemonRunDisabledReason(base), /could not verify/);
  assert.equal(
    manualDaemonRunDisabledReason({
      ...base,
      scheduleStatus: {
        readiness: "unsupported_runtime",
        readinessReason: "This runtime cannot complete daemon runs.",
        nextOccurrenceUtc: null,
      },
    }),
    "No run was sent to the desktop runner. This runtime cannot complete daemon runs. Configure the selected managed agent with a daemon-capable local runtime (Codex or Buzz Agent), start or restart it, save or complete setup, and retry.",
  );
  assert.match(
    manualDaemonRunDisabledReason({
      ...base,
      scheduleStatus: {
        readiness: "agent_not_ready",
        readinessReason: "The selected managed agent has not finished setup.",
        nextOccurrenceUtc: null,
      },
    }),
    /No run was sent.*not finished setup.*Codex or Buzz Agent.*retry/,
  );
  for (const readiness of ["ready", "disabled", "watch_only"]) {
    assert.equal(
      manualDaemonRunDisabledReason({
        ...base,
        scheduleStatus: {
          readiness,
          readinessReason: null,
          nextOccurrenceUtc: null,
        },
      }),
      null,
    );
  }
});

test("presents activation and readiness in actionable language", () => {
  assert.equal(activationModeLabel("hybrid"), "Watch + schedule");
  assert.equal(
    readinessLabel("agent_not_ready"),
    "Start or repair the managed agent",
  );
  assert.equal(
    readinessLabel("invalid_context"),
    "Choose an available context folder",
  );
  assert.equal(
    readinessLabel("channel_unavailable"),
    "Output channel could not be validated",
  );
});

test("distinguishes terminal daemon outcomes", () => {
  assert.equal(
    runStateLabel("terminal", "skipped_overlap"),
    "Skipped: already running",
  );
  assert.equal(runStateLabel("terminal", "cancelled"), "Canceled");
  assert.equal(runStateLabel("terminal", "no_op"), "No changes needed");
});

test("only successful published runs expose output actions", () => {
  assert.deepEqual(
    daemonOutputAction({
      status: "succeeded",
      outputEventId: "event-1",
      channelId: "channel-1",
    }),
    { kind: "view", channelId: "channel-1", eventId: "event-1" },
  );
  assert.deepEqual(
    daemonOutputAction({
      status: "succeeded",
      outputEventId: "event-legacy",
    }),
    { kind: "event_id", eventId: "event-legacy" },
  );
  for (const status of ["no_op", "missed", "failed"]) {
    assert.deepEqual(
      daemonOutputAction({ status, outputEventId: "not-real-output" }),
      { kind: "none" },
    );
  }
});

test("formats native action failures with useful context", () => {
  assert.equal(
    formatDaemonActionError("Export daemon", new Error("Disk is read-only")),
    "Export daemon failed. Disk is read-only",
  );
  assert.match(formatDaemonActionError("Open daemon folder", null), /failed/);
});

test("derives concise run durations", () => {
  assert.equal(
    durationLabel("2026-07-24T20:00:00Z", "2026-07-24T20:00:12Z"),
    "12 sec",
  );
});
