<script lang="ts">
  import Activity from "@lucide/svelte/icons/activity";
  import Clipboard from "@lucide/svelte/icons/clipboard";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import ShieldCheck from "@lucide/svelte/icons/shield-check";
  import UserPlus from "@lucide/svelte/icons/user-plus";

  import { Button } from "$lib/components/ui/button/index.js";
  import type { AdminSettingsState } from "$lib/settings/admin-settings-state.svelte.js";

  let {
    state,
    view,
  }: {
    state: AdminSettingsState;
    view: "access" | "activity";
  } = $props();
  const inputClass =
    "w-full rounded-md border bg-background px-3 py-2 text-sm outline-none focus:border-ring focus:ring-2 focus:ring-ring/20";

  const actionLabels: Record<string, string> = {
    "account.password.update": "Password updated",
    "account.register": "Account registered",
    "account.username.update": "Username updated",
    "admin.bootstrap": "Administrator created",
    "api_token.create": "API token created",
    "api_token.revoke": "API token revoked",
    "auth.login.passkey": "Signed in with passkey",
    "auth.login.password": "Signed in with password",
    "auth.logout": "Signed out",
    "instance.settings.update": "Instance settings updated",
    "invitation.create": "Invitation created",
    "organization.create": "Organization created",
    "organization.member.add": "Organization member added",
    "organization.member.remove": "Organization member removed",
    "oauth_application.authorize": "OAuth application authorized",
    "oauth_application.create": "OAuth application created",
    "oauth_application.delete": "OAuth application revoked",
    "passkey.create": "Passkey created",
    "passkey.delete": "Passkey removed",
    "repository.collaborator.add": "Repository collaborator added",
    "repository.collaborator.remove": "Repository collaborator removed",
    "repository.create": "Repository created",
    "repository.push": "Repository pushed",
    "ssh_key.create": "SSH key created",
    "ssh_key.delete": "SSH key removed",
  };

  function actionLabel(action: string): string {
    return (
      actionLabels[action] ??
      action
        .split(/[._]/u)
        .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
        .join(" ")
    );
  }
</script>

{#if state.error}
  <p
    class="rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive"
  >
    {state.error}
  </p>
{/if}

<div class="grid gap-6 lg:grid-cols-2">
  {#if view === "access"}
    <section class="overflow-hidden rounded-xl border bg-card/40 shadow-sm">
      <header class="flex items-center gap-3 border-b px-5 py-4">
        <ShieldCheck class="size-4 text-muted-foreground" />
        <div>
          <h2 class="text-sm font-semibold">Registration</h2>
          <p class="mt-0.5 text-xs text-muted-foreground">
            Closed after initial setup.
          </p>
        </div>
      </header>
      <div class="flex items-center justify-between gap-5 p-5 text-sm">
        <div>
          <p class="font-medium">Public registration disabled</p>
          <p class="mt-1 text-xs text-muted-foreground">
            The first account is the administrator. Additional accounts require
            an invitation.
          </p>
        </div>
        <span
          class="rounded-full border px-2.5 py-1 text-xs text-muted-foreground"
          >Locked</span
        >
      </div>
    </section>
  <section class="overflow-hidden rounded-xl border bg-card/40 shadow-sm">
    <header class="flex items-center gap-3 border-b px-5 py-4">
      <UserPlus class="size-4 text-muted-foreground" />
      <div>
        <h2 class="text-sm font-semibold">Invite a user</h2>
        <p class="mt-0.5 text-xs text-muted-foreground">
          Send the token over a private channel.
        </p>
      </div>
    </header>
    <div class="p-5">
      {#if state.invitation}
        <div class="mb-4 rounded-md border bg-muted p-3">
          <div class="flex items-start justify-between gap-3">
            <code class="min-w-0 break-all text-sm">{state.invitation}</code>
            <Button
              variant="ghost"
              size="icon-sm"
              onclick={() =>
                navigator.clipboard.writeText(state.invitation ?? "")}
              aria-label="Copy invitation token"
            >
              <Clipboard class="size-3.5" />
            </Button>
          </div>
          <p class="mt-2 text-xs text-muted-foreground">
            Register at /register?token=&lt;token&gt;.
          </p>
        </div>
      {/if}
      <form
        class="flex items-end gap-2"
        onsubmit={(event) => {
          event.preventDefault();
          void state.createInvitation();
        }}
      >
        <label class="grid flex-1 gap-1.5 text-sm font-medium">
          Expires in hours
          <input
            class={inputClass}
            type="number"
            min="1"
            max="720"
            bind:value={state.invitationHours}
          />
        </label>
        <Button type="submit" disabled={state.working}>Create invitation</Button
        >
      </form>
    </div>
  </section>
  {/if}

  {#if view === "activity"}
  <section class="overflow-hidden rounded-xl border bg-card/40 shadow-sm lg:col-span-2">
    <header class="flex items-center justify-between gap-3 border-b px-5 py-4">
      <div class="flex items-center gap-3">
        <Activity class="size-4 text-muted-foreground" />
        <div>
          <h2 class="text-sm font-semibold">Instance activity</h2>
          <p class="mt-0.5 text-xs text-muted-foreground">
            Repository, authentication, and administration events.
          </p>
        </div>
      </div>
      <Button
        variant="outline"
        size="sm"
        disabled={state.working}
        onclick={() => void state.initialize()}
      >
        <RefreshCw
          class={state.working ? "size-3.5 animate-spin" : "size-3.5"}
        />
        Refresh
      </Button>
    </header>
    <ul class="max-h-[32rem] divide-y overflow-auto px-5" aria-live="polite">
      {#each state.auditEvents as event (event.id)}
        <li class="grid gap-1 py-3">
          <div class="flex items-baseline justify-between gap-4">
            <span class="font-medium">{actionLabel(event.action)}</span>
            <time class="shrink-0 text-xs text-muted-foreground">
              {new Date(event.created_at).toLocaleString()}
            </time>
          </div>
          <p class="text-xs text-muted-foreground">
            {event.actor_username ?? "System"}
            {#if event.target}
              <span> · {event.target}</span>
            {/if}
          </p>
        </li>
      {:else}
        <li class="py-5 text-sm text-muted-foreground">
          No instance activity yet.
        </li>
      {/each}
    </ul>
  </section>
  {/if}
</div>
