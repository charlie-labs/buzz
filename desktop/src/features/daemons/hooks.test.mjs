import assert from "node:assert/strict";
import test from "node:test";

import { daemonHistoryRefetchInterval } from "./hooks.ts";

test("daemon history polls while a run mutation is pending", () => {
  assert.equal(daemonHistoryRefetchInterval([], 1), 1_000);
  assert.equal(daemonHistoryRefetchInterval(undefined, 2), 1_000);
});

test("daemon history polling transfers to the reserved active receipt", () => {
  const active = {
    recordType: "managed",
    status: null,
  };
  assert.equal(daemonHistoryRefetchInterval([active], 0), 1_000);
});

test("daemon history does not poll after terminal settlement", () => {
  const terminal = {
    recordType: "managed",
    status: "failed",
  };
  assert.equal(daemonHistoryRefetchInterval([terminal], 0), false);
  assert.equal(daemonHistoryRefetchInterval([], 0), false);
});
