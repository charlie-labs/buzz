import assert from "node:assert/strict";
import test from "node:test";

import {
  activationModeLabel,
  durationLabel,
  readinessLabel,
  runStateLabel,
} from "./presentation.ts";

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
});

test("distinguishes terminal daemon outcomes", () => {
  assert.equal(
    runStateLabel("terminal", "skipped_overlap"),
    "Skipped: already running",
  );
  assert.equal(runStateLabel("terminal", "cancelled"), "Canceled");
});

test("derives concise run durations", () => {
  assert.equal(
    durationLabel("2026-07-24T20:00:00Z", "2026-07-24T20:00:12Z"),
    "12 sec",
  );
});
