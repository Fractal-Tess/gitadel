<script lang="ts">
  import ShieldCheck from "@lucide/svelte/icons/shield-check";
  import TriangleAlert from "@lucide/svelte/icons/triangle-alert";

  import { Button } from "$lib/components/ui/button/index.js";
  import { Checkbox } from "$lib/components/ui/checkbox/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import { formatBytes, formatDateTime } from "$lib/format.js";
  import type { BackupState } from "$lib/settings/backup-state.svelte.js";

  let { backup }: { backup: BackupState } = $props();
</script>

{#if backup.preflight}
  <section
    class="overflow-hidden rounded-xl border border-destructive/35 bg-destructive/3 shadow-sm"
  >
    <header
      class="flex items-start gap-3 border-b border-destructive/20 px-5 py-4"
    >
      <TriangleAlert class="mt-0.5 size-4 shrink-0 text-destructive" />
      <div>
        <h3 class="text-sm font-semibold">Replace this instance</h3>
        <p class="mt-1 text-xs leading-relaxed text-muted-foreground">
          Restoring <span class="font-medium text-foreground"
            >{backup.preflight.backup_name ?? "this backup"}</span
          >
          from Gitadel {backup.preflight.gitadel_version ?? "unknown"} will stop Gitadel
          and completely replace the current database, users, settings, repositories,
          LFS data, assets, and SSH host key.
        </p>
      </div>
    </header>
    <div class="grid gap-5 p-5">
      <dl
        class="grid gap-3 rounded-lg border bg-background/50 p-4 text-sm sm:grid-cols-3"
      >
        <div>
          <dt class="text-xs text-muted-foreground">Created</dt>
          <dd class="mt-1 font-medium">
            {formatDateTime(backup.preflight.created_at)}
          </dd>
        </div>
        <div>
          <dt class="text-xs text-muted-foreground">Verified files</dt>
          <dd class="mt-1 font-medium">{backup.preflight.file_count}</dd>
        </div>
        <div>
          <dt class="text-xs text-muted-foreground">Restored size</dt>
          <dd class="mt-1 font-medium">
            {formatBytes(backup.preflight.uncompressed_size)}
          </dd>
        </div>
      </dl>

      {#if backup.preflight.version_warning}
        <p
          class="rounded-md border border-amber-500/30 bg-amber-500/8 p-3 text-sm text-amber-200"
        >
          {backup.preflight.version_warning} Review release notes and compatibility
          before continuing.
        </p>
      {/if}

      <label
        for="restore-safety-backup"
        class="flex items-start gap-3 rounded-lg border p-4 text-sm"
      >
        <Checkbox
          id="restore-safety-backup"
          class="mt-0.5"
          bind:checked={backup.createSafetyBackup}
        />
        <span>
          <span class="flex items-center gap-2 font-medium">
            <ShieldCheck class="size-4 text-emerald-400" />
            Back up the current instance first
          </span>
          <span
            class="mt-1 block text-xs leading-relaxed text-muted-foreground"
          >
            Recommended. Restore will stop if this safety backup cannot be
            created with {backup.settings?.name ?? "the selected provider"}.
          </span>
        </span>
      </label>

      {#if !backup.createSafetyBackup}
        <p
          class="rounded-md border border-amber-500/30 bg-amber-500/8 p-3 text-sm text-amber-200"
        >
          No recovery snapshot of the current instance will be created.
        </p>
      {/if}

      <Field.Field>
        <Field.Label for="restore-admin-password">
          Administrator password
        </Field.Label>
        <Input
          id="restore-admin-password"
          bind:value={backup.password}
          type="password"
          autocomplete="current-password"
          required
        />
      </Field.Field>
      <label
        for="confirm-restore-replacement"
        class="flex items-start gap-3 text-sm"
      >
        <Checkbox
          id="confirm-restore-replacement"
          class="mt-0.5"
          bind:checked={backup.confirmReplacement}
        />
        <span>
          I understand that all current instance data will be replaced.
        </span>
      </label>
      {#if backup.restoring}
        <p
          class="flex items-center gap-2 text-sm text-muted-foreground"
          role="status"
        >
          <Spinner />
          Restoring. This page reloads when Gitadel is back.
        </p>
      {/if}
      <div class="flex justify-end gap-2">
        <Button
          type="button"
          variant="outline"
          disabled={backup.busy || backup.restoring}
          onclick={() => (backup.preflight = null)}>Cancel</Button
        >
        <Button
          type="button"
          variant="destructive"
          disabled={backup.busy ||
            backup.restoring ||
            !backup.password ||
            !backup.confirmReplacement}
          onclick={() => void backup.restoreBackup()}
        >
          Restore this backup
        </Button>
      </div>
    </div>
  </section>
{/if}
