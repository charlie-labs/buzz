import { stringify as stringifyYaml } from "yaml";

export type DaemonActivationChoice = "schedule" | "watch" | "hybrid";

export type BuildDaemonTemplateInput = {
  id: string;
  purpose: string;
  routines: string[];
  body: string;
  activation: DaemonActivationChoice;
  watch?: string[];
  schedule?: string;
  deny?: string[];
};

export function normalizeDaemonId(value: string): string {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64);
}

export function buildDaemonMd(input: BuildDaemonTemplateInput): string {
  const id = normalizeDaemonId(input.id);
  const purpose = input.purpose.trim();
  const routines = input.routines.map((item) => item.trim()).filter(Boolean);
  const body = input.body.trim();
  const watch = (input.watch ?? []).map((item) => item.trim()).filter(Boolean);
  const schedule = input.schedule?.trim() || undefined;

  if (!id) throw new Error("Enter a daemon ID using letters and numbers.");
  if (!purpose)
    throw new Error("Enter the outcome this daemon should achieve.");
  if (routines.length === 0)
    throw new Error("Add at least one concrete routine.");
  if (!body) throw new Error("Add operating guidance for the daemon.");
  if (input.activation !== "watch" && !schedule) {
    throw new Error("Choose a schedule or enter an advanced five-field cron.");
  }
  if (input.activation !== "schedule" && watch.length === 0) {
    throw new Error("Describe at least one observable event to watch.");
  }

  const frontmatter: Record<string, unknown> = {
    id,
    purpose,
    ...(input.activation !== "schedule" ? { watch } : {}),
    routines,
    deny: input.deny?.map((item) => item.trim()).filter(Boolean) ?? [
      "Do not take destructive or irreversible action without explicit human approval.",
    ],
    ...(input.activation !== "watch" ? { schedule } : {}),
  };

  return `---\n${stringifyYaml(frontmatter, { lineWidth: 0 }).trim()}\n---\n\n${body}\n`;
}

export function suggestedWakeInstruction(
  purpose: string,
  routineCount: number,
): string {
  return `Run this daemon now. Work toward: ${purpose.trim()} Review and perform the ${routineCount} configured routine${routineCount === 1 ? "" : "s"}, while following all deny rules and operating guidance.`;
}
