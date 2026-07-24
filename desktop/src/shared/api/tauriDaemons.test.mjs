import assert from "node:assert/strict";
import test from "node:test";

const calls = [];
globalThis.window = globalThis;
globalThis.__TAURI_INTERNALS__ = {
  invoke(command, payload) {
    calls.push({ command, payload });
    if (command === "pick_and_import_daemon_md") return Promise.resolve(null);
    if (command === "export_daemon_package_with_picker")
      return Promise.resolve(false);
    return Promise.resolve({});
  },
};

const {
  exportDaemonPackageWithPicker,
  getDaemonScheduleStatus,
  pickAndImportDaemonMd,
  runManagedDaemon,
  updateDaemonPackage,
} = await import("./tauriDaemons.ts");

test("package update wrapper preserves camelCase request contract", async () => {
  calls.length = 0;
  await updateDaemonPackage({
    daemonId: "docs",
    daemonMd: "---\nid: docs\n---",
  });
  assert.deepEqual(calls, [
    {
      command: "update_daemon_package",
      payload: {
        request: { daemonId: "docs", daemonMd: "---\nid: docs\n---" },
      },
    },
  ]);
});

test("picker cancellation remains a null or false no-op", async () => {
  assert.equal(await pickAndImportDaemonMd(), null);
  assert.equal(await exportDaemonPackageWithPicker("docs"), false);
});

test("schedule status and scheduled run wrappers use typed UTC fields", async () => {
  calls.length = 0;
  await getDaemonScheduleStatus("binding-1");
  await runManagedDaemon({
    runId: "7a267aae-5210-5c8a-9f7a-31f919f83544",
    bindingId: "binding-1",
    wakeInstruction: "scheduled",
    trigger: "schedule",
    scheduledForUtc: "2026-07-24T21:00:00Z",
  });
  assert.deepEqual(calls, [
    {
      command: "get_daemon_schedule_status",
      payload: { bindingId: "binding-1" },
    },
    {
      command: "run_managed_daemon",
      payload: {
        request: {
          runId: "7a267aae-5210-5c8a-9f7a-31f919f83544",
          bindingId: "binding-1",
          wakeInstruction: "scheduled",
          trigger: "schedule",
          scheduledForUtc: "2026-07-24T21:00:00Z",
        },
      },
    },
  ]);
});
