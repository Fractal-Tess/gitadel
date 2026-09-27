<script lang="ts">
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import Users from "@lucide/svelte/icons/users";
  import { toast } from "svelte-sonner";
  import { z } from "zod";

  import {
    collaboratorSchema,
    type Collaborator,
    type CollaboratorRole,
  } from "$lib/api/collaborators.js";
  import { jsonBody, requestEmpty, requestJson } from "$lib/api/transport.js";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";
  import {
    errorMessage,
    repositoryApi,
  } from "$lib/repository/state/shared.js";

  let { state: repository }: { state: RepositoryPageState } = $props();

  const roleLabels: Record<CollaboratorRole, string> = {
    read: "Read",
    write: "Write",
  };
  const timestampFormatter = new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
  });

  let collaborators = $state.raw<Collaborator[]>([]);
  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let username = $state("");
  let role = $state<CollaboratorRole>("read");
  let adding = $state(false);
  let pendingUsername = $state<string | null>(null);
  let pendingRemoval = $state<Collaborator | null>(null);
  let removeDialogOpen = $state(false);

  const endpoint = $derived(
    repositoryApi(
      { namespace: repository.namespace, name: repository.name },
      "/collaborators",
    ),
  );

  $effect(() => {
    const path = endpoint;
    const controller = new AbortController();
    loading = true;
    loadError = null;
    requestJson(path, z.array(collaboratorSchema), {
      signal: controller.signal,
    })
      .then((rows) => {
        collaborators = rows;
      })
      .catch((caught: unknown) => {
        if (!controller.signal.aborted) loadError = errorMessage(caught);
      })
      .finally(() => {
        if (!controller.signal.aborted) loading = false;
      });
    return () => controller.abort();
  });

  async function addCollaborator() {
    adding = true;
    try {
      const created = await requestJson(endpoint, collaboratorSchema, {
        method: "POST",
        body: jsonBody({ username: username.trim(), role }),
      });
      collaborators = [...collaborators, created];
      username = "";
      toast.success(`${created.username} can now access this repository.`);
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      adding = false;
    }
  }

  // The API has no update route, so a role change re-adds the collaborator.
  async function changeRole(collaborator: Collaborator, next: CollaboratorRole) {
    if (collaborator.role === next) return;
    pendingUsername = collaborator.username;
    const path = `${endpoint}/${encodeURIComponent(collaborator.username)}`;
    try {
      await requestEmpty(path, { method: "DELETE" });
      const updated = await requestJson(endpoint, collaboratorSchema, {
        method: "POST",
        body: jsonBody({ username: collaborator.username, role: next }),
      });
      collaborators = collaborators.map((item) =>
        item.username === updated.username ? updated : item,
      );
      toast.success(`${updated.username} now has ${roleLabels[next].toLowerCase()} access.`);
    } catch (caught) {
      toast.error(errorMessage(caught));
      collaborators = await requestJson(endpoint, z.array(collaboratorSchema)).catch(
        () => collaborators,
      );
    } finally {
      pendingUsername = null;
    }
  }

  function requestRemoval(collaborator: Collaborator) {
    pendingRemoval = collaborator;
    removeDialogOpen = true;
  }

  async function confirmRemoval() {
    const target = pendingRemoval;
    if (!target) return;
    removeDialogOpen = false;
    pendingRemoval = null;
    pendingUsername = target.username;
    try {
      await requestEmpty(
        `${endpoint}/${encodeURIComponent(target.username)}`,
        { method: "DELETE" },
      );
      collaborators = collaborators.filter(
        (item) => item.username !== target.username,
      );
      toast.success(`${target.username} was removed.`);
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      pendingUsername = null;
    }
  }
</script>

<Card.Root class="self-start">
  <Card.Header class="border-b">
    <div class="flex items-start gap-3">
      <Users class="mt-0.5 size-4 shrink-0 text-foreground/70" />
      <div>
        <h2 class="text-base font-medium leading-snug">Collaborators</h2>
        <Card.Description class="text-foreground/70">
          Grant other users read or write access to this personal repository.
        </Card.Description>
      </div>
    </div>
  </Card.Header>
  <Card.Content
    class="grid gap-5 lg:grid-cols-[minmax(0,1.25fr)_minmax(18rem,0.75fr)]"
  >
    <ul class="divide-y rounded-lg border self-start" aria-busy={loading}>
      {#if loading}
        <li class="px-5 py-12 text-center text-sm text-foreground/70">
          Loading collaborators…
        </li>
      {:else if loadError}
        <li class="p-4">
          <Alert.Root variant="destructive">
            <Alert.Title>Collaborators unavailable</Alert.Title>
            <Alert.Description>{loadError}</Alert.Description>
          </Alert.Root>
        </li>
      {:else}
        {#each collaborators as collaborator (collaborator.username)}
          <li class="flex flex-wrap items-center justify-between gap-3 p-4">
            <div class="min-w-0">
              <p class="truncate text-sm font-medium">
                {collaborator.username}
              </p>
              <p class="mt-0.5 text-xs text-foreground/70">
                Added {timestampFormatter.format(
                  new Date(collaborator.created_at),
                )}
              </p>
            </div>
            <div class="flex items-center gap-2">
              <Select.Root
                type="single"
                value={collaborator.role}
                onValueChange={(value) =>
                  void changeRole(collaborator, value as CollaboratorRole)}
                disabled={pendingUsername === collaborator.username}
              >
                <Select.Trigger
                  size="sm"
                  class="w-28"
                  aria-label={`Access for ${collaborator.username}`}
                >
                  {roleLabels[collaborator.role]}
                </Select.Trigger>
                <Select.Content>
                  <Select.Item value="read">Read</Select.Item>
                  <Select.Item value="write">Write</Select.Item>
                </Select.Content>
              </Select.Root>
              <Button
                type="button"
                size="icon-sm"
                variant="ghost"
                disabled={pendingUsername === collaborator.username}
                aria-label={`Remove ${collaborator.username}`}
                onclick={() => requestRemoval(collaborator)}
              >
                <Trash2 class="size-3.5" />
              </Button>
            </div>
          </li>
        {:else}
          <li class="px-5 py-12 text-center text-sm text-foreground/70">
            Only you can access this repository beyond its visibility.
          </li>
        {/each}
      {/if}
    </ul>

    <form
      class="grid gap-4 self-start rounded-lg border p-4"
      onsubmit={(event) => {
        event.preventDefault();
        void addCollaborator();
      }}
    >
      <Field.Field>
        <Field.Label for="collaborator-username">Username</Field.Label>
        <Input
          id="collaborator-username"
          bind:value={username}
          autocomplete="off"
          maxlength={39}
          required
        />
      </Field.Field>
      <Field.Field>
        <Field.Label>Access</Field.Label>
        <Select.Root type="single" bind:value={role}>
          <Select.Trigger class="w-full">{roleLabels[role]}</Select.Trigger>
          <Select.Content>
            <Select.Item value="read">Read</Select.Item>
            <Select.Item value="write">Write</Select.Item>
          </Select.Content>
        </Select.Root>
        <Field.Description>
          Write access allows pushes. Repository settings stay with you.
        </Field.Description>
      </Field.Field>
      <Button
        type="submit"
        class="w-fit"
        disabled={adding || !username.trim()}
      >
        {adding ? "Adding…" : "Add collaborator"}
      </Button>
    </form>
  </Card.Content>
</Card.Root>

<AlertDialog.Root bind:open={removeDialogOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title
        >Remove {pendingRemoval?.username ?? "collaborator"}?</AlertDialog.Title
      >
      <AlertDialog.Description>
        They will lose access to private content and can no longer push.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        onclick={() => void confirmRemoval()}>Remove</AlertDialog.Action
      >
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
