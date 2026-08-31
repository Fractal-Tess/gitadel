<script lang="ts">
  import ImageIcon from "@lucide/svelte/icons/image";
  import Palette from "@lucide/svelte/icons/palette";
  import RotateCcw from "@lucide/svelte/icons/rotate-ccw";
  import { toast } from "svelte-sonner";

  import AdminAccessSettings from "$lib/components/settings/admin-access-settings.svelte";
  import AuthenticationSettings from "$lib/components/settings/authentication-settings.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Textarea } from "$lib/components/ui/textarea/index.js";
  import {
    ApiFailure,
    jsonBody,
    requestEmpty,
    requestJson,
  } from "$lib/api/transport.js";
  import { instanceSettingsSchema } from "$lib/api/instance.js";
  import { AdminSettingsState } from "$lib/settings/admin-settings-state.svelte.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";
  let { view }: { view: "general" | "access" | "activity" } = $props();

  const app = useAppState();
  const adminState = new AdminSettingsState(app);
  let siteName = $state(app.instance?.site_name ?? "Gitadel");
  let siteDescription = $state(app.instance?.site_description ?? "");
  type FaviconTheme = "light" | "dark";

  let working = $state(false);
  let faviconInputVersion = $state(0);
  let settingsSaveQueued = false;
  let savingSettings = false;
  let faviconVersion = $derived(
    encodeURIComponent(app.instance?.updated_at ?? "default"),
  );

  $effect(() => {
    app.authorizationScope;
    void adminState.initialize(view === "general" ? "appearance" : view);
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
    try {
      await uploadFavicon(theme, file);
      await app.refreshInstance();
      faviconInputVersion += 1;
      toast.success(`${theme === "light" ? "Light" : "Dark"} favicon saved.`);
    } catch (caught) {
      toast.error(
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "Could not save the favicon.",
      );
    } finally {
      working = false;
    }
  }

  async function restoreFavicon(theme: FaviconTheme) {
    working = true;
    try {
      await requestEmpty(`/api/v1/admin/instance/favicon/${theme}`, {
        method: "DELETE",
      });
      await app.refreshInstance();
      faviconInputVersion += 1;
      toast.success(
        `${theme === "light" ? "Light" : "Dark"} favicon restored to the default.`,
      );
    } catch (caught) {
      toast.error(
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "Could not restore the favicon.",
      );
    } finally {
      working = false;
    }
  }

  async function saveSettings() {
    settingsSaveQueued = true;
    if (savingSettings) return;

    savingSettings = true;
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
      toast.success("Settings saved automatically.");
    } catch (caught) {
      toast.error(
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "Could not save instance settings.",
      );
    } finally {
      savingSettings = false;
    }
  }
</script>

<section class="flex flex-col gap-6" aria-label="Administration settings">
  <div class="flex flex-col gap-6">
    {#if view === "general"}
      <form
        onsubmit={(event) => {
          event.preventDefault();
          void saveSettings();
        }}
      >
        <Card.Root
          class="gap-5 [--card-spacing:--spacing(5)] md:grid md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:[--card-spacing:--spacing(6)]"
          aria-labelledby="branding-heading"
        >
          <Card.Header class="flex flex-row items-start gap-3">
            <Palette class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
            <div>
              <Card.Title id="branding-heading" role="heading" aria-level={2}>
                Branding and defaults
              </Card.Title>
              <Card.Description class="mt-1 max-w-xs leading-5">
                Browser identity and repository creation defaults.
              </Card.Description>
            </div>
          </Card.Header>
          <Card.Content class="grid max-w-2xl gap-5">
            <Field.Field>
              <Field.Label for="instance-site-name">Site name</Field.Label>
              <Input
                id="instance-site-name"
                bind:value={siteName}
                maxlength={80}
                onchange={() => void saveSettings()}
                required
              />
            </Field.Field>
            <Field.Field>
              <Field.Label for="instance-description">Description</Field.Label>
              <Textarea
                id="instance-description"
                class="min-h-24 resize-y"
                bind:value={siteDescription}
                maxlength={280}
                onchange={() => void saveSettings()}
              />
            </Field.Field>
            <Field.FieldSet class="border-t pt-5">
              <Field.FieldLegend class="sr-only"
                >Browser icons</Field.FieldLegend
              >
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
                  <RotateCcw data-icon="inline-start" />Default
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
                  <RotateCcw data-icon="inline-start" />Default
                </Button>
              </div>
            </Field.FieldSet>
          </Card.Content>
        </Card.Root>
      </form>
    {/if}

    {#if view === "access" || view === "activity"}
      <AdminAccessSettings state={adminState} {view} />
      {#if view === "access"}
        {#key app.authorizationScope}
          <AuthenticationSettings />
        {/key}
      {/if}
    {/if}
  </div>
</section>
