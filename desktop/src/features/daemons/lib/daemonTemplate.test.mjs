import assert from "node:assert/strict";
import test from "node:test";
import { parse as parseYaml } from "yaml";

import { buildDaemonMd, normalizeDaemonId } from "./daemonTemplate.ts";

test("normalizes safe daemon package IDs", () => {
  assert.equal(normalizeDaemonId("  PR Review Helper!  "), "pr-review-helper");
});

test("builds canonical strict scheduled DAEMON.md", () => {
  const markdown = buildDaemonMd({
    id: "docs-check",
    purpose: "Keeps documentation current.",
    routines: ["Identify stale documentation", "Propose focused updates"],
    activation: "schedule",
    schedule: "0 9 * * 1-5",
    body: "## Decision policy\n\nNo-op when everything is current.",
  });
  const frontmatter = markdown.match(/^---\n([\s\S]*?)\n---/)?.[1];
  assert.ok(frontmatter);
  assert.deepEqual(parseYaml(frontmatter), {
    id: "docs-check",
    purpose: "Keeps documentation current.",
    routines: ["Identify stale documentation", "Propose focused updates"],
    deny: [
      "Do not take destructive or irreversible action without explicit human approval.",
    ],
    schedule: "0 9 * * 1-5",
  });
  assert.match(markdown, /## Decision policy/);
});

test("quotes and round-trips cron values that begin with YAML reserved syntax", () => {
  const markdown = buildDaemonMd({
    id: "frequent-check",
    purpose: "Checks a bounded condition frequently.",
    routines: ["Inspect the bounded condition"],
    activation: "schedule",
    schedule: "*/2 * * * *",
    body: "No-op when the condition is healthy.",
  });
  const frontmatter = markdown.match(/^---\n([\s\S]*?)\n---/)?.[1];
  assert.ok(frontmatter);
  assert.match(markdown, /schedule: "\*\/2 \* \* \* \*"/);
  assert.equal(parseYaml(frontmatter).schedule, "*/2 * * * *");
});

test("requires at least one valid activation source", () => {
  assert.throws(
    () =>
      buildDaemonMd({
        id: "watcher",
        purpose: "Watches useful events.",
        routines: ["Review the event"],
        activation: "watch",
        watch: [],
        body: "Act only with current context.",
      }),
    /observable event/,
  );
});
