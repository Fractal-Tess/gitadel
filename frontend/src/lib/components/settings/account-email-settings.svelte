<script lang="ts">
  import Mail from "@lucide/svelte/icons/mail";
  import { onMount } from "svelte";
  import { toast } from "svelte-sonner";

  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { emailStatusSchema, type EmailStatus } from "$lib/api/email.js";
  import {
    jsonBody,
    requestEmpty,
    requestJson,
  } from "$lib/api/transport.js";
  import { errorMessage } from "$lib/repository/state/shared.js";

  let status = $state.raw<EmailStatus | null>(null);
  let loadError = $state<string | null>(null);
  let draft = $state("");
  let working = $state(false);

  async function load() {
    try {
      status = await requestJson("/api/v1/me/email", emailStatusSchema);
      draft = status.email ?? "";
      loadError = null;
    } catch (caught) {
      loadError = errorMessage(caught);
    }
  }

  onMount(load);

  async function save(event: SubmitEvent) {
    event.preventDefault();
    working = true;
    try {
      status = await requestJson("/api/v1/me/email", emailStatusSchema, {
        method: "PUT",
        body: jsonBody({ email: draft }),
      });
      draft = status.email ?? "";
      toast.success(
        status.verified
          ? "Email address is already verified."
          : "Check your inbox for a verification link.",
      );
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      working = false;
    }
  }

  async function resend() {
    working = true;
    try {
      await requestEmpty("/api/v1/me/email/verification", { method: "POST" });
      toast.success("Verification email sent.");
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      working = false;
    }
  }

  async function remove() {
    working = true;
    try {
      await requestEmpty("/api/v1/me/email", { method: "DELETE" });
      status = status && { ...status, email: null, verified: false };
      draft = "";
      toast.success("Email address removed.");
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      working = false;
    }
  }
</script>

<section
  class="grid gap-5 py-(--card-spacing) md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10"
  aria-labelledby="email-heading"
>
  <Card.Header class="flex flex-row items-start gap-3">
    <Mail class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
    <div>
      <Card.Title id="email-heading" role="heading" aria-level={2}>
        Email
      </Card.Title>
      <Card.Description class="mt-1 max-w-xs leading-5">
        A verified address can reset your password and receive
        notifications.
      </Card.Description>
    </div>
  </Card.Header>

  <Card.Content>
    {#if loadError}
      <p class="text-sm text-destructive">{loadError}</p>
    {:else if !status}
      <p class="text-sm text-muted-foreground">Loading email settings…</p>
    {:else}
      <form class="grid max-w-2xl gap-4" onsubmit={save}>
        <Field.Field>
          <div class="flex items-center gap-2">
            <Field.Label for="account-email">Email address</Field.Label>
            {#if status.email}
              <Badge variant={status.verified ? "secondary" : "outline"}>
                {status.verified ? "Verified" : "Unverified"}
              </Badge>
            {/if}
          </div>
          <Input
            id="account-email"
            type="email"
            autocomplete="email"
            bind:value={draft}
            maxlength={254}
            disabled={working}
            required
          />
          <Field.Description>
            {status.email && !status.verified
              ? "Open the link we emailed to verify this address."
              : "Only you and administrators can see this address."}
          </Field.Description>
        </Field.Field>
        <div class="flex flex-wrap justify-end gap-2">
          {#if status.email}
            <Button
              type="button"
              variant="ghost"
              disabled={working}
              onclick={() => void remove()}
            >
              Remove
            </Button>
          {/if}
          {#if status.email && !status.verified}
            <Button
              type="button"
              variant="outline"
              disabled={working}
              onclick={() => void resend()}
            >
              Resend verification
            </Button>
          {/if}
          <Button
            type="submit"
            disabled={working ||
              !draft.trim() ||
              (draft.trim().toLowerCase() === status.email && status.verified)}
          >
            {working ? "Saving…" : "Save email"}
          </Button>
        </div>
      </form>
    {/if}
  </Card.Content>
</section>
