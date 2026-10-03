<!--
  The addresses the user writes commits under, which make those commits count
  on their profile, and the name and address commits made in the browser use.
-->
<script lang="ts">
  import GitCommitHorizontal from "@lucide/svelte/icons/git-commit-horizontal";
  import X from "@lucide/svelte/icons/x";
  import { onMount } from "svelte";
  import { toast } from "svelte-sonner";

  import {
    commitIdentitySchema,
    type CommitIdentity,
  } from "$lib/api/profile.js";
  import {
    errorMessage,
    jsonBody,
    requestEmpty,
    requestJson,
  } from "$lib/api/transport.js";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Select from "$lib/components/ui/select/index.js";

  const DEFAULT_EMAIL = "";

  let identity = $state.raw<CommitIdentity | null>(null);
  let loadError = $state<string | null>(null);
  let newEmail = $state("");
  let name = $state("");
  let primaryEmail = $state(DEFAULT_EMAIL);
  let working = $state(false);

  /** Addresses browser commits can be authored with: verified ones only. */
  const choices = $derived(
    identity
      ? [
          ...(identity.account_email ? [identity.account_email] : []),
          ...identity.emails
            .filter((entry) => entry.verification !== "pending")
            .map((entry) => entry.email),
        ]
      : [],
  );
  const authorChanged = $derived(
    identity !== null &&
      (name.trim() !== (identity.name ?? "") ||
        primaryEmail !== (identity.primary_email ?? DEFAULT_EMAIL)),
  );

  function apply(next: CommitIdentity): void {
    identity = next;
    name = next.name ?? "";
    primaryEmail = next.primary_email ?? DEFAULT_EMAIL;
  }

  onMount(async () => {
    try {
      apply(
        await requestJson("/api/v1/me/commit-identity", commitIdentitySchema),
      );
    } catch (caught) {
      loadError = errorMessage(caught);
    }
  });

  async function run(request: () => Promise<CommitIdentity>, done?: string) {
    working = true;
    try {
      apply(await request());
      if (done) toast.success(done);
      return true;
    } catch (caught) {
      toast.error(errorMessage(caught));
      return false;
    } finally {
      working = false;
    }
  }

  async function addEmail(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const email = newEmail.trim();
    if (!email) return;
    const added = await run(
      () =>
        requestJson("/api/v1/me/commit-emails", commitIdentitySchema, {
          method: "POST",
          body: jsonBody({ email }),
        }),
      identity?.verification_required
        ? `Check ${email.toLowerCase()} for a verification link.`
        : undefined,
    );
    if (added) newEmail = "";
  }

  async function resendVerification(email: string): Promise<void> {
    working = true;
    try {
      await requestEmpty(
        `/api/v1/me/commit-emails/${encodeURIComponent(email)}/verification`,
        { method: "POST" },
      );
      toast.success(`Verification email sent to ${email}.`);
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      working = false;
    }
  }

  function removeEmail(email: string): void {
    void run(() =>
      requestJson(
        `/api/v1/me/commit-emails/${encodeURIComponent(email)}`,
        commitIdentitySchema,
        { method: "DELETE" },
      ),
    );
  }

  function saveAuthor(event: SubmitEvent): void {
    event.preventDefault();
    void run(
      () =>
        requestJson("/api/v1/me/commit-identity", commitIdentitySchema, {
          method: "PUT",
          body: jsonBody({
            name: name.trim() || null,
            primary_email: primaryEmail || null,
          }),
        }),
      "Commit author saved.",
    );
  }
</script>

<section
  class="grid gap-5 py-(--card-spacing) md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10"
  aria-labelledby="commit-identity-heading"
>
  <Card.Header class="flex flex-row items-start gap-3">
    <GitCommitHorizontal
      class="mt-0.5 size-4 shrink-0 text-muted-foreground"
    />
    <div>
      <Card.Title id="commit-identity-heading" role="heading" aria-level={2}>
        Commit emails
      </Card.Title>
      <Card.Description class="mt-1 max-w-xs leading-5">
        Commits written with these addresses count on your profile's activity.
        Your account email and commits signed with your SSH keys count too.
      </Card.Description>
      {#if identity}
        <Card.Description class="mt-2 max-w-xs leading-5">
          {#if identity.verification_required}
            A new address counts only after you open the link sent to it, so
            nobody can claim commits written with someone else's address.
          {:else}
            This instance does not send email, so addresses count as soon as
            they are added.
          {/if}
        </Card.Description>
      {/if}
    </div>
  </Card.Header>

  <Card.Content class="grid max-w-2xl gap-6">
    {#if loadError}
      <p class="text-sm text-destructive">{loadError}</p>
    {:else if !identity}
      <p class="text-sm text-muted-foreground">Loading commit emails…</p>
    {:else}
      <div class="grid gap-3">
        <ul class="motion-list grid divide-y rounded-lg border">
          {#if identity.account_email}
            <li class="flex items-center gap-3 px-4 py-2.5 text-sm">
              <span class="min-w-0 flex-1 truncate font-mono text-xs"
                >{identity.account_email}</span
              >
              <Badge variant="outline">Account email</Badge>
            </li>
          {/if}
          {#each identity.emails as entry (entry.email)}
            <li class="flex items-center gap-3 py-1.5 pr-1.5 pl-4 text-sm">
              <span class="min-w-0 flex-1 truncate font-mono text-xs"
                >{entry.email}</span
              >
              {#if entry.verification === "pending"}
                <Badge variant="outline">Pending</Badge>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={working}
                  onclick={() => void resendVerification(entry.email)}
                >
                  Resend
                </Button>
              {:else if entry.verification === "verified"}
                <Badge variant="secondary">Verified</Badge>
              {/if}
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={`Remove ${entry.email}`}
                disabled={working}
                onclick={() => removeEmail(entry.email)}
              >
                <X />
              </Button>
            </li>
          {:else}
            {#if !identity.account_email}
              <li class="px-4 py-3 text-sm text-muted-foreground">
                No commit emails yet. Add the address from your
                <code class="font-mono text-xs">git config user.email</code>.
              </li>
            {/if}
          {/each}
        </ul>
        <form class="flex gap-2" onsubmit={addEmail}>
          <Input
            type="email"
            placeholder="you@example.com"
            aria-label="Commit email to add"
            bind:value={newEmail}
            disabled={working}
          />
          <Button
            type="submit"
            variant="outline"
            disabled={working || !newEmail.trim()}>Add</Button
          >
        </form>
      </div>

      <form class="grid gap-4 border-t pt-5" onsubmit={saveAuthor}>
        <div>
          <p class="text-sm font-medium">Commits made in the browser</p>
          <p class="mt-1 text-xs leading-5 text-muted-foreground">
            Edits and new files committed here are authored as
            <span class="font-mono text-foreground"
              >{identity.author_name} &lt;{identity.author_email}&gt;</span
            >.
          </p>
        </div>
        <div class="grid gap-4 sm:grid-cols-2">
          <Field.Field>
            <Field.Label for="commit-author-name">Name</Field.Label>
            <Input
              id="commit-author-name"
              placeholder={identity.author_name}
              maxlength={100}
              bind:value={name}
              disabled={working}
            />
          </Field.Field>
          <Field.Field>
            <Field.Label for="commit-author-email">Email</Field.Label>
            <Select.Root
              type="single"
              bind:value={primaryEmail}
              disabled={working}
            >
              <Select.Trigger id="commit-author-email" class="w-full">
                <span class="truncate">
                  {primaryEmail ||
                    (identity.account_email ?? "Instance default")}
                </span>
              </Select.Trigger>
              <Select.Content>
                <Select.Item value={DEFAULT_EMAIL}>
                  {identity.account_email
                    ? "Account email"
                    : "Instance default"}
                </Select.Item>
                {#each choices as choice (choice)}
                  <Select.Item value={choice}>{choice}</Select.Item>
                {/each}
              </Select.Content>
            </Select.Root>
          </Field.Field>
        </div>
        <div>
          <Button
            type="submit"
            size="sm"
            disabled={working || !authorChanged}>Save author</Button
          >
        </div>
      </form>
    {/if}
  </Card.Content>
</section>
