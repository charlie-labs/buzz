import { expect, test } from "@playwright/test";

import { installMockBridge } from "../helpers/bridge";

const AGENT_PUBKEY = "a1".repeat(32);

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
