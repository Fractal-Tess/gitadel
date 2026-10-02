<script lang="ts">
  import { onMount } from "svelte";
  import CalendarClock from "@lucide/svelte/icons/calendar-clock";
  import CloudUpload from "@lucide/svelte/icons/cloud-upload";
  import HardDrive from "@lucide/svelte/icons/hard-drive";
  import TriangleAlert from "@lucide/svelte/icons/triangle-alert";

  import IntegrationAddCard from "$lib/components/integrations/integration-add-card.svelte";
  import IntegrationConnectionCard from "$lib/components/integrations/integration-connection-card.svelte";
  import BackupCreateDialog from "$lib/components/settings/backup-create-dialog.svelte";
  import BackupProviderEditor from "$lib/components/settings/backup-provider-editor.svelte";
  import BackupRestorePanel from "$lib/components/settings/backup-restore-panel.svelte";
  import BackupSnapshotList from "$lib/components/settings/backup-snapshot-list.svelte";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { formatDateTime } from "$lib/format.js";
  import {
    BackupState,
    CUSTOM_SCHEDULE,
    SCHEDULE_OPTIONS,
  } from "$lib/settings/backup-state.svelte.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const backup = new BackupState(useAppState());

  onMount(() => {
    void backup.initialize();
    return backup.destroy;
  });
</script>

<section class="flex flex-col gap-6" aria-label="Backups">
  {#if backup.providers.length > 0}
    <div class="flex justify-end">
      <Button
        type="button"
        onclick={backup.openCreateBackup}
        disabled={backup.busy}
      >
        <CloudUpload data-icon="inline-start" /> Create backup
      </Button>
    </div>
  {/if}

  {#if backup.error}
    <Alert.Root variant="destructive">
      <TriangleAlert />
      <Alert.Title>Backup operation failed</Alert.Title>
      <Alert.Description>{backup.error}</Alert.Description>
    </Alert.Root>
  {/if}

  {#if backup.busy && backup.providers.length === 0}
    <p class="flex items-center gap-2 text-sm text-muted-foreground">
      <Spinner />Loading backup providers…
    </p>
  {:else}
    <div class="motion-list grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
      <IntegrationAddCard
        title="Add backup provider"
        description="Connect another filesystem or S3 backup destination."
        onclick={backup.openCreateProvider}
      />
      {#each backup.providers as provider (provider.id)}
        {@const catalogProvider =
          backup.providerCatalog.find(
            (candidate) => candidate.slug === provider.provider,
          ) ?? null}
        <IntegrationConnectionCard
          name={provider.name}
          provider={provider.provider}
          providerName={catalogProvider?.name ?? provider.provider}
          subtitle={provider.provider === "filesystem"
            ? (provider.path ?? "Filesystem")
            : (provider.bucket ?? "S3")}
          description={provider.managed_by_storage
            ? "Storage target managed from Administration → Storage."
            : (catalogProvider?.description ??
              "Configured backup destination.")}
          enabled
          statusLabel={provider.schedule ? "Scheduled" : "Ready"}
          statusHealthy
          detailLabel={provider.schedule
            ? `Next ${provider.next_backup_at ? formatDateTime(provider.next_backup_at) : "run pending"}`
            : null}
          selected={provider.id === backup.selectedProviderId}
          onselect={() => void backup.selectProvider(provider)}
        >
          {#snippet icon()}
            {#if provider.provider === "filesystem"}
              <HardDrive class="size-6 text-primary" />
            {:else}
              <CloudUpload class="size-6 text-primary" />
            {/if}
          {/snippet}
        </IntegrationConnectionCard>
      {/each}
    </div>
  {/if}

  {#if backup.settings}
    {@const selected = backup.settings}
    <div
      class="flex flex-wrap items-center justify-between gap-3 rounded-xl border bg-card/25 px-4 py-3"
    >
      <div class="min-w-0">
        <p class="truncate text-sm font-medium">
          Managing {backup.settings.name}
        </p>
        <p class="mt-0.5 text-xs text-muted-foreground">
          {backup.settings.provider === "filesystem"
            ? backup.settings.path
            : `${backup.settings.bucket} at ${backup.settings.endpoint}`}
        </p>
      </div>
      <div class="flex gap-2">
        {#if !backup.settings.managed_by_storage}
          <Button
            type="button"
            variant="outline"
            size="sm"
            onclick={() => backup.openConfigureProvider(selected)}
          >
            Configure
          </Button>
        {/if}
        <Button
          type="button"
          variant="ghost"
          size="sm"
          class="text-muted-foreground hover:text-destructive"
          onclick={() => backup.requestProviderRemove(selected)}
        >
          Remove
        </Button>
      </div>
    </div>
  {/if}

  {#if backup.settings}
    <Card.Root>
      <Card.Header class="border-b">
        <div class="flex items-center gap-3">
          <CalendarClock class="size-4 text-muted-foreground" />
          <div>
            <Card.Title>Automatic backups</Card.Title>
            <Card.Description>
              Create complete snapshots with {backup.settings.name} on a predefined
              interval or UTC cron schedule.
            </Card.Description>
          </div>
        </div>
      </Card.Header>
      <Card.Content>
        <form
          onsubmit={(event) => {
            event.preventDefault();
            void backup.saveSchedule();
          }}
        >
          <Field.Group>
            <Field.Field>
              <Field.Label for="automatic-backup-schedule">Schedule</Field.Label
              >
              <Select.Root type="single" bind:value={backup.scheduleMode}>
                <Select.Trigger id="automatic-backup-schedule" class="w-full">
                  {SCHEDULE_OPTIONS.find(
                    (option) => option.value === backup.scheduleMode,
                  )?.label ?? "Choose a schedule"}
                </Select.Trigger>
                <Select.Content>
                  <Select.Group>
                    <Select.Label>Frequency</Select.Label>
                    {#each SCHEDULE_OPTIONS as option (option.value)}
                      <Select.Item value={option.value} label={option.label}>
                        {option.label}
                      </Select.Item>
                    {/each}
                  </Select.Group>
                </Select.Content>
              </Select.Root>
              <Field.Description>
                Scheduled backups use this provider and briefly restart Gitadel.
              </Field.Description>
            </Field.Field>

            {#if backup.scheduleMode === CUSTOM_SCHEDULE}
              <Field.Field data-invalid={!backup.cronExplanation.valid}>
                <Field.Label for="automatic-backup-cron"
                  >Cron expression</Field.Label
                >
                <Input
                  id="automatic-backup-cron"
                  bind:value={backup.customCron}
                  class="font-mono"
                  aria-invalid={!backup.cronExplanation.valid}
                  autocomplete="off"
                  spellcheck="false"
                  placeholder="0 2 * * *"
                />
                <Field.Description>
                  Five-field cron uses minute, hour, day of month, month, and
                  day of week. Six- and seven-field expressions may include
                  seconds and year.
                </Field.Description>
              </Field.Field>
              <Alert.Root
                variant={backup.cronExplanation.valid
                  ? "default"
                  : "destructive"}
              >
                <CalendarClock />
                <Alert.Title>
                  {backup.cronExplanation.valid
                    ? "Cron translation"
                    : "Invalid cron"}
                </Alert.Title>
                <Alert.Description
                  >{backup.cronExplanation.description}</Alert.Description
                >
              </Alert.Root>
            {/if}

            <div class="flex flex-wrap items-end justify-between gap-3">
              <dl class="text-sm">
                <dt class="text-xs text-muted-foreground">
                  Next automatic backup
                </dt>
                <dd class="mt-1 font-medium">
                  {#if backup.scheduleDirty}
                    Save the schedule to calculate the next run.
                  {:else if backup.settings.next_backup_at}
                    {formatDateTime(backup.settings.next_backup_at)}
                  {:else}
                    Not scheduled
                  {/if}
                </dd>
              </dl>
              <Button
                type="submit"
                disabled={backup.busy || !backup.canSaveSchedule}
              >
                {#if backup.savingSchedule}
                  <Spinner data-icon="inline-start" class="animate-spin" />
                  Saving…
                {:else}
                  Save schedule
                {/if}
              </Button>
            </div>
          </Field.Group>
        </form>
      </Card.Content>
    </Card.Root>
  {/if}

  <BackupSnapshotList {backup} />
  <BackupRestorePanel {backup} />
</section>

<BackupCreateDialog {backup} />

<Dialog.Root bind:open={backup.providerEditorOpen}>
  <Dialog.Content class="ring-foreground/20 sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>
        {backup.editingProvider
          ? `Configure ${backup.editingProvider.name}`
          : "Add backup provider"}
      </Dialog.Title>
      <Dialog.Description>
        {backup.editingProvider
          ? "Update this destination, test it, then save the exact configuration."
          : "Choose a provider, test the destination, then add it to the backup catalog."}
      </Dialog.Description>
    </Dialog.Header>
    {#key backup.editorRevision}
      <BackupProviderEditor
        providers={backup.providerCatalog}
        connection={backup.editingProvider}
        onsave={backup.saveProvider}
        ontest={backup.testProvider}
        oncancel={() => (backup.providerEditorOpen = false)}
      />
    {/key}
  </Dialog.Content>
</Dialog.Root>

<AlertDialog.Root bind:open={backup.providerRemoveDialogOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>
        Remove {backup.pendingProviderRemove?.name ?? "this backup provider"}?
      </AlertDialog.Title>
      <AlertDialog.Description>
        Existing backup files remain at the destination, but Gitadel will no
        longer list or schedule them.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        onclick={() => void backup.confirmProviderRemove()}
      >
        Remove provider
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
