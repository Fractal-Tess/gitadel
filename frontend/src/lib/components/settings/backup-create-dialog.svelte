<script lang="ts">
  import CloudUpload from "@lucide/svelte/icons/cloud-upload";

  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import type { BackupState } from "$lib/settings/backup-state.svelte.js";

  let { backup }: { backup: BackupState } = $props();
</script>

<Dialog.Root bind:open={backup.createDialogOpen}>
  <Dialog.Content class="ring-foreground/20 sm:max-w-md">
    <Dialog.Header>
      <Dialog.Title>Create backup</Dialog.Title>
      <Dialog.Description>
        Choose the destination first, then optionally name this snapshot.
      </Dialog.Description>
    </Dialog.Header>
    <form
      class="grid gap-4"
      onsubmit={(event) => {
        event.preventDefault();
        void backup.createBackup();
      }}
    >
      <Field.Field>
        <Field.Label for="backup-provider">Storage provider</Field.Label>
        <Select.Root type="single" bind:value={backup.createProviderId}>
          <Select.Trigger id="backup-provider" class="w-full">
            {backup.providers.find(
              (provider) => provider.id === backup.createProviderId,
            )?.name ?? "Choose a storage provider"}
          </Select.Trigger>
          <Select.Content>
            <Select.Group>
              <Select.Label>Backup destinations</Select.Label>
              {#each backup.providers as provider (provider.id)}
                <Select.Item value={provider.id} label={provider.name}>
                  {provider.name}
                </Select.Item>
              {/each}
            </Select.Group>
          </Select.Content>
        </Select.Root>
      </Field.Field>
      <Field.Field>
        <Field.Label for="backup-name">
          Backup name <span class="font-normal text-muted-foreground"
            >(optional)</span
          >
        </Field.Label>
        <Input
          id="backup-name"
          bind:value={backup.backupName}
          maxlength={80}
          pattern="[A-Za-z0-9 _.-]+"
          placeholder="Before platform upgrade"
        />
      </Field.Field>
      <Dialog.Footer>
        <Button
          type="button"
          variant="ghost"
          onclick={() => (backup.createDialogOpen = false)}
          disabled={backup.creating}
        >
          Cancel
        </Button>
        <Button
          type="submit"
          disabled={backup.creating || !backup.createProviderId}
        >
          {#if backup.creating}
            <Spinner class="size-4" data-icon="inline-start" /> Creating backup…
          {:else}
            <CloudUpload class="size-4" data-icon="inline-start" /> Create backup
          {/if}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
