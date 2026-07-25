import assert from "node:assert/strict";
import test from "node:test";

import {
  activationModeLabel,
  daemonOutputAction,
  durationLabel,
  formatDaemonActionError,
  isManualDaemonRunEligible,
  readinessLabel,
  runStateLabel,
} from "./presentation.ts";

test("manual runs require a backend-confirmed eligible schedule status", () => {
  assert.equal(isManualDaemonRunEligible("ready"), true);
  assert.equal(isManualDaemonRunEligible("disabled"), true);
  assert.equal(isManualDaemonRunEligible("watch_only"), true);
  assert.equal(isManualDaemonRunEligible("unsupported_runtime"), false);
  assert.equal(isManualDaemonRunEligible("agent_not_ready"), false);
  assert.equal(isManualDaemonRunEligible(undefined), false);
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
