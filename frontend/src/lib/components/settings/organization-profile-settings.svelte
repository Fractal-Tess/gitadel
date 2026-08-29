<script lang="ts">
  import { goto } from "$app/navigation";
  import { resolve } from "$app/paths";
  import Building2 from "@lucide/svelte/icons/building-2";
  import Camera from "@lucide/svelte/icons/camera";
  import { untrack } from "svelte";
  import { toast } from "svelte-sonner";

  import { blobToBase64 } from "$lib/avatar-crop.js";
  import {
    ApiFailure,
    jsonBody,
    requestEmpty,
    requestJson,
  } from "$lib/api/transport.js";
  import {
    organizationAvatarUrl,
    organizationSchema,
    type Organization,
  } from "$lib/api/organizations.js";
  import AvatarCropDialog from "$lib/components/settings/avatar-crop-dialog.svelte";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import * as Avatar from "$lib/components/ui/avatar/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  let { organization }: { organization: Organization } = $props();
  const app = useAppState();
  let displayName = $state(untrack(() => organization.display_name));
  let slug = $state(untrack(() => organization.slug));
  let savingProfile = $state(false);
  let profileError = $state<string | null>(null);
  let editorOpen = $state(false);
  let removeOpen = $state(false);
  let removing = $state(false);
  const imageUrl = $derived(
    organizationAvatarUrl(organization.slug, organization.avatar_updated_at),
  );

  function message(caught: unknown, fallback: string): string {
    return caught instanceof ApiFailure || caught instanceof Error
      ? caught.message
      : fallback;
  }

  async function saveProfile(): Promise<void> {
    const currentSlug = organization.slug;
    savingProfile = true;
    profileError = null;
    try {
      const updated = await requestJson(
        `/api/v1/organizations/${encodeURIComponent(currentSlug)}`,
        organizationSchema,
        {
          method: "PUT",
          body: jsonBody({ slug, display_name: displayName }),
        },
      );
      slug = updated.slug;
      displayName = updated.display_name;
      await app.refreshOrganizations();
      toast.success("Organization profile updated");
      if (updated.slug !== currentSlug) {
        await goto(
          resolve("/[namespace]/settings", { namespace: updated.slug }),
        );
      }
    } catch (caught) {
      profileError = message(
        caught,
        "The organization profile could not be updated.",
      );
    } finally {
      savingProfile = false;
    }
  }

  async function saveAvatar(blob: Blob): Promise<void> {
    await requestEmpty(
      `/api/v1/organizations/${encodeURIComponent(organization.slug)}/avatar`,
      {
        method: "PUT",
        body: jsonBody({ image_base64: await blobToBase64(blob) }),
      },
    );
    await app.refreshOrganizations();
    toast.success("Organization picture updated");
  }

  async function removeAvatar(): Promise<void> {
    removing = true;
    try {
      await requestEmpty(
        `/api/v1/organizations/${encodeURIComponent(organization.slug)}/avatar`,
        { method: "DELETE" },
      );
      await app.refreshOrganizations();
      removeOpen = false;
      toast.success("Organization picture removed");
    } catch (caught) {
      toast.error(
        message(caught, "The organization picture could not be removed."),
      );
    } finally {
      removing = false;
    }
  }
</script>

<section class="space-y-6" aria-labelledby="organization-profile-heading">
  <header>
    <h2
      id="organization-profile-heading"
      class="text-lg font-semibold tracking-tight"
    >
      Organization profile
    </h2>
    <p class="mt-1.5 max-w-2xl text-sm leading-6 text-muted-foreground">
      Control how {organization.slug} appears across Gitadel.
    </p>
  </header>

  <div class="overflow-hidden rounded-xl border bg-card/40 shadow-sm">
    <section
      class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
    >
      <header class="flex items-start gap-3">
        <Camera class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div>
          <h3 class="font-semibold">Profile picture</h3>
          <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
            Shown in navigation and organization pages.
          </p>
        </div>
      </header>
      <div class="flex max-w-2xl flex-col gap-4 sm:flex-row sm:items-center">
        <Avatar.Root class="size-24 ring-1 ring-foreground/15">
          {#if imageUrl}<Avatar.Image src={imageUrl} alt="" />{/if}
          <Avatar.Fallback class="text-xl font-medium uppercase">
            {organization.display_name.slice(0, 2)}
          </Avatar.Fallback>
        </Avatar.Root>
        <div class="grid gap-3">
          <div class="flex flex-wrap gap-2">
            <Button type="button" onclick={() => (editorOpen = true)}>
              {imageUrl ? "Change picture" : "Upload picture"}
            </Button>
            {#if imageUrl}
              <Button
                type="button"
                variant="outline"
                onclick={() => (removeOpen = true)}
              >
                Remove
              </Button>
            {/if}
          </div>
          <p class="max-w-sm text-xs leading-5 text-muted-foreground">
            JPG, PNG, or WebP up to 10 MB. Reposition and zoom before saving.
          </p>
        </div>
      </div>
    </section>

    <section
      class="grid gap-5 border-t p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
    >
      <header class="flex items-start gap-3">
        <Building2 class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div>
          <h3 class="font-semibold">Organization identity</h3>
          <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
            Set the readable name and the slug used in repository URLs.
          </p>
        </div>
      </header>
      <form
        class="grid max-w-xl gap-3"
        onsubmit={(event) => {
          event.preventDefault();
          void saveProfile();
        }}
      >
        {#if profileError}
          <p
            class="rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive"
            role="alert"
          >
            {profileError}
          </p>
        {/if}
        <label class="grid gap-1.5 text-sm font-medium">
          Organization name
          <Input bind:value={displayName} maxlength={80} required />
        </label>
        <label class="grid gap-1.5 text-sm font-medium">
          Organization slug
          <Input bind:value={slug} maxlength={39} required />
          <span class="text-xs font-normal text-muted-foreground">
            Changing the slug updates repository, clone, runner, integration,
            and mirror credential paths.
          </span>
        </label>
        <div>
          <Button type="submit" disabled={savingProfile}
            >{savingProfile ? "Saving…" : "Save changes"}</Button
          >
        </div>
      </form>
    </section>
  </div>
</section>

<AvatarCropDialog bind:open={editorOpen} onsave={saveAvatar} />

<AlertDialog.Root bind:open={removeOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Remove organization picture?</AlertDialog.Title>
      <AlertDialog.Description>
        The organization initials will appear in its place.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        disabled={removing}
        onclick={() => void removeAvatar()}
      >
        {removing ? "Removing…" : "Remove picture"}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
