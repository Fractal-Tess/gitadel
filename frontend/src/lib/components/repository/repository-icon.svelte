<script lang="ts">
  import { repositoryIconUrl } from "$lib/api/repositories.js";
  import * as Avatar from "$lib/components/ui/avatar/index.js";
  import { cn } from "$lib/utils.js";

  let {
    namespace,
    name,
    iconUpdatedAt = null,
    class: className = "size-9",
  }: {
    namespace: string;
    name: string;
    iconUpdatedAt?: string | null;
    class?: string;
  } = $props();

  let source = $derived(repositoryIconUrl(namespace, name, iconUpdatedAt));

  // Most repositories will never set an icon, so the fallback has to be worth
  // looking at on its own. Hashing the full path gives every repository a
  // stable hue that is the same in every list it appears in.
  let hue = $derived.by(() => {
    const seed = `${namespace}/${name}`;
    let hash = 0;
    for (let index = 0; index < seed.length; index += 1) {
      hash = (hash * 31 + seed.charCodeAt(index)) | 0;
    }
    return Math.abs(hash) % 360;
  });

  let initials = $derived(
    (
      name
        .match(/[a-z0-9]/gi)
        ?.slice(0, 2)
        .join("") || name.slice(0, 2)
    ).toUpperCase(),
  );
</script>

<Avatar.Root class={cn("shrink-0 rounded-md after:hidden", className)}>
  {#if source}
    <Avatar.Image src={source} alt="" class="rounded-md object-cover" />
  {/if}
  <Avatar.Fallback
    class="rounded-md text-xs font-semibold tracking-tight"
    style={`background:oklch(0.48 0.085 ${hue});color:oklch(0.98 0.015 ${hue})`}
  >
    {initials}
  </Avatar.Fallback>
</Avatar.Root>
