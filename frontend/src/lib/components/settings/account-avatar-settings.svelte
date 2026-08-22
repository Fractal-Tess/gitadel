<script lang="ts">
  import { Camera } from "lucide-svelte";
  import { toast } from "svelte-sonner";

  import { blobToBase64 } from "$lib/avatar-crop.js";
  import AvatarCropDialog from "$lib/components/settings/avatar-crop-dialog.svelte";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
  import * as Avatar from "$lib/components/ui/avatar/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { ApiFailure, avatarUrl, jsonBody, requestEmpty } from "$lib/api.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";

  const app = useAppState();
  let editorOpen = $state(false);
  let removeOpen = $state(false);
  let removing = $state(false);
  const user = $derived(app.authStatus?.user ?? null);
  const imageUrl = $derived(
    user ? avatarUrl(user.id, user.avatar_updated_at) : null,
  );

  async function saveAvatar(blob: Blob) {
    await requestEmpty("/api/v1/me/avatar", {
      method: "PUT",
      body: jsonBody({ image_base64: await blobToBase64(blob) }),
    });
    await app.refreshAuth();
    toast.success("Profile picture updated");
  }

  async function removeAvatar() {
    removing = true;
    try {
      await requestEmpty("/api/v1/me/avatar", { method: "DELETE" });
      await app.refreshAuth();
      removeOpen = false;
      toast.success("Profile picture removed");
    } catch (caught) {
      toast.error(
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "The profile picture could not be removed.",
      );
    } finally {
      removing = false;
    }
  }
</script>

<section
  class="grid gap-5 p-5 md:grid-cols-[minmax(12rem,0.72fr)_minmax(0,1.5fr)] md:gap-10 md:p-6"
  aria-labelledby="profile-picture-heading"
>
  <header class="flex items-start gap-3">
    <Camera class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
    <div>
      <h2 id="profile-picture-heading" class="font-semibold">
        Profile picture
      </h2>
      <p class="mt-1 max-w-xs text-sm leading-5 text-muted-foreground">
        Choose how your account appears across Gitadel.
      </p>
    </div>
  </header>

  <div class="flex max-w-2xl flex-col gap-4 sm:flex-row sm:items-center">
    <Avatar.Root class="size-24 ring-1 ring-foreground/15">
      {#if imageUrl}
        <Avatar.Image src={imageUrl} alt="" />
      {/if}
      <Avatar.Fallback class="text-xl font-medium uppercase">
        {user?.username.slice(0, 2) ?? ""}
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
        JPG, PNG, or WebP up to 10 MB. You can reposition and zoom before
        saving.
      </p>
    </div>
  </div>
</section>

<AvatarCropDialog bind:open={editorOpen} onsave={saveAvatar} />

<AlertDialog.Root bind:open={removeOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Remove profile picture?</AlertDialog.Title>
      <AlertDialog.Description>
        Your initials will appear in its place. You can upload another picture
        at any time.
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
