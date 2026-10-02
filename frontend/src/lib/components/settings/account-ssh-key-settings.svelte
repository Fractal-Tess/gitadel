<script lang="ts">
  import Clipboard from "@lucide/svelte/icons/clipboard";
  import Terminal from "@lucide/svelte/icons/terminal";
  import { toast } from "svelte-sonner";

  import { copyText } from "$lib/clipboard.js";
  import IntegrationAddCard from "$lib/components/integrations/integration-add-card.svelte";
  import CredentialRemovalDialog, {
    type CredentialRemoval,
  } from "$lib/components/settings/credential-removal-dialog.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import { formatDateTime } from "$lib/format.js";
  import type { CredentialsSettingsState } from "$lib/settings/account/credentials-settings-state.svelte.js";

  let { credentials }: { credentials: CredentialsSettingsState } = $props();
  let sshKeyDialogOpen = $state(false);
  let deleteDialogOpen = $state(false);
  let pendingDeletion = $state<CredentialRemoval | null>(null);

  function updateSshPublicKey(publicKey: string): void {
    credentials.sshPublicKey = publicKey;
    if (credentials.sshKeyName.trim()) return;

    const firstLine = publicKey.trim().split(/\r?\n/u)[0] ?? "";
    const comment = firstLine.match(
      /^(?:ssh-(?:ed25519|rsa)|ecdsa-sha2-\S+|sk-\S+)\s+\S+\s+(.+?)\s*$/u,
    )?.[1];
    if (comment) credentials.sshKeyName = comment;
  }

  async function addSshKey() {
    if (await credentials.addSshKey()) sshKeyDialogOpen = false;
  }

  async function copyPublicKey(publicKey: string) {
    try {
      await copyText(publicKey);
      toast.success("SSH public key copied");
    } catch {
      toast.error("Could not copy the ssh public key", {
        description: "Copy the key from its original file and try again.",
      });
    }
  }

  function requestDeletion(target: CredentialRemoval) {
    pendingDeletion = target;
    deleteDialogOpen = true;
  }

  async function confirmDeletion() {
    const target = pendingDeletion;
    if (!target) return;
    if (await credentials.removeSshKey(target.id)) {
      deleteDialogOpen = false;
      pendingDeletion = null;
    }
  }
</script>

<section class="space-y-6">
  <div class="motion-list grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
    <IntegrationAddCard
      title="New SSH key"
      description="Add a public key for Git authentication and commit signing."
      onclick={() => (sshKeyDialogOpen = true)}
    />

    {#each credentials.sshKeys as key (key.id)}
      <article
        class="flex min-h-72 flex-col rounded-xl border bg-card/40 p-5 shadow-sm transition-colors hover:bg-card/60"
      >
        <header class="flex items-start gap-3">
          <span
            class="grid size-10 shrink-0 place-items-center rounded-lg border bg-muted"
            aria-hidden="true"
          >
            <Terminal class="size-5 text-primary" />
          </span>
          <span class="min-w-0 flex-1">
            <span class="block truncate font-medium">{key.name}</span>
            <span class="mt-0.5 block text-xs text-muted-foreground">
              Authentication and signing
            </span>
          </span>
        </header>
        <code
          class="mt-4 block truncate text-xs text-muted-foreground"
          title={key.fingerprint}
        >
          {key.fingerprint}
        </code>
        <dl class="mt-4 grid grid-cols-2 gap-3 text-xs">
          <div>
            <dt class="text-muted-foreground">Created</dt>
            <dd class="mt-0.5">
              <time datetime={key.created_at}>
                {formatDateTime(key.created_at)}
              </time>
            </dd>
          </div>
          <div>
            <dt class="text-muted-foreground">Last used</dt>
            <dd class="mt-0.5">
              {#if key.last_used_at}
                <time datetime={key.last_used_at}>
                  {formatDateTime(key.last_used_at)}
                </time>
              {:else}
                Never
              {/if}
            </dd>
          </div>
        </dl>
        <div class="mt-auto flex flex-wrap justify-end gap-2 pt-4">
          <Button
            size="sm"
            variant="outline"
            class="gap-2"
            onclick={() => copyPublicKey(key.public_key)}
          >
            <Clipboard class="size-3.5" />Copy public key
          </Button>
          <Button
            size="sm"
            variant="outline"
            onclick={() =>
              requestDeletion({
                kind: "ssh-key",
                id: key.id,
                name: key.name,
              })}
          >
            Remove
          </Button>
        </div>
      </article>
    {/each}
  </div>

  <Dialog.Root bind:open={sshKeyDialogOpen}>
    <Dialog.Content class="ring-foreground/20 sm:max-w-lg">
      <Dialog.Header>
        <Dialog.Title>Add an SSH key</Dialog.Title>
        <Dialog.Description>
          Add a public key for SSH authentication and signed commit
          verification.
        </Dialog.Description>
      </Dialog.Header>
      <form
        class="grid min-w-0 gap-4"
        onsubmit={(event) => {
          event.preventDefault();
          void addSshKey();
        }}
      >
        <Field.Field>
          <Field.Label for="ssh-key-name">Key name</Field.Label>
          <Input
            id="ssh-key-name"
            bind:value={credentials.sshKeyName}
            placeholder="Work laptop"
            autofocus
            required
          />
        </Field.Field>
        <Field.Field>
          <Field.Label for="ssh-public-key">Public key</Field.Label>
          <Textarea
            id="ssh-public-key"
            class="min-w-0 font-mono text-xs [overflow-wrap:anywhere]"
            rows={5}
            value={credentials.sshPublicKey}
            oninput={(event) => updateSshPublicKey(event.currentTarget.value)}
            placeholder="ssh-ed25519 AAAA…"
            required
          />
          <Field.Description>
            Paste the complete OpenSSH public key, including its key type.
          </Field.Description>
        </Field.Field>
        <Dialog.Footer>
          <Dialog.Close>
            {#snippet child({ props })}
              <Button {...props} type="button" variant="outline">Cancel</Button>
            {/snippet}
          </Dialog.Close>
          <Button type="submit" disabled={credentials.working}>
            {credentials.working ? "Adding…" : "Add SSH key"}
          </Button>
        </Dialog.Footer>
      </form>
    </Dialog.Content>
  </Dialog.Root>
</section>

<CredentialRemovalDialog
  bind:open={deleteDialogOpen}
  target={pendingDeletion}
  working={credentials.working}
  onconfirm={() => void confirmDeletion()}
/>
