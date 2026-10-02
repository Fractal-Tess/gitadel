<script lang="ts">
  import ArchiveRestore from "@lucide/svelte/icons/archive-restore";
  import Bot from "@lucide/svelte/icons/bot";
  import CalendarDays from "@lucide/svelte/icons/calendar-days";
  import CheckCircle2 from "@lucide/svelte/icons/check-circle-2";
  import Download from "@lucide/svelte/icons/download";
  import HardDrive from "@lucide/svelte/icons/hard-drive";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import Tag from "@lucide/svelte/icons/tag";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import UserRound from "@lucide/svelte/icons/user-round";

  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { formatBytes, formatDateTime } from "$lib/format.js";
  import {
    backupFileName,
    backupProviderPath,
    isAutomaticBackup,
  } from "$lib/settings/backup-state.svelte.js";
  import type { BackupState } from "$lib/settings/backup-state.svelte.js";

  let { backup }: { backup: BackupState } = $props();
</script>

{#if backup.settings}
  {@const provider = backup.settings}
  <section class="overflow-hidden rounded-xl border bg-card/40 shadow-sm">
    <header
      class="flex flex-wrap items-center justify-between gap-3 border-b px-5 py-4"
    >
      <div class="flex items-center gap-3">
        <ArchiveRestore class="size-4 text-muted-foreground" />
        <div>
          <h3 class="text-sm font-semibold">Snapshots</h3>
          <p class="mt-0.5 text-xs text-muted-foreground">
            Each snapshot includes the database, repositories, container images,
            LFS, assets, host key, and effective configuration.
          </p>
        </div>
      </div>
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={backup.busy}
        onclick={() => void backup.refreshSnapshots()}
      >
        <RefreshCw class="size-3.5" /> Refresh
      </Button>
    </header>

    {#if backup.creating && backup.backupProgress}
      <div
        class="border-b bg-muted/20 px-5 py-4"
        role="status"
        aria-live="polite"
      >
        <div class="flex items-start gap-3">
          {#if backup.backupProgress.phase === "completed"}
            <CheckCircle2 class="mt-0.5 size-4 shrink-0 text-emerald-400" />
          {:else}
            <Spinner class="mt-0.5 size-4 shrink-0 text-primary" />
          {/if}
          <div class="min-w-0 flex-1">
            <div class="flex items-center justify-between gap-3">
              <p class="text-sm font-medium">{backup.backupProgress.message}</p>
              {#if backup.backupProgressPercent !== null}
                <span class="text-xs tabular-nums text-muted-foreground">
                  {backup.backupProgressPercent}%
                </span>
              {/if}
            </div>
            {#if backup.backupProgressPercent !== null}
              <div
                class="mt-2 h-1.5 overflow-hidden rounded-full bg-muted"
                aria-label="Backup upload progress"
                aria-valuemin="0"
                aria-valuemax="100"
                aria-valuenow={backup.backupProgressPercent}
                role="progressbar"
              >
                <div
                  class="h-full rounded-full bg-primary transition-[width] duration-300"
                  style:width={`${backup.backupProgressPercent}%`}
                ></div>
              </div>
              <p class="mt-1.5 text-xs text-muted-foreground">
                {formatBytes(backup.backupProgress.processed_bytes ?? 0)} of
                {formatBytes(backup.backupProgress.total_bytes ?? 0)} uploaded
              </p>
            {:else}
              <p class="mt-1 text-xs text-muted-foreground">
                The server remains unavailable to normal requests while this
                phase runs.
              </p>
            {/if}
          </div>
        </div>
      </div>
    {/if}

    {#if backup.snapshots.length === 0}
      <p class="p-5 text-sm text-muted-foreground">
        No snapshots found under this prefix.
      </p>
    {:else}
      <div class="motion-list divide-y">
        {#each backup.snapshots as snapshot (snapshot.key)}
          <article
            class="flex flex-wrap items-center justify-between gap-4 px-5 py-4"
          >
            <div class="min-w-0">
              <p class="truncate text-sm font-medium" title={snapshot.key}>
                {snapshot.name ?? "Unnamed backup"}
              </p>
              <p
                class="mt-0.5 truncate font-mono text-xs text-muted-foreground"
              >
                {backupFileName(snapshot.key)}
              </p>
              <div class="mt-2 flex flex-wrap gap-1.5">
                <Badge variant="outline">
                  <CalendarDays data-icon="inline-start" />
                  {formatDateTime(snapshot.created_at)}
                </Badge>
                <Badge variant="outline">
                  <HardDrive data-icon="inline-start" />
                  {formatBytes(snapshot.size)}
                </Badge>
                <Badge variant="outline">
                  <Tag data-icon="inline-start" />
                  Gitadel {snapshot.gitadel_version ?? "unknown"}
                </Badge>
                <Badge variant="secondary">
                  {#if isAutomaticBackup(snapshot)}
                    <Bot data-icon="inline-start" /> Auto-generated
                  {:else}
                    <UserRound data-icon="inline-start" /> Manual
                  {/if}
                </Badge>
              </div>
            </div>
            <div class="flex flex-wrap items-center gap-2">
              <Button
                href={`${backupProviderPath(provider.id)}/backups/object?key=${encodeURIComponent(snapshot.key)}`}
                download={backupFileName(snapshot.key)}
                variant="outline"
                size="sm"
                disabled={backup.busy}
              >
                <Download class="size-3.5" /> Download
              </Button>
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={backup.busy}
                onclick={() => void backup.validateSnapshot(snapshot)}
              >
                Validate restore
              </Button>
              <Button
                type="button"
                variant="destructive"
                size="sm"
                disabled={backup.busy}
                onclick={() => backup.requestDelete(snapshot)}
              >
                <Trash2 class="size-3.5" /> Delete
              </Button>
            </div>
          </article>
        {/each}
      </div>
    {/if}
  </section>
{/if}

<AlertDialog.Root bind:open={backup.deleteDialogOpen}>
  <AlertDialog.Content class="min-w-0 overflow-hidden sm:max-w-md">
    <AlertDialog.Header>
      <AlertDialog.Title class="break-words">
        Delete {backup.pendingDelete?.name ?? "this backup"}?
      </AlertDialog.Title>
      <AlertDialog.Description class="min-w-0 text-left">
        This permanently removes
        {#if backup.pendingDelete}
          <span class="break-all font-mono text-foreground">
            {backupFileName(backup.pendingDelete.key)}
          </span>
        {:else}
          the snapshot
        {/if}
        from {backup.settings?.name ?? "the selected provider"}. It cannot be
        restored after deletion.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={backup.deleting}>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        disabled={backup.deleting}
        onclick={() => void backup.deleteSnapshot()}
      >
        {#if backup.deleting}
          <Spinner class="size-3.5" data-icon="inline-start" /> Deleting…
        {:else}
          Delete backup
        {/if}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
