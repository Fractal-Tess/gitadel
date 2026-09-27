<script lang="ts">
  import Clipboard from "@lucide/svelte/icons/clipboard";
  import ShieldCheck from "@lucide/svelte/icons/shield-check";
  import { toast } from "svelte-sonner";

  import {
    recoveryCodesSchema,
    twoFactorEnrollmentSchema,
    twoFactorStatusSchema,
    type TwoFactorEnrollment,
    type TwoFactorStatus,
  } from "$lib/api/auth.js";
  import {
    ApiFailure,
    jsonBody,
    requestEmpty,
    requestJson,
  } from "$lib/api/transport.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";

  type ProtectedAction = "disable" | "regenerate";

  const dateFormatter = new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
  });

  let status = $state.raw<TwoFactorStatus | null>(null);
  let loadError = $state<string | null>(null);
  let enrollment = $state.raw<TwoFactorEnrollment | null>(null);
  let enrollmentCode = $state("");
  let recoveryCodes = $state.raw<string[] | null>(null);
  let working = $state(false);
  let actionDialogOpen = $state(false);
  let action = $state<ProtectedAction>("disable");
  let actionPassword = $state("");
  let actionCode = $state("");

  const qrImage = $derived(
    enrollment
      ? `data:image/svg+xml;charset=utf-8,${encodeURIComponent(enrollment.qr_svg)}`
      : null,
  );

  $effect(() => {
    void loadStatus();
  });

  function messageOf(caught: unknown): string {
    return caught instanceof ApiFailure || caught instanceof Error
      ? caught.message
      : "The request failed.";
  }

  async function loadStatus() {
    try {
      status = await requestJson("/api/v1/me/two-factor", twoFactorStatusSchema);
      loadError = null;
    } catch (caught) {
      loadError = messageOf(caught);
    }
  }

  async function run(task: () => Promise<void>) {
    working = true;
    try {
      await task();
    } catch (caught) {
      toast.error(messageOf(caught));
    } finally {
      working = false;
    }
  }

  function startEnrollment() {
    void run(async () => {
      recoveryCodes = null;
      enrollmentCode = "";
      enrollment = await requestJson(
        "/api/v1/me/two-factor/enroll",
        twoFactorEnrollmentSchema,
        { method: "POST" },
      );
    });
  }

  function confirmEnrollment() {
    void run(async () => {
      const response = await requestJson(
        "/api/v1/me/two-factor/confirm",
        recoveryCodesSchema,
        { method: "POST", body: jsonBody({ code: enrollmentCode }) },
      );
      enrollment = null;
      enrollmentCode = "";
      recoveryCodes = response.recovery_codes;
      toast.success("Two-factor authentication is on.");
      await loadStatus();
    });
  }

  function openAction(next: ProtectedAction) {
    action = next;
    actionPassword = "";
    actionCode = "";
    actionDialogOpen = true;
  }

  function submitAction() {
    const body = jsonBody({
      ...(actionPassword && { password: actionPassword }),
      ...(actionCode.trim() && { code: actionCode.trim() }),
    });
    void run(async () => {
      if (action === "disable") {
        await requestEmpty("/api/v1/me/two-factor/disable", {
          method: "POST",
          body,
        });
        recoveryCodes = null;
        toast.success("Two-factor authentication is off.");
      } else {
        const response = await requestJson(
          "/api/v1/me/two-factor/recovery-codes",
          recoveryCodesSchema,
          { method: "POST", body },
        );
        recoveryCodes = response.recovery_codes;
        toast.success("New recovery codes generated. The old ones stop working.");
      }
      actionDialogOpen = false;
      actionPassword = "";
      actionCode = "";
      await loadStatus();
    });
  }

  async function copyRecoveryCodes() {
    if (!recoveryCodes) return;
    try {
      await navigator.clipboard.writeText(recoveryCodes.join("\n"));
      toast.success("Recovery codes copied");
    } catch {
      toast.error("Could not copy the recovery codes", {
        description: "Select the codes and copy them manually.",
      });
    }
  }
</script>

<section
  class="grid gap-5 py-(--card-spacing) md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10"
  aria-labelledby="two-factor-heading"
>
  <Card.Header class="flex flex-row items-start gap-3">
    <ShieldCheck class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
    <div>
      <Card.Title id="two-factor-heading" role="heading" aria-level={2}>
        Two-factor authentication
      </Card.Title>
      <Card.Description class="mt-1 max-w-xs leading-5">
        Ask for a code from an authenticator app after your password. Passkey
        sign-in already counts as two factors.
      </Card.Description>
    </div>
  </Card.Header>

  <Card.Content class="grid max-w-2xl gap-5">
    {#if loadError}
      <p class="text-sm text-destructive">{loadError}</p>
    {:else if !status}
      <p class="text-sm text-muted-foreground">Loading…</p>
    {:else}
      {#if recoveryCodes}
        <div class="grid gap-3 rounded-lg border bg-muted/30 p-4">
          <div class="flex items-start justify-between gap-3">
            <div>
              <p class="text-sm font-medium">Save your recovery codes</p>
              <p class="mt-1 text-xs leading-5 text-muted-foreground">
                Each code signs you in once if you lose your authenticator. They
                are shown only now.
              </p>
            </div>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label="Copy recovery codes"
              onclick={() => void copyRecoveryCodes()}
            >
              <Clipboard class="size-3.5" />
            </Button>
          </div>
          <ul class="grid grid-cols-2 gap-x-6 gap-y-1 font-mono text-sm">
            {#each recoveryCodes as code (code)}
              <li>{code}</li>
            {/each}
          </ul>
          <Button
            class="w-fit"
            size="sm"
            variant="outline"
            onclick={() => (recoveryCodes = null)}
          >
            I saved them
          </Button>
        </div>
      {/if}

      {#if status.enabled}
        <div class="flex flex-wrap items-center justify-between gap-3">
          <div class="text-sm">
            <p class="font-medium">On</p>
            <p class="mt-0.5 text-xs text-muted-foreground">
              {#if status.confirmed_at}
                Enabled {dateFormatter.format(new Date(status.confirmed_at))} ·
              {/if}
              {status.recovery_codes_remaining} recovery
              {status.recovery_codes_remaining === 1 ? "code" : "codes"} left
            </p>
          </div>
          <div class="flex flex-wrap gap-2">
            <Button
              size="sm"
              variant="outline"
              disabled={working}
              onclick={() => openAction("regenerate")}
            >
              New recovery codes
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={working}
              onclick={() => openAction("disable")}
            >
              Turn off
            </Button>
          </div>
        </div>
      {:else if enrollment}
        <div class="grid gap-4 sm:grid-cols-[auto_minmax(0,1fr)]">
          {#if qrImage}
            <img
              class="size-44 rounded-md border bg-white p-2"
              src={qrImage}
              alt="QR code for your authenticator app"
            />
          {/if}
          <div class="grid content-start gap-3 text-sm">
            <p class="text-muted-foreground">
              Scan the code with an authenticator app, or enter this key by
              hand:
            </p>
            <code class="break-all rounded-md border bg-muted/40 p-2 text-xs"
              >{enrollment.secret}</code
            >
            <form
              class="grid gap-3"
              onsubmit={(event) => {
                event.preventDefault();
                confirmEnrollment();
              }}
            >
              <Field.Field>
                <Field.Label for="two-factor-enrollment-code">
                  Code from the app
                </Field.Label>
                <Input
                  id="two-factor-enrollment-code"
                  bind:value={enrollmentCode}
                  autocomplete="one-time-code"
                  inputmode="numeric"
                  maxlength={6}
                  required
                />
              </Field.Field>
              <div class="flex gap-2">
                <Button
                  type="submit"
                  disabled={working || enrollmentCode.trim().length !== 6}
                >
                  Turn on
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  disabled={working}
                  onclick={() => (enrollment = null)}
                >
                  Cancel
                </Button>
              </div>
            </form>
          </div>
        </div>
      {:else}
        <div class="flex flex-wrap items-center justify-between gap-3">
          <p class="text-sm text-muted-foreground">
            Two-factor authentication is off.
          </p>
          <Button disabled={working} onclick={startEnrollment}>
            Set up authenticator
          </Button>
        </div>
      {/if}
    {/if}
  </Card.Content>
</section>

<Dialog.Root bind:open={actionDialogOpen}>
  <Dialog.Content>
    <Dialog.Header>
      <Dialog.Title>
        {action === "disable"
          ? "Turn off two-factor authentication"
          : "Generate new recovery codes"}
      </Dialog.Title>
      <Dialog.Description>
        Confirm with your password or a current code from your authenticator.
      </Dialog.Description>
    </Dialog.Header>
    <form
      class="grid gap-4"
      onsubmit={(event) => {
        event.preventDefault();
        submitAction();
      }}
    >
      <Field.Field>
        <Field.Label for="two-factor-action-password">Password</Field.Label>
        <Input
          id="two-factor-action-password"
          type="password"
          autocomplete="current-password"
          bind:value={actionPassword}
        />
      </Field.Field>
      <Field.Field>
        <Field.Label for="two-factor-action-code">
          Or authentication code
        </Field.Label>
        <Input
          id="two-factor-action-code"
          autocomplete="one-time-code"
          inputmode="numeric"
          maxlength={6}
          bind:value={actionCode}
        />
      </Field.Field>
      <Dialog.Footer>
        <Button
          type="submit"
          variant={action === "disable" ? "destructive" : "default"}
          disabled={working || (!actionPassword && !actionCode.trim())}
        >
          {action === "disable" ? "Turn off" : "Generate codes"}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
