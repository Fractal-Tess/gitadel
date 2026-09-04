<script lang="ts">
  import { toString as cronToString } from "cronstrue";
  import CalendarClock from "@lucide/svelte/icons/calendar-clock";
  import ShieldCheck from "@lucide/svelte/icons/shield-check";
  import { toast } from "svelte-sonner";

  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import {
    integritySettingsSchema,
    type IntegritySettings,
  } from "$lib/api/instance.js";
  import {
    ApiFailure,
    jsonBody,
    requestJson,
  } from "$lib/api/transport.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  let settings = $state.raw<IntegritySettings | null>(null);
  let enabled = $state(true);
  let schedule = $state("0 3 * * *");
  let loading = $state(true);
  let saving = $state(false);
  let loadVersion = 0;

  let cronExplanation = $derived.by(() => {
    try {
      return {
        valid: schedule.trim().length > 0,
        description: cronToString(schedule.trim(), {
          use24HourTimeFormat: true,
          verbose: true,
        }),
      };
    } catch {
      return {
        valid: false,
        description: "Enter a valid UTC cron expression.",
      };
    }
  });
  let dirty = $derived(
    settings !== null &&
      (enabled !== settings.enabled || schedule.trim() !== settings.schedule),
  );

  $effect(() => {
    app.authorizationScope;
    void loadSettings();
  });

  async function loadSettings() {
    const version = ++loadVersion;
    loading = true;
    try {
      const loaded = await requestJson(
        "/api/v1/admin/integrity",
        integritySettingsSchema,
      );
      if (version !== loadVersion) return;
      settings = loaded;
      enabled = loaded.enabled;
      schedule = loaded.schedule;
    } catch (caught) {
      if (version !== loadVersion) return;
      toast.error(message(caught, "Could not load integrity settings."));
    } finally {
      if (version === loadVersion) loading = false;
    }
  }

  async function saveSettings() {
    if (!cronExplanation.valid || !dirty) return;
    saving = true;
    try {
      const saved = await requestJson(
        "/api/v1/admin/integrity",
        integritySettingsSchema,
        {
          method: "PUT",
          body: jsonBody({ enabled, schedule: schedule.trim() }),
        },
      );
      settings = saved;
      enabled = saved.enabled;
      schedule = saved.schedule;
      toast.success("Integrity settings saved.");
    } catch (caught) {
      toast.error(message(caught, "Could not save integrity settings."));
    } finally {
      saving = false;
    }
  }

  function message(caught: unknown, fallback: string) {
    return caught instanceof ApiFailure || caught instanceof Error
      ? caught.message
      : fallback;
  }

  function formatDate(value: string) {
    return new Intl.DateTimeFormat(undefined, {
      dateStyle: "medium",
      timeStyle: "short",
    }).format(new Date(value));
  }
</script>

<Card.Root data-integrity-settings>
  <Card.Header class="border-b">
    <div class="flex items-center gap-3">
      <ShieldCheck class="size-4 text-muted-foreground" />
      <div>
        <Card.Title>Repository integrity checks</Card.Title>
        <Card.Description>
          Run strict Git object checks and verify stored LFS objects, release
          assets, and issue attachments.
        </Card.Description>
      </div>
    </div>
  </Card.Header>
  <Card.Content>
    {#if loading}
      <div class="flex min-h-28 items-center justify-center" aria-label="Loading integrity settings">
        <Spinner class="size-5 animate-spin text-muted-foreground" />
      </div>
    {:else}
      <form
        onsubmit={(event) => {
          event.preventDefault();
          void saveSettings();
        }}
      >
        <Field.Group>
          <label class="flex items-start gap-3 rounded-lg border bg-muted/20 p-4">
            <Checkbox class="mt-0.5" bind:checked={enabled} />
            <span class="grid gap-1">
              <span class="text-sm font-medium">Run automatic checks</span>
              <span class="text-xs leading-5 text-muted-foreground">
                Disable this to stop scheduled checks without losing the saved
                schedule.
              </span>
            </span>
          </label>

          <Field.Field data-invalid={!cronExplanation.valid}>
            <Field.Label for="integrity-check-schedule">UTC cron schedule</Field.Label>
            <Input
              id="integrity-check-schedule"
              class="font-mono"
              bind:value={schedule}
              maxlength={128}
              aria-invalid={!cronExplanation.valid}
              autocomplete="off"
              spellcheck="false"
              placeholder="0 3 * * *"
            />
            <Field.Description>
              Five fields use minute, hour, day of month, month, and day of
              week. Six- and seven-field expressions may include seconds and
              year.
            </Field.Description>
          </Field.Field>

          <Alert.Root variant={cronExplanation.valid ? "default" : "destructive"}>
            <CalendarClock />
            <Alert.Title>
              {cronExplanation.valid ? "Schedule translation" : "Invalid cron"}
            </Alert.Title>
            <Alert.Description>{cronExplanation.description}</Alert.Description>
          </Alert.Root>

          <div class="flex flex-wrap items-end justify-between gap-4 border-t pt-5">
            <dl class="grid gap-1 text-sm">
              <dt class="text-xs text-muted-foreground">Last completed check</dt>
              <dd class="font-medium">
                {settings?.last_checked_at
                  ? formatDate(settings.last_checked_at)
                  : "No completed checks yet"}
              </dd>
              {#if settings?.last_result}
                <dd class="text-xs text-muted-foreground">{settings.last_result}</dd>
              {/if}
            </dl>
            <Button type="submit" disabled={saving || !dirty || !cronExplanation.valid}>
              {#if saving}
                <Spinner data-icon="inline-start" class="animate-spin" />
                Saving…
              {:else}
                Save settings
              {/if}
            </Button>
          </div>
        </Field.Group>
      </form>
    {/if}
  </Card.Content>
</Card.Root>
