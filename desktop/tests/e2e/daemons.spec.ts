import { expect, test } from "@playwright/test";

import { installMockBridge } from "../helpers/bridge";

const AGENT_PUBKEY = "a1".repeat(32);
const OUTPUT_EVENT_ID = "e1".repeat(32);

const managedAgentMocks = {
  managedAgents: [
    { pubkey: AGENT_PUBKEY, name: "Local Helper", status: "running" as const },
  ],
  managedAgentRuntimes: [
    {
      pubkey: AGENT_PUBKEY,
      relayUrl: "ws://localhost:3000",
      lifecycle: "ready" as const,
    },
  ],
};

async function openDaemons(page: import("@playwright/test").Page) {
  await page.goto("/");
  await page.getByTestId("open-daemons-view").click();
  await expect(page).toHaveURL(/#\/daemons$/);
  await expect(page.getByTestId("daemons-view")).toBeVisible();
}

test("empty library creates, sets up, schedules, and runs a daemon", async ({
  page,
}) => {
  await installMockBridge(page, {
    managedAgents: [
      { pubkey: AGENT_PUBKEY, name: "Local Helper", status: "running" },
    ],
    managedAgentRuntimes: [
      {
        pubkey: AGENT_PUBKEY,
        relayUrl: "ws://localhost:3000",
        lifecycle: "ready",
      },
    ],
  });
  await openDaemons(page);
  await expect(page.getByText("No daemons yet")).toBeVisible();

  await page.getByRole("button", { name: "New daemon" }).first().click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Daemon ID").fill("docs-check");
  await dialog
    .getByLabel("Purpose")
    .fill("Keeps documentation current and useful.");
  await dialog
    .locator("#daemon-routines")
    .fill("Identify stale documentation\nPropose focused updates");
  await dialog.getByRole("button", { name: "Create and set up" }).click();

  await expect(page).toHaveURL(/#\/daemons\/docs-check$/);
  await expect(page.getByTestId("daemon-detail-view")).toBeVisible();
  const setup = page.getByTestId("daemon-setup-panel");
  await expect(setup.getByLabel("Managed agent")).toHaveValue(AGENT_PUBKEY);
  await setup.getByRole("combobox", { name: "Channel" }).click();
  await page.getByRole("button", { name: /general · stream/ }).click();
  await setup.getByRole("button", { name: "Complete setup" }).click();

  await expect(page.getByText("Ready", { exact: true }).first()).toBeVisible();
  await expect(page.getByText("Next occurrence")).toBeVisible();
  await expect(page.getByText(/UTC · .* local/)).toBeVisible();
  await page.getByRole("button", { name: "Run now" }).click();
  await expect(page.getByText("Succeeded")).toBeVisible();
  await expect(page.getByText("manual", { exact: true })).toBeVisible();
});

test("native import cancellation is a silent no-op", async ({ page }) => {
  await installMockBridge(page);
  await openDaemons(page);
  await page.getByRole("button", { name: "Import DAEMON.md" }).first().click();
  await expect(page).toHaveURL(/#\/daemons$/);
  await expect(page.getByText("No daemons yet")).toBeVisible();
  await expect(page.locator('[data-testid^="daemon-card-"]')).toHaveCount(0);
});

test("one-file DAEMON.md import opens the shared detail and setup flow", async ({
  page,
}) => {
  await installMockBridge(page, {
    ...managedAgentMocks,
    daemonImports: [
      {
        kind: "file",
        id: "imported-file",
        purpose: "Keeps imported docs current.",
        schedule: "0 9 * * 1-5",
      },
    ],
  });
  await openDaemons(page);
  await page.getByRole("button", { name: "Import DAEMON.md" }).first().click();

  await expect(page).toHaveURL(/#\/daemons\/imported-file$/);
  await expect(page.getByTestId("daemon-detail-view")).toBeVisible();
  await expect(page.getByTestId("daemon-setup-panel")).toBeVisible();
  await expect(
    page.getByText("Keeps imported docs current.", { exact: true }),
  ).toBeVisible();
});

test("folder import preserves scripts and references package indicators", async ({
  page,
}) => {
  await installMockBridge(page, {
    daemonImports: [
      {
        kind: "folder",
        id: "package-import",
        purpose: "Uses a portable support tree.",
        hasScripts: true,
        hasReferences: true,
      },
    ],
  });
  await openDaemons(page);
  await page.getByRole("button", { name: "Import folder" }).first().click();

  await expect(page).toHaveURL(/#\/daemons\/package-import$/);
  const contents = page.getByLabel("Package contents");
  await expect(contents.getByText("Scripts", { exact: true })).toBeVisible();
  await expect(contents.getByText("References", { exact: true })).toBeVisible();
});

test("malformed import reports a useful error without creating a package", async ({
  page,
}) => {
  await installMockBridge(page, {
    daemonImports: [
      {
        kind: "file",
        error:
          "DAEMON.md requires id, purpose, routines, and at least one of watch or schedule.",
      },
    ],
  });
  await openDaemons(page);
  await page.getByRole("button", { name: "Import DAEMON.md" }).first().click();

  await expect(
    page.getByText(/DAEMON\.md requires id, purpose, routines/),
  ).toBeVisible();
  await expect(page).toHaveURL(/#\/daemons$/);
  await expect(page.locator('[data-testid^="daemon-card-"]')).toHaveCount(0);
});

test("duplicate import replaces only after the explicit Replace action", async ({
  page,
}) => {
  await installMockBridge(page, {
    daemonPackages: [{ id: "duplicate", purpose: "Original package purpose." }],
    daemonImports: [
      {
        kind: "file",
        id: "duplicate",
        purpose: "Replacement package purpose.",
      },
    ],
  });
  await openDaemons(page);
  await page.getByRole("button", { name: "Import DAEMON.md" }).first().click();

  await expect(page.getByText(/Retry with Replace/)).toBeVisible();
  await expect(page.getByText("Original package purpose.")).toBeVisible();
  await page.getByRole("button", { name: "Replace" }).click();
  await expect(page).toHaveURL(/#\/daemons\/duplicate$/);
  await expect(
    page.getByText("Replacement package purpose.", { exact: true }),
  ).toBeVisible();
});

test("schedule disable and re-enable update readiness without hiding UTC and Buzz-open disclosure", async ({
  page,
}) => {
  await installMockBridge(page, {
    ...managedAgentMocks,
    daemonPackages: [
      {
        id: "scheduled",
        purpose: "Runs on a controlled schedule.",
        schedule: "0 9 * * *",
      },
    ],
    daemonBinding: {
      daemonId: "scheduled",
      agentPubkey: AGENT_PUBKEY,
      channelId: "36411e44-0e2d-4cfe-bd6e-567eb169db9f",
      scheduleEnabled: true,
    },
  });
  await openDaemons(page);
  await page.getByTestId("daemon-card-scheduled").click();
  const setup = page.getByTestId("daemon-setup-panel");
  const scheduleSwitch = setup.getByLabel("Enable schedule");

  await scheduleSwitch.click();
  await setup.getByRole("button", { name: "Save setup" }).click();
  await expect(
    page.getByText("Schedule disabled", { exact: true }).first(),
  ).toBeVisible();
  await expect(
    setup.getByText("Runs only while Buzz is open.", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText(/scheduler uses UTC/)).toBeVisible();

  await scheduleSwitch.click();
  await setup.getByRole("button", { name: "Save setup" }).click();
  await expect(page.getByText("Ready", { exact: true }).first()).toBeVisible();
  await expect(page.getByText(/UTC · .* local/)).toBeVisible();
});

test("channel-unavailable readiness explains active relay validation", async ({
  page,
}) => {
  await installMockBridge(page, {
    ...managedAgentMocks,
    daemonPackages: [
      {
        id: "unavailable",
        purpose: "Needs an output channel.",
        schedule: "0 9 * * *",
      },
    ],
    daemonBinding: {
      daemonId: "unavailable",
      agentPubkey: AGENT_PUBKEY,
      channelId: "missing-channel",
    },
    daemonScheduleStatus: {
      readiness: "channel_unavailable",
      readinessReason: "The active relay did not confirm the selected channel.",
      nextOccurrenceUtc: null,
    },
  });
  await openDaemons(page);
  await page.getByTestId("daemon-card-unavailable").click();

  await expect(
    page
      .getByText("Output channel could not be validated", { exact: true })
      .first(),
  ).toBeVisible();
  await expect(page.getByRole("alert")).toContainText(
    "must reach the active relay before setup can be considered ready",
  );
});

test("export and open-folder commands report success and actionable failures", async ({
  page,
}) => {
  await installMockBridge(page, {
    daemonPackages: [{ id: "actions", purpose: "Exercises native actions." }],
    daemonExportResults: [true, "The selected destination is read-only."],
    daemonOpenFolderResults: [
      null,
      "The daemon library folder is unavailable.",
    ],
  });
  await openDaemons(page);
  await page.getByTestId("daemon-card-actions").click();

  await page.getByRole("button", { name: "Export" }).click();
  await expect(page.getByText("Daemon exported")).toBeVisible();
  await page.getByRole("button", { name: "Export" }).click();
  await expect(
    page.getByText(
      "Export daemon failed. The selected destination is read-only.",
    ),
  ).toBeVisible();

  await page.getByRole("button", { name: "Open folder" }).click();
  await page.getByRole("button", { name: "Open folder" }).click();
  await expect(
    page.getByText(
      "Open daemon folder failed. The daemon library folder is unavailable.",
    ),
  ).toBeVisible();

  const commands = await page.evaluate(
    () => window.__BUZZ_E2E_COMMANDS__ ?? [],
  );
  expect(
    commands.filter(
      (command) => command === "export_daemon_package_with_picker",
    ),
  ).toHaveLength(2);
  expect(
    commands.filter((command) => command === "open_daemon_library_folder"),
  ).toHaveLength(2);
});

test("history distinguishes outcomes and only real output navigates", async ({
  page,
}) => {
  await installMockBridge(page, {
    daemonPackages: [{ id: "history", purpose: "Shows durable outcomes." }],
    daemonBinding: {
      daemonId: "history",
      agentPubkey: AGENT_PUBKEY,
      channelId: "36411e44-0e2d-4cfe-bd6e-567eb169db9f",
    },
    daemonHistory: [
      {
        runId: "success",
        daemonId: "history",
        status: "succeeded",
        outputEventId: OUTPUT_EVENT_ID,
      },
      {
        runId: "failure",
        daemonId: "history",
        status: "failed",
        diagnostic: "The agent returned a bounded failure diagnostic.",
      },
      {
        runId: "noop",
        daemonId: "history",
        status: "no_op",
        outputEventId: "not-a-real-output",
      },
      {
        runId: "missed",
        daemonId: "history",
        status: "missed",
        trigger: "schedule",
        outputEventId: "not-a-real-output-either",
      },
    ],
  });
  await openDaemons(page);
  await page.getByTestId("daemon-card-history").click();

  await expect(page.getByText("Succeeded", { exact: true })).toBeVisible();
  await expect(page.getByText("Failed", { exact: true })).toBeVisible();
  await expect(
    page.getByText("No changes needed", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText("Missed", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "View output" })).toHaveCount(
    1,
  );
  await expect(
    page.getByText("not-a-real-output", { exact: true }),
  ).toHaveCount(0);

  await page.getByRole("button", { name: "View output" }).click();
  await expect(page).toHaveURL(new RegExp(`messageId=${OUTPUT_EVENT_ID}`));
});
