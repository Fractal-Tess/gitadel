<script lang="ts">
  import MailX from "@lucide/svelte/icons/mail-x";
  import { toast } from "svelte-sonner";
  import { z } from "zod";

  import {
    pendingInvitationSchema,
    type PendingInvitation,
  } from "$lib/api/admin-users.js";
  import { ApiFailure, requestEmpty, requestJson } from "$lib/api/transport.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";

  // Bumped by the parent whenever it creates an invitation.
  let { version }: { version: unknown } = $props();

  const timestampFormatter = new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });

  let invitations = $state.raw<PendingInvitation[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let revokingId = $state<string | null>(null);

  $effect(() => {
    void version;
    const controller = new AbortController();
    loading = true;
    requestJson("/api/v1/invitations", z.array(pendingInvitationSchema), {
      signal: controller.signal,
    })
      .then((rows) => {
        invitations = rows;
        error = null;
      })
      .catch((caught: unknown) => {
        if (!controller.signal.aborted) error = messageOf(caught);
      })
      .finally(() => {
        if (!controller.signal.aborted) loading = false;
      });
    return () => controller.abort();
  });

  function messageOf(caught: unknown): string {
    return caught instanceof ApiFailure || caught instanceof Error
      ? caught.message
      : "The request failed.";
  }

  async function revoke(invitation: PendingInvitation) {
    revokingId = invitation.id;
    try {
      await requestEmpty(
        `/api/v1/invitations/${encodeURIComponent(invitation.id)}`,
        { method: "DELETE" },
      );
      invitations = invitations.filter((item) => item.id !== invitation.id);
      toast.success("Invitation revoked.");
    } catch (caught) {
      toast.error(messageOf(caught));
    } finally {
      revokingId = null;
    }
  }
</script>

<section
  class="overflow-hidden rounded-xl border bg-card/40 shadow-sm lg:col-span-2"
>
  <header class="flex items-center gap-3 border-b px-5 py-4">
    <MailX class="size-4 text-muted-foreground" />
    <div>
      <h2 class="text-sm font-semibold">Pending invitations</h2>
      <p class="mt-0.5 text-xs text-muted-foreground">
        Unused invitations that have not expired. Revoke any that were shared
        by mistake.
      </p>
    </div>
  </header>
  {#if loading && invitations.length === 0}
    <p
      class="flex items-center justify-center gap-2 py-10 text-sm text-muted-foreground"
    >
      <Spinner class="size-4" /> Loading invitations…
    </p>
  {:else if error}
    <p class="px-5 py-6 text-sm text-destructive">{error}</p>
  {:else}
    <ul class="divide-y px-5" aria-busy={loading}>
      {#each invitations as invitation (invitation.id)}
        <li class="flex flex-wrap items-center justify-between gap-3 py-3">
          <div class="min-w-0 text-sm">
            <p class="font-medium">
              Expires {timestampFormatter.format(
                new Date(invitation.expires_at),
              )}
            </p>
            <p class="mt-0.5 text-xs text-muted-foreground">
              Created {timestampFormatter.format(
                new Date(invitation.created_at),
              )}
              {#if invitation.created_by}
                by {invitation.created_by}
              {/if}
            </p>
          </div>
          <Button
            variant="outline"
            size="sm"
            disabled={revokingId === invitation.id}
            onclick={() => void revoke(invitation)}
          >
            Revoke
          </Button>
        </li>
      {:else}
        <li class="py-5 text-sm text-muted-foreground">
          No pending invitations.
        </li>
      {/each}
    </ul>
  {/if}
</section>
