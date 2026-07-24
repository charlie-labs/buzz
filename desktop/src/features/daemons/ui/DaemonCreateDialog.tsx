import * as React from "react";

import {
  buildDaemonMd,
  normalizeDaemonId,
  type DaemonActivationChoice,
} from "@/features/daemons/lib/daemonTemplate";
import { schedulePresets } from "@/features/daemons/lib/presentation";
import { useCreateDaemonPackageMutation } from "@/features/daemons/hooks";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { Input } from "@/shared/ui/input";
import { Textarea } from "@/shared/ui/textarea";

type DaemonCreateDialogProps = {
  onCreated: (daemonId: string) => void;
  onOpenChange: (open: boolean) => void;
  open: boolean;
};

const selectClassName =
  "h-9 w-full rounded-md border border-input bg-background px-3 text-sm focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50";

export function DaemonCreateDialog({
  onCreated,
  onOpenChange,
  open,
}: DaemonCreateDialogProps) {
  const mutation = useCreateDaemonPackageMutation();
  const [id, setId] = React.useState("");
  const [purpose, setPurpose] = React.useState("");
  const [routines, setRoutines] = React.useState("");
  const [body, setBody] = React.useState(
    "## Decision policy\n\nAct only when the configured routines have a clear, current target. No-op when context is missing or the work is already complete.\n\n## Verification and freshness\n\nRe-check current state before any visible or consequential action. Summarize what changed and what was verified.",
  );
  const [activation, setActivation] =
    React.useState<DaemonActivationChoice>("schedule");
  const [watch, setWatch] = React.useState(
    "A relevant GitHub, Linear, or Slack event is routed to this daemon.",
  );
  const [schedule, setSchedule] = React.useState("0 9 * * 1-5");
  const [advancedSchedule, setAdvancedSchedule] = React.useState(false);
  const [validationError, setValidationError] = React.useState<string | null>(
    null,
  );

  React.useEffect(() => {
    if (!open) return;
    mutation.reset();
    setValidationError(null);
  }, [open, mutation.reset]);

  async function handleCreate() {
    try {
      const daemonMd = buildDaemonMd({
        id,
        purpose,
        routines: routines.split("\n"),
        body,
        activation,
        watch: watch.split("\n"),
        schedule,
      });
      setValidationError(null);
      const created = await mutation.mutateAsync({ daemonMd });
      onOpenChange(false);
      onCreated(created.id);
    } catch (error) {
      setValidationError(
        error instanceof Error ? error.message : String(error),
      );
    }
  }

  const error =
    validationError ??
    (mutation.error instanceof Error ? mutation.error.message : null);

  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent className="flex max-h-[88vh] flex-col overflow-hidden sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>New daemon</DialogTitle>
          <DialogDescription>
            Start with one narrow, repeatable job. You can edit the generated
            DAEMON.md later without changing support files.
          </DialogDescription>
        </DialogHeader>
        <div className="min-h-0 flex-1 space-y-5 overflow-y-auto pr-1">
          <div className="grid gap-4 sm:grid-cols-2">
            <label
              className="space-y-1.5 text-sm font-medium"
              htmlFor="daemon-id"
            >
              Daemon ID
              <Input
                aria-describedby="daemon-id-help"
                id="daemon-id"
                onChange={(event) =>
                  setId(normalizeDaemonId(event.target.value))
                }
                placeholder="pr-review-helper"
                value={id}
              />
              <span
                className="block text-xs font-normal text-muted-foreground"
                id="daemon-id-help"
              >
                Lowercase letters, numbers, and hyphens. This becomes the
                package folder name.
              </span>
            </label>
            <label
              className="space-y-1.5 text-sm font-medium"
              htmlFor="daemon-purpose"
            >
              Purpose
              <Input
                id="daemon-purpose"
                onChange={(event) => setPurpose(event.target.value)}
                placeholder="Keeps pull request descriptions useful and current."
                value={purpose}
              />
            </label>
          </div>

          <label
            className="block space-y-1.5 text-sm font-medium"
            htmlFor="daemon-routines"
          >
            Routines
            <Textarea
              id="daemon-routines"
              onChange={(event) => setRoutines(event.target.value)}
              placeholder={
                "Identify missing test evidence\nPropose a concise PR summary"
              }
              rows={3}
              value={routines}
            />
            <span className="block text-xs font-normal text-muted-foreground">
              One concrete, finite operation per line. Two or three is a good
              start.
            </span>
          </label>

          <fieldset className="space-y-3">
            <legend className="text-sm font-medium">Activation</legend>
            <div className="grid gap-2 sm:grid-cols-3">
              {(
                [
                  ["schedule", "Schedule", "Survey on a regular cadence."],
                  [
                    "watch",
                    "Watch only",
                    "Manual now; event automation is not enabled in this MVP.",
                  ],
                  [
                    "hybrid",
                    "Both",
                    "Keep watch intent and a scheduled review.",
                  ],
                ] as const
              ).map(([value, label, description]) => (
                <label
                  className="rounded-xl border border-border p-3 text-sm has-[:checked]:border-primary has-[:checked]:bg-primary/5"
                  key={value}
                >
                  <span className="flex items-center gap-2 font-medium">
                    <input
                      checked={activation === value}
                      name="daemon-activation"
                      onChange={() => setActivation(value)}
                      type="radio"
                    />
                    {label}
                  </span>
                  <span className="mt-1 block text-xs text-muted-foreground">
                    {description}
                  </span>
                </label>
              ))}
            </div>
          </fieldset>

          {activation !== "schedule" ? (
            <label
              className="block space-y-1.5 text-sm font-medium"
              htmlFor="daemon-watch"
            >
              Watch intent
              <Textarea
                id="daemon-watch"
                onChange={(event) => setWatch(event.target.value)}
                rows={2}
                value={watch}
              />
              <span className="block text-xs font-normal text-muted-foreground">
                Describe observable events. Watch automation is not enabled in
                this desktop MVP; watch-only daemons can still run manually.
              </span>
            </label>
          ) : null}

          {activation !== "watch" ? (
            <div className="space-y-2">
              <label
                className="block space-y-1.5 text-sm font-medium"
                htmlFor="daemon-schedule"
              >
                Schedule
                {advancedSchedule ? (
                  <Input
                    id="daemon-schedule"
                    onChange={(event) => setSchedule(event.target.value)}
                    value={schedule}
                  />
                ) : (
                  <select
                    className={selectClassName}
                    id="daemon-schedule"
                    onChange={(event) => setSchedule(event.target.value)}
                    value={schedule}
                  >
                    {schedulePresets.map((preset) => (
                      <option key={preset.value} value={preset.value}>
                        {preset.label}
                      </option>
                    ))}
                  </select>
                )}
              </label>
              <button
                className="text-xs font-medium text-primary hover:underline"
                onClick={() => setAdvancedSchedule((value) => !value)}
                type="button"
              >
                {advancedSchedule
                  ? "Use a preset"
                  : "Advanced: five-field UTC cron"}
              </button>
              <p className="text-xs text-muted-foreground">
                Schedules are evaluated in UTC and run only while Buzz is open.
                Missed times are recorded, not replayed.
              </p>
            </div>
          ) : null}

          <label
            className="block space-y-1.5 text-sm font-medium"
            htmlFor="daemon-body"
          >
            Operating guidance
            <Textarea
              id="daemon-body"
              onChange={(event) => setBody(event.target.value)}
              rows={8}
              value={body}
            />
          </label>

          {error ? (
            <p
              className="rounded-xl border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive"
              role="alert"
            >
              {error}
            </p>
          ) : null}
        </div>
        <div className="flex justify-end gap-2 border-t border-border pt-4">
          <Button
            onClick={() => onOpenChange(false)}
            type="button"
            variant="outline"
          >
            Cancel
          </Button>
          <Button
            disabled={mutation.isPending}
            onClick={handleCreate}
            type="button"
          >
            {mutation.isPending ? "Creating…" : "Create and set up"}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
