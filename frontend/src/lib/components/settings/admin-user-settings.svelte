<script lang="ts">
  import ChevronLeft from "@lucide/svelte/icons/chevron-left";
  import ChevronRight from "@lucide/svelte/icons/chevron-right";
  import Ellipsis from "@lucide/svelte/icons/ellipsis";
  import Search from "@lucide/svelte/icons/search";
  import { toast } from "svelte-sonner";

  import {
    adminUserApi,
    adminUserPageSchema,
    adminUserSchema,
    type AdminUser,
  } from "$lib/api/admin-users.js";
  import { ApiFailure, requestEmpty, requestJson } from "$lib/api/transport.js";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Spinner } from "$lib/components/ui/spinner/index.js";
  import * as Table from "$lib/components/ui/table/index.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  type PendingAction = {
    kind: "disable" | "delete" | "reset-two-factor";
    user: AdminUser;
  };

  const pageSize = 25;
  const app = useAppState();
  const dateFormatter = new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
  });

  let search = $state("");
  let query = $state("");
  let offset = $state(0);
  let users = $state.raw<AdminUser[]>([]);
  let total = $state(0);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let busyUsername = $state<string | null>(null);
  let pending = $state<PendingAction | null>(null);
  let confirmation = $state("");
  let dialogOpen = $state(false);
  let reloadVersion = $state(0);

  const viewer = $derived(app.authStatus?.user?.username);
  const pageEnd = $derived(Math.min(offset + users.length, total));

  $effect(() => {
    const parameters = new URLSearchParams({
      q: query,
      limit: String(pageSize),
      offset: String(offset),
    });
    void reloadVersion;
    const controller = new AbortController();
    loading = true;
    requestJson(`/api/v1/admin/users?${parameters}`, adminUserPageSchema, {
      signal: controller.signal,
    })
      .then((page) => {
        users = page.users;
        total = page.total;
        error = null;
      })
      .catch((caught: unknown) => {
        if (!controller.signal.aborted) error = messageOf(caught);
      })
      .finally(() => {
        if (!controller.signal.aborted) loading = false;
      });
    return () => controller.abort();
  });

  function messageOf(caught: unknown): string {
    return caught instanceof ApiFailure || caught instanceof Error
      ? caught.message
      : "The request failed.";
  }

  function applySearch() {
    offset = 0;
    query = search.trim().toLowerCase();
  }

  function replace(updated: AdminUser) {
    users = users.map((user) => (user.id === updated.id ? updated : user));
  }

  async function enable(user: AdminUser) {
    busyUsername = user.username;
    try {
      replace(
        await requestJson(adminUserApi(user.username, "/enable"), adminUserSchema, {
          method: "POST",
        }),
      );
      toast.success(`${user.username} can sign in again.`);
    } catch (caught) {
      toast.error(messageOf(caught));
    } finally {
      busyUsername = null;
    }
  }

  function request(kind: PendingAction["kind"], user: AdminUser) {
    pending = { kind, user };
    confirmation = "";
    dialogOpen = true;
  }

  async function confirm() {
    const action = pending;
    if (!action) return;
    busyUsername = action.user.username;
    try {
      if (action.kind === "disable") {
        replace(
          await requestJson(
            adminUserApi(action.user.username, "/disable"),
            adminUserSchema,
            { method: "POST" },
          ),
        );
        toast.success(`${action.user.username} was disabled and signed out.`);
      } else if (action.kind === "reset-two-factor") {
        await requestEmpty(adminUserApi(action.user.username, "/two-factor"), {
          method: "DELETE",
        });
        replace({ ...action.user, two_factor_enabled: false });
        toast.success(`Two-factor authentication was reset for ${action.user.username}.`);
      } else {
        await requestEmpty(adminUserApi(action.user.username), {
          method: "DELETE",
        });
        toast.success(`${action.user.username} was deleted.`);
        reloadVersion += 1;
      }
      dialogOpen = false;
      pending = null;
    } catch (caught) {
      toast.error(messageOf(caught));
    } finally {
      busyUsername = null;
    }
  }
</script>

<section class="overflow-hidden rounded-xl border bg-card/40 shadow-sm">
  <header
    class="flex flex-wrap items-center justify-between gap-3 border-b px-5 py-4"
  >
    <form
      class="flex w-full max-w-sm items-center gap-2"
      role="search"
      onsubmit={(event) => {
        event.preventDefault();
        applySearch();
      }}
    >
      <Input
        type="search"
        placeholder="Search by username"
        aria-label="Search users"
        bind:value={search}
        maxlength={39}
      />
      <Button type="submit" variant="outline" size="icon" aria-label="Search">
        <Search class="size-4" />
      </Button>
    </form>
    <p class="text-xs text-muted-foreground" aria-live="polite">
      {#if total > 0}
        {offset + 1}–{pageEnd} of {total}
      {:else if !loading}
        No users
      {/if}
    </p>
  </header>

  {#if error}
    <div class="p-5">
      <Alert.Root variant="destructive">
        <Alert.Title>Users unavailable</Alert.Title>
        <Alert.Description>{error}</Alert.Description>
      </Alert.Root>
    </div>
  {:else if loading && users.length === 0}
    <p
      class="flex items-center justify-center gap-2 py-16 text-sm text-muted-foreground"
    >
      <Spinner class="size-4" /> Loading users…
    </p>
  {:else}
    <Table.Root aria-busy={loading}>
      <Table.Header>
        <Table.Row>
          <Table.Head class="pl-5">User</Table.Head>
          <Table.Head>Status</Table.Head>
          <Table.Head class="max-sm:hidden">Joined</Table.Head>
          <Table.Head class="w-12 pr-5"
            ><span class="sr-only">Actions</span></Table.Head
          >
        </Table.Row>
      </Table.Header>
      <Table.Body class="motion-list">
        {#each users as user (user.id)}
          <Table.Row>
            <Table.Cell class="pl-5">
              <div class="flex flex-wrap items-center gap-2">
                <span class="font-medium">{user.username}</span>
                {#if user.username === viewer}
                  <span class="text-xs text-muted-foreground">(you)</span>
                {/if}
                {#if user.is_admin}
                  <Badge variant="secondary">Administrator</Badge>
                {/if}
                {#if user.two_factor_enabled}
                  <Badge variant="outline">2FA</Badge>
                {/if}
              </div>
            </Table.Cell>
            <Table.Cell>
              {#if user.disabled_at}
                <span class="text-destructive">
                  Disabled {dateFormatter.format(new Date(user.disabled_at))}
                </span>
              {:else}
                <span class="text-muted-foreground">Active</span>
              {/if}
            </Table.Cell>
            <Table.Cell class="text-muted-foreground max-sm:hidden">
              {dateFormatter.format(new Date(user.created_at))}
            </Table.Cell>
            <Table.Cell class="pr-5 text-right">
              <DropdownMenu.Root>
                <DropdownMenu.Trigger
                  disabled={user.username === viewer ||
                    busyUsername === user.username}
                >
                  {#snippet child({ props })}
                    <Button
                      {...props}
                      variant="ghost"
                      size="icon-sm"
                      aria-label={`Manage ${user.username}`}
                    >
                      <Ellipsis class="size-4" />
                    </Button>
                  {/snippet}
                </DropdownMenu.Trigger>
                <DropdownMenu.Content align="end">
                  {#if user.disabled_at}
                    <DropdownMenu.Item onclick={() => void enable(user)}>
                      Enable account
                    </DropdownMenu.Item>
                  {:else}
                    <DropdownMenu.Item
                      onclick={() => request("disable", user)}
                    >
                      Disable account
                    </DropdownMenu.Item>
                  {/if}
                  {#if user.two_factor_enabled}
                    <DropdownMenu.Item
                      onclick={() => request("reset-two-factor", user)}
                    >
                      Reset two-factor authentication
                    </DropdownMenu.Item>
                  {/if}
                  <DropdownMenu.Separator />
                  <DropdownMenu.Item
                    variant="destructive"
                    onclick={() => request("delete", user)}
                  >
                    Delete account
                  </DropdownMenu.Item>
                </DropdownMenu.Content>
              </DropdownMenu.Root>
            </Table.Cell>
          </Table.Row>
        {:else}
          <Table.Row>
            <Table.Cell colspan={4} class="py-12 text-center text-muted-foreground">
              No users match “{query}”.
            </Table.Cell>
          </Table.Row>
        {/each}
      </Table.Body>
    </Table.Root>
  {/if}

  {#if total > pageSize}
    <footer class="flex items-center justify-end gap-2 border-t px-5 py-3">
      <Button
        variant="outline"
        size="sm"
        disabled={offset === 0 || loading}
        onclick={() => (offset = Math.max(0, offset - pageSize))}
      >
        <ChevronLeft data-icon="inline-start" />Previous
      </Button>
      <Button
        variant="outline"
        size="sm"
        disabled={offset + pageSize >= total || loading}
        onclick={() => (offset += pageSize)}
      >
        Next<ChevronRight data-icon="inline-end" />
      </Button>
    </footer>
  {/if}
</section>

<AlertDialog.Root bind:open={dialogOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>
        {#if pending?.kind === "disable"}
          Disable {pending.user.username}?
        {:else if pending?.kind === "reset-two-factor"}
          Reset two-factor authentication for {pending.user.username}?
        {:else}
          Delete {pending?.user.username ?? "this user"}?
        {/if}
      </AlertDialog.Title>
      <AlertDialog.Description>
        {#if pending?.kind === "disable"}
          They are signed out everywhere and their API and OAuth tokens are
          revoked. Re-enabling the account does not restore those tokens.
        {:else if pending?.kind === "reset-two-factor"}
          Their authenticator and recovery codes are removed, so a password
          alone will sign them in until they enroll again.
        {:else}
          This permanently removes the account, its keys, and its personal
          namespace. Accounts that own repositories, are the only owner of an
          organization, or authored issues and releases must be disabled
          instead.
        {/if}
      </AlertDialog.Description>
    </AlertDialog.Header>
    {#if pending?.kind === "delete"}
      <Field.Field>
        <Field.Label for="delete-user-confirmation">
          Type <span class="font-mono">{pending.user.username}</span> to confirm
        </Field.Label>
        <Input
          id="delete-user-confirmation"
          bind:value={confirmation}
          autocomplete="off"
        />
      </Field.Field>
    {/if}
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        disabled={busyUsername !== null ||
          (pending?.kind === "delete" &&
            confirmation !== pending.user.username)}
        onclick={() => void confirm()}
      >
        {pending?.kind === "disable"
          ? "Disable account"
          : pending?.kind === "reset-two-factor"
            ? "Reset"
            : "Delete account"}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
