<script lang="ts">
  import { resolve } from "$app/paths";
  import { onMount } from "svelte";
  import {
    Database,
    ImageIcon,
    RotateCcw,
    Save,
    Server,
    ShieldCheck,
  } from "lucide-svelte";

  import BrandMark from "$lib/components/brand-mark.svelte";
  import AdminAccessSettings from "$lib/components/settings/admin-access-settings.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Select from "$lib/components/ui/select/index.js";
  import {
    ApiFailure,
    instanceSettingsSchema,
    jsonBody,
    requestEmpty,
    requestJson,
  } from "$lib/api.js";
  import { preloadAccountSettings } from "$lib/navigation-cache.js";
  import { AdminSettingsState } from "$lib/settings/admin-settings-state.svelte.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  const adminState = new AdminSettingsState();
  let siteName = $state(app.instance?.site_name ?? "Gitadel");
  let siteDescription = $state(app.instance?.site_description ?? "");
  let defaultVisibility = $state<"public" | "private">(
    app.instance?.default_repository_visibility ?? "private",
  );
  type FaviconTheme = "light" | "dark";

  let working = $state(false);
  let notice = $state<string | null>(null);
  let error = $state<string | null>(null);
  let lightFavicon = $state<File | null>(null);
  let darkFavicon = $state<File | null>(null);
  let faviconInputVersion = $state(0);
  let faviconVersion = $derived(
    encodeURIComponent(app.instance?.updated_at ?? "default"),
  );

  onMount(() => {
    void adminState.initialize();
  });

  async function uploadFavicon(theme: FaviconTheme, file: File | null) {
    if (!file) return;
    await requestEmpty(`/api/v1/admin/instance/favicon/${theme}`, {
      method: "PUT",
      headers: { "content-type": "image/png" },
      body: file,
    });
  }

  async function restoreFavicon(theme: FaviconTheme) {
    working = true;
    notice = null;
    error = null;
    try {
      await requestEmpty(`/api/v1/admin/instance/favicon/${theme}`, {
        method: "DELETE",
      });
      await app.refreshInstance();
      lightFavicon = null;
      darkFavicon = null;
      faviconInputVersion += 1;
      notice = `${theme === "light" ? "Light" : "Dark"} favicon restored to the default.`;
    } catch (caught) {
      error =
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "Could not restore the favicon.";
    } finally {
      working = false;
    }
  }

  async function saveSettings() {
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
      await uploadFavicon("light", lightFavicon);
      await uploadFavicon("dark", darkFavicon);
      if (lightFavicon || darkFavicon) await app.refreshInstance();
      lightFavicon = null;
      darkFavicon = null;
      faviconInputVersion += 1;
      notice = "Instance identity saved.";
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
    <div
      class="mx-auto flex h-16 max-w-6xl items-center justify-between gap-4 px-5"
    >
      <a
        class="flex items-center gap-2 text-sm font-bold tracking-[-0.035em]"
        href={resolve("/")}
      >
        <BrandMark />
        {app.instance?.site_name ?? "GITADEL"}
      </a>
      <nav class="flex items-center gap-2 text-sm">
        <a
          class="text-muted-foreground hover:text-foreground"
          href={resolve("/settings")}
          onpointerenter={() =>
            preloadAccountSettings(app.authStatus?.user?.username)}
          onpointerdown={() =>
            preloadAccountSettings(app.authStatus?.user?.username)}
          onfocus={() => preloadAccountSettings(app.authStatus?.user?.username)}
        >
          Account settings
        </a>
        <a
          class="text-muted-foreground hover:text-foreground"
          href={resolve("/changelog")}
        >
          Changelog
        </a>
      </nav>
    </div>
  </header>

  <main class="mx-auto max-w-6xl px-5 py-10">
    <div class="grid gap-8 lg:grid-cols-[15rem_minmax(0,1fr)]">
      <aside>
        <p
          class="text-xs font-semibold uppercase tracking-wider text-muted-foreground"
        >
          Administration
        </p>
        <h1 class="mt-2 text-2xl font-semibold">Instance settings</h1>
        <p class="mt-3 text-sm leading-6 text-muted-foreground">
          Global identity and repository defaults for this Gitadel installation.
        </p>
      </aside>

      <div class="space-y-6">
        {#if notice}
          <p
            class="rounded-md border border-emerald-500/25 bg-emerald-500/8 p-3 text-sm text-emerald-300"
          >
            {notice}
          </p>
        {/if}
        {#if error}
          <p
            class="rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive"
          >
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
                maxlength="280"></textarea>
            </label>
            <fieldset class="grid gap-4 border-t pt-5">
              <legend class="sr-only">Browser icons</legend>
              <div class="flex items-center gap-2">
                <ImageIcon class="size-4 text-muted-foreground" />
                <div>
                  <h3 class="text-sm font-medium">Browser icons</h3>
                  <p class="mt-0.5 text-xs text-muted-foreground">
                    Upload square PNG files up to 512 KiB.
                  </p>
                </div>
              </div>

              <div
                class="grid items-center gap-3 sm:grid-cols-[2.5rem_minmax(0,1fr)_auto]"
              >
                <img
                  class="size-10 rounded-md border bg-white object-contain p-1"
                  src={`/api/v1/instance/favicon/light?v=${faviconVersion}&r=2`}
                  alt="Current light theme favicon"
                />
                <label class="grid min-w-0 gap-1 text-sm font-medium">
                  Light browser theme
                  {#key faviconInputVersion}
                    <input
                      class="min-w-0 text-xs font-normal text-muted-foreground file:mr-3 file:rounded-md file:border file:bg-background file:px-3 file:py-1.5 file:text-xs file:font-medium file:text-foreground"
                      type="file"
                      accept="image/png,.png"
                      onchange={(event) => {
                        lightFavicon = event.currentTarget.files?.[0] ?? null;
                      }}
                    />
                  {/key}
                </label>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  class="gap-2 justify-self-start"
                  disabled={working}
                  onclick={() => void restoreFavicon("light")}
                >
                  <RotateCcw class="size-3.5" />Default
                </Button>
              </div>

              <div
                class="grid items-center gap-3 sm:grid-cols-[2.5rem_minmax(0,1fr)_auto]"
              >
                <img
                  class="size-10 rounded-md border bg-zinc-950 object-contain p-1"
                  src={`/api/v1/instance/favicon/dark?v=${faviconVersion}&r=2`}
                  alt="Current dark theme favicon"
                />
                <label class="grid min-w-0 gap-1 text-sm font-medium">
                  Dark browser theme
                  {#key faviconInputVersion}
                    <input
                      class="min-w-0 text-xs font-normal text-muted-foreground file:mr-3 file:rounded-md file:border file:bg-background file:px-3 file:py-1.5 file:text-xs file:font-medium file:text-foreground"
                      type="file"
                      accept="image/png,.png"
                      onchange={(event) => {
                        darkFavicon = event.currentTarget.files?.[0] ?? null;
                      }}
                    />
                  {/key}
                </label>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  class="gap-2 justify-self-start"
                  disabled={working}
                  onclick={() => void restoreFavicon("dark")}
                >
                  <RotateCcw class="size-3.5" />Default
                </Button>
              </div>
            </fieldset>

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
              <p class="mt-0.5 text-xs text-muted-foreground">
                Closed after initial setup.
              </p>
            </div>
          </header>
          <div class="flex items-center justify-between gap-5 p-5 text-sm">
            <div>
              <p class="font-medium">Public registration disabled</p>
              <p class="mt-1 text-xs text-muted-foreground">
                The first account is the administrator. Additional accounts
                require an invitation.
              </p>
            </div>
            <span
              class="rounded-full border px-2.5 py-1 text-xs text-muted-foreground"
              >Locked</span
            >
          </div>
        </section>

        <AdminAccessSettings state={adminState} />

        <section class="rounded-md border bg-card/25 p-5">
          <div class="flex items-start gap-3">
            <Database class="mt-0.5 size-4 text-muted-foreground" />
            <div>
              <h2 class="text-sm font-semibold">Persistence</h2>
              <p class="mt-1 text-xs leading-5 text-muted-foreground">
                Instance settings are stored in the application database and
                apply to every deployment mode.
              </p>
            </div>
          </div>
        </section>
      </div>
    </div>
  </main>
</div>
