<script lang="ts">
  import { onMount } from "svelte";
  import { ImageIcon, Palette, RotateCcw } from "lucide-svelte";

  import AdminAccessSettings from "$lib/components/settings/admin-access-settings.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import {
    ApiFailure,
    instanceSettingsSchema,
    jsonBody,
    requestEmpty,
    requestJson,
  } from "$lib/api.js";
  import { AdminSettingsState } from "$lib/settings/admin-settings-state.svelte.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";
  let {
    view,
  }: { view: "general" | "access" | "activity" } = $props();


  const app = useAppState();
  const adminState = new AdminSettingsState();
  let siteName = $state(app.instance?.site_name ?? "Gitadel");
  let siteDescription = $state(app.instance?.site_description ?? "");
  type FaviconTheme = "light" | "dark";

  let working = $state(false);
  let notice = $state<string | null>(null);
  let error = $state<string | null>(null);
  let faviconInputVersion = $state(0);
  let settingsSaveQueued = false;
  let savingSettings = false;
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

  async function saveFavicon(theme: FaviconTheme, file: File | null) {
    if (!file) return;

    working = true;
    notice = null;
    error = null;
    try {
      await uploadFavicon(theme, file);
      await app.refreshInstance();
      faviconInputVersion += 1;
      notice = `${theme === "light" ? "Light" : "Dark"} favicon saved.`;
    } catch (caught) {
      error =
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "Could not save the favicon.";
    } finally {
      working = false;
    }
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
    settingsSaveQueued = true;
    if (savingSettings) return;

    savingSettings = true;
    notice = null;
    error = null;
    try {
      while (settingsSaveQueued) {
        settingsSaveQueued = false;
        app.instance = await requestJson(
          "/api/v1/admin/instance",
          instanceSettingsSchema,
          {
            method: "PUT",
            body: jsonBody({
              site_name: siteName,
              site_description: siteDescription || null,
            }),
          },
        );
      }
      notice = "Settings saved automatically.";
    } catch (caught) {
      error =
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "Could not save instance settings.";
    } finally {
      savingSettings = false;
    }
  }
</script>

<section class="space-y-6" aria-labelledby="administration-heading">
  <header>
    <h2
      id="administration-heading"
      class="text-lg font-semibold tracking-tight"
    >
      {view === "general"
        ? "Appearance"
        : view === "access"
          ? "Access"
          : "Activity"}
    </h2>
    <p class="mt-1.5 text-sm text-muted-foreground">
      {view === "general"
        ? "Manage instance identity and repository defaults."
        : view === "access"
          ? "Control how additional users gain access."
          : "Review repository, authentication, and administration events."}
    </p>
  </header>

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

    {#if view === "general"}
    <form
      class="overflow-hidden rounded-xl border bg-card/40 shadow-sm"
      onsubmit={(event) => {
        event.preventDefault();
        void saveSettings();
      }}
    >
      <header class="flex items-center gap-3 border-b px-5 py-4">
        <Palette class="size-4 text-muted-foreground" />
        <div>
          <h2 class="text-sm font-semibold">Branding and defaults</h2>
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
            onchange={() => void saveSettings()}
            required
          />
        </label>
        <label class="grid gap-1.5 text-sm font-medium">
          Description
          <textarea
            class="min-h-24 resize-y rounded-md border bg-background px-3 py-2 outline-none focus:border-ring focus:ring-2 focus:ring-ring/20"
            bind:value={siteDescription}
            maxlength="280"
            onchange={() => void saveSettings()}></textarea>
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
                  disabled={working}
                  onchange={(event) =>
                    void saveFavicon(
                      "light",
                      event.currentTarget.files?.[0] ?? null,
                    )}
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
                  disabled={working}
                  onchange={(event) =>
                    void saveFavicon(
                      "dark",
                      event.currentTarget.files?.[0] ?? null,
                    )}
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

      </div>
    </form>

    {/if}

    {#if view === "access" || view === "activity"}
      <AdminAccessSettings state={adminState} {view} />
    {/if}

  </div>
</section>
