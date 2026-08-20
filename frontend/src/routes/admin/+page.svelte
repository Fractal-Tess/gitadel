<script lang="ts">
  import { resolve } from "$app/paths";
  import { onMount } from "svelte";
  import { Database, Save, Server, ShieldCheck } from "lucide-svelte";

  import AdminAccessSettings from "$lib/components/settings/admin-access-settings.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import {
    ApiFailure,
    instanceSettingsSchema,
    jsonBody,
    requestJson,
  } from "$lib/api.js";
  import { AdminSettingsState } from "$lib/settings/admin-settings-state.svelte.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  const adminState = new AdminSettingsState();
  let siteName = $state(app.instance?.site_name ?? "Gitadel");
  let siteDescription = $state(app.instance?.site_description ?? "");
  let defaultVisibility = $state<"public" | "private">(
    app.instance?.default_repository_visibility ?? "private",
  );
  let working = $state(false);
  let notice = $state<string | null>(null);
  let error = $state<string | null>(null);

  onMount(() => {
    void adminState.initialize();
  });


  async function saveSettings(): Promise<void> {
    working = true;
    notice = null;
    error = null;
    try {
      app.instance = await requestJson(
        "/api/v1/admin/instance",
        instanceSettingsSchema,
        {
          method: "PUT",
          body: jsonBody({
            site_name: siteName,
            site_description: siteDescription || null,
            default_repository_visibility: defaultVisibility,
          }),
        },
      );
      notice = "Instance settings saved.";
    } catch (caught) {
      error =
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "Could not save instance settings.";
    } finally {
      working = false;
    }
  }
</script>

<svelte:head>
  <title>Instance settings · {app.instance?.site_name ?? "Gitadel"}</title>
</svelte:head>

<div class="min-h-screen bg-background">
  <header class="border-b bg-background/95">
    <div class="mx-auto flex h-16 max-w-6xl items-center justify-between gap-4 px-5">
      <a class="text-sm font-bold tracking-[-0.035em]" href={resolve("/")}>
        {app.instance?.site_name ?? "GITADEL"}
      </a>
      <nav class="flex items-center gap-2 text-sm">
        <a class="text-muted-foreground hover:text-foreground" href={resolve("/settings")}>
          Account settings
        </a>
      </nav>
    </div>
  </header>

  <main class="mx-auto max-w-6xl px-5 py-10">
    <div class="grid gap-8 lg:grid-cols-[15rem_minmax(0,1fr)]">
      <aside>
        <p class="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
          Administration
        </p>
        <h1 class="mt-2 text-2xl font-semibold">Instance settings</h1>
        <p class="mt-3 text-sm leading-6 text-muted-foreground">
          Global identity and repository defaults for this Gitadel installation.
        </p>
      </aside>

      <div class="space-y-6">
        {#if notice}
          <p class="rounded-md border border-emerald-500/25 bg-emerald-500/8 p-3 text-sm text-emerald-300">
            {notice}
          </p>
        {/if}
        {#if error}
          <p class="rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
            {error}
          </p>
        {/if}

        <form
          class="rounded-md border bg-card/25"
          onsubmit={(event) => {
            event.preventDefault();
            void saveSettings();
          }}
        >
          <header class="flex items-center gap-3 border-b px-5 py-4">
            <Server class="size-4 text-muted-foreground" />
            <div>
              <h2 class="text-sm font-semibold">General</h2>
              <p class="mt-0.5 text-xs text-muted-foreground">
                Browser identity and repository creation defaults.
              </p>
            </div>
          </header>
          <div class="grid gap-5 p-5">
            <label class="grid gap-1.5 text-sm font-medium">
              Site name
              <input
                class="rounded-md border bg-background px-3 py-2 outline-none focus:border-ring focus:ring-2 focus:ring-ring/20"
                bind:value={siteName}
                maxlength="80"
                required
              />
            </label>
            <label class="grid gap-1.5 text-sm font-medium">
              Description
              <textarea
                class="min-h-24 resize-y rounded-md border bg-background px-3 py-2 outline-none focus:border-ring focus:ring-2 focus:ring-ring/20"
                bind:value={siteDescription}
                maxlength="280"
              ></textarea>
            </label>
            <label class="grid max-w-sm gap-1.5 text-sm font-medium">
              Default repository visibility
              <Select.Root
                type="single"
                value={defaultVisibility}
                onValueChange={(value) => {
                  if (value === "public" || value === "private") {
                    defaultVisibility = value;
                  }
                }}
              >
                <Select.Trigger class="w-full">
                  {defaultVisibility === "private" ? "Private" : "Public"}
                </Select.Trigger>
                <Select.Content>
                  <Select.Item value="private">Private</Select.Item>
                  <Select.Item value="public">Public</Select.Item>
                </Select.Content>
              </Select.Root>
            </label>
          </div>
          <footer class="flex justify-end border-t px-5 py-4">
            <Button class="gap-2" type="submit" disabled={working}>
              <Save class="size-4" />Save settings
            </Button>
          </footer>
        </form>

        <section class="rounded-md border bg-card/25">
          <header class="flex items-center gap-3 border-b px-5 py-4">
            <ShieldCheck class="size-4 text-muted-foreground" />
            <div>
              <h2 class="text-sm font-semibold">Registration</h2>
              <p class="mt-0.5 text-xs text-muted-foreground">Closed after initial setup.</p>
            </div>
          </header>
          <div class="flex items-center justify-between gap-5 p-5 text-sm">
            <div>
              <p class="font-medium">Public registration disabled</p>
              <p class="mt-1 text-xs text-muted-foreground">
                The first account is the administrator. Additional accounts require an invitation.
              </p>
            </div>
            <span class="rounded-full border px-2.5 py-1 text-xs text-muted-foreground">Locked</span>
          </div>
        </section>

        <AdminAccessSettings state={adminState} />

        <section class="rounded-md border bg-card/25 p-5">
          <div class="flex items-start gap-3">
            <Database class="mt-0.5 size-4 text-muted-foreground" />
            <div>
              <h2 class="text-sm font-semibold">Persistence</h2>
              <p class="mt-1 text-xs leading-5 text-muted-foreground">
                Instance settings are stored in the application database and apply to every deployment mode.
              </p>
            </div>
          </div>
        </section>
      </div>
    </div>
  </main>
</div>
