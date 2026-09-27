<script lang="ts">
  import Bell from "@lucide/svelte/icons/bell";
  import { onMount } from "svelte";
  import { toast } from "svelte-sonner";

  import * as Card from "$lib/components/ui/card/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import {
    notificationPreferencesSchema,
    type NotificationPreferences,
  } from "$lib/api/email.js";
  import { jsonBody, requestJson } from "$lib/api/transport.js";
  import { errorMessage } from "$lib/repository/state/shared.js";

  type PreferenceKey = "issues" | "issue_comments" | "action_failures";

  const options: { key: PreferenceKey; label: string; description: string }[] =
    [
      {
        key: "issues",
        label: "Issues",
        description:
          "New issues in repositories you own, and issues assigned to you.",
      },
      {
        key: "issue_comments",
        label: "Issue comments",
        description:
          "Comments on issues you opened, are assigned to, or whose repository you own.",
      },
      {
        key: "action_failures",
        label: "Failed workflow runs",
        description: "Actions runs triggered by your pushes that fail.",
      },
    ];

  let preferences = $state.raw<NotificationPreferences | null>(null);
  let loadError = $state<string | null>(null);
  let saving = $state<PreferenceKey | null>(null);

  onMount(async () => {
    try {
      preferences = await requestJson(
        "/api/v1/me/notifications",
        notificationPreferencesSchema,
      );
    } catch (caught) {
      loadError = errorMessage(caught);
    }
  });

  async function toggle(key: PreferenceKey, enabled: boolean) {
    if (!preferences) return;
    const previous = preferences;
    preferences = { ...preferences, [key]: enabled };
    saving = key;
    try {
      preferences = await requestJson(
        "/api/v1/me/notifications",
        notificationPreferencesSchema,
        {
          method: "PUT",
          body: jsonBody({
            issues: preferences.issues,
            issue_comments: preferences.issue_comments,
            action_failures: preferences.action_failures,
          }),
        },
      );
    } catch (caught) {
      preferences = previous;
      toast.error(errorMessage(caught));
    } finally {
      saving = null;
    }
  }
</script>

<section
  class="grid gap-5 py-(--card-spacing) md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10"
  aria-labelledby="notifications-heading"
>
  <Card.Header class="flex flex-row items-start gap-3">
    <Bell class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
    <div>
      <Card.Title id="notifications-heading" role="heading" aria-level={2}>
        Notifications
      </Card.Title>
      <Card.Description class="mt-1 max-w-xs leading-5">
        Choose which emails Gitadel sends to your verified address. You are
        never emailed about your own actions.
      </Card.Description>
    </div>
  </Card.Header>

  <Card.Content>
    {#if loadError}
      <p class="text-sm text-destructive">{loadError}</p>
    {:else if !preferences}
      <p class="text-sm text-muted-foreground">Loading notification settings…</p>
    {:else}
      <ul class="grid max-w-2xl divide-y rounded-lg border">
        {#each options as option (option.key)}
          <li>
            <label
              class="flex items-center justify-between gap-4 px-4 py-3"
              for={`notification-${option.key}`}
            >
              <span class="grid gap-0.5">
                <span class="text-sm font-medium">{option.label}</span>
                <span class="text-xs leading-5 text-muted-foreground"
                  >{option.description}</span
                >
              </span>
              <Switch
                id={`notification-${option.key}`}
                checked={preferences[option.key]}
                disabled={saving !== null}
                onCheckedChange={(checked) => void toggle(option.key, checked)}
              />
            </label>
          </li>
        {/each}
      </ul>
    {/if}
  </Card.Content>
</section>
