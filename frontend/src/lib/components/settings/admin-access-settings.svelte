<script lang="ts">
  import { Clipboard, UserPlus, UsersRound } from "lucide-svelte";

  import { Button } from "$lib/components/ui/button/index.js";
  import type { AdminSettingsState } from "$lib/settings/admin-settings-state.svelte.js";

  let { state }: { state: AdminSettingsState } = $props();
  const inputClass =
    "w-full rounded-md border bg-background px-3 py-2 text-sm outline-none focus:border-ring focus:ring-2 focus:ring-ring/20";
</script>

{#if state.error}
  <p class="rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
    {state.error}
  </p>
{/if}

<div class="grid gap-6 lg:grid-cols-2">
  <section class="rounded-md border bg-card/25">
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
              onclick={() => navigator.clipboard.writeText(state.invitation ?? "")}
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
        <Button type="submit" disabled={state.working}>Create invitation</Button>
      </form>
    </div>
  </section>

  <section class="rounded-md border bg-card/25">
    <header class="flex items-center gap-3 border-b px-5 py-4">
      <UsersRound class="size-4 text-muted-foreground" />
      <h2 class="text-sm font-semibold">Recent audit events</h2>
    </header>
    <ul class="max-h-96 space-y-2 overflow-auto p-5">
      {#each state.auditEvents as event (event.id)}
        <li class="border-b pb-2 text-sm last:border-0">
          <span class="font-medium">{event.action}</span>
          {#if event.target}
            <span class="text-muted-foreground"> · {event.target}</span>
          {/if}
          <time class="block text-xs text-muted-foreground">
            {new Date(event.created_at).toLocaleString()}
          </time>
        </li>
      {:else}
        <li class="text-sm text-muted-foreground">No audit events yet.</li>
      {/each}
    </ul>
  </section>
</div>
