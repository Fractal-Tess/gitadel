<script lang="ts">
  import Mail from "@lucide/svelte/icons/mail";
  import Send from "@lucide/svelte/icons/send";
  import { onMount } from "svelte";
  import { toast } from "svelte-sonner";

  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import {
    smtpStatusSchema,
    testEmailSchema,
    type SmtpStatus,
  } from "$lib/api/email.js";
  import { ApiFailure, jsonBody, requestJson } from "$lib/api/transport.js";

  let status = $state.raw<SmtpStatus | null>(null);
  let loadError = $state<string | null>(null);
  let recipient = $state("");
  let sending = $state(false);

  const tlsLabels = {
    starttls: "STARTTLS",
    tls: "Implicit TLS",
    none: "Unencrypted",
  } as const;

  function message(caught: unknown, fallback: string) {
    return caught instanceof ApiFailure || caught instanceof Error
      ? caught.message
      : fallback;
  }

  onMount(async () => {
    try {
      status = await requestJson("/api/v1/admin/smtp", smtpStatusSchema);
    } catch (caught) {
      loadError = message(caught, "Could not load email settings.");
    }
  });

  async function sendTest(event: SubmitEvent) {
    event.preventDefault();
    sending = true;
    try {
      const result = await requestJson(
        "/api/v1/admin/smtp/test",
        testEmailSchema,
        {
          method: "POST",
          body: jsonBody(recipient.trim() ? { to: recipient.trim() } : {}),
        },
      );
      toast.success(`Test email accepted for ${result.delivered_to}.`);
    } catch (caught) {
      toast.error(message(caught, "The test email failed."));
    } finally {
      sending = false;
    }
  }
</script>

<Card.Root>
  <Card.Header class="border-b">
    <div class="flex items-center gap-3">
      <Mail class="size-4 text-muted-foreground" />
      <div>
        <Card.Title>Outgoing email</Card.Title>
        <Card.Description>
          SMTP is configured in the server configuration file. Without it,
          password reset, email verification, and notifications stay hidden.
        </Card.Description>
      </div>
    </div>
  </Card.Header>
  <Card.Content>
    {#if loadError}
      <Alert.Root variant="destructive">
        <Alert.Title>Email settings unavailable</Alert.Title>
        <Alert.Description>{loadError}</Alert.Description>
      </Alert.Root>
    {:else if !status}
      <div
        class="flex min-h-28 items-center justify-center"
        aria-label="Loading email settings"
      >
        <Spinner class="size-5 animate-spin text-muted-foreground" />
      </div>
    {:else if !status.configured}
      <Alert.Root>
        <Alert.Title>SMTP is not configured</Alert.Title>
        <Alert.Description>
          Add an <code>[smtp]</code> section to <code>gitadel.toml</code> (or the
          matching <code>GITADEL__SMTP__*</code> environment variables) and restart
          Gitadel. See INSTALL.md for the options.
        </Alert.Description>
      </Alert.Root>
    {:else}
      <div class="grid gap-6">
        <dl class="grid gap-x-6 gap-y-3 text-sm sm:grid-cols-2">
          <div>
            <dt class="text-xs text-muted-foreground">Server</dt>
            <dd class="font-mono">{status.host}:{status.port}</dd>
          </div>
          <div>
            <dt class="text-xs text-muted-foreground">Security</dt>
            <dd>{status.tls ? tlsLabels[status.tls] : "Unknown"}</dd>
          </div>
          <div>
            <dt class="text-xs text-muted-foreground">Sender</dt>
            <dd class="break-all">{status.from}</dd>
          </div>
          <div>
            <dt class="text-xs text-muted-foreground">Authentication</dt>
            <dd>{status.username ? `As ${status.username}` : "None"}</dd>
          </div>
        </dl>

        <form class="grid max-w-xl gap-3 border-t pt-5" onsubmit={sendTest}>
          <Field.Field>
            <Field.Label for="smtp-test-recipient">Send a test email</Field.Label>
            <Input
              id="smtp-test-recipient"
              type="email"
              bind:value={recipient}
              placeholder="Your verified address"
              maxlength={254}
            />
            <Field.Description>
              Leave blank to use the verified address on your account. The
              relay's answer is shown right away.
            </Field.Description>
          </Field.Field>
          <div>
            <Button type="submit" class="gap-2" disabled={sending}>
              <Send class="size-3.5" />
              {sending ? "Sending…" : "Send test email"}
            </Button>
          </div>
        </form>
      </div>
    {/if}
  </Card.Content>
</Card.Root>
