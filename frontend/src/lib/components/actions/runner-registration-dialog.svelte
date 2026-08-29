<script lang="ts">
  import { Copy, ShieldAlert } from "lucide-svelte";

  import type { ActionRegistration } from "$lib/api.js";
  import { copyText } from "$lib/clipboard.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";

  let {
    registration,
    onclose,
  }: {
    registration: ActionRegistration | null;
    onclose: () => void;
  } = $props();

  function shellQuote(value: string) {
    return `'${value.replaceAll("'", "'\\''")}'`;
  }

  const command = $derived(
    registration
      ? `forgejo-runner register --no-interactive --instance ${shellQuote(registration.server_url.replace(/\/$/, ""))} --token ${shellQuote(registration.token)} --name ${shellQuote(registration.runner_name)} --labels ${shellQuote(registration.labels.join(","))}`
      : "",
  );
</script>

<Dialog.Root
  open={registration !== null}
  onOpenChange={(open) => {
    if (!open) onclose();
  }}
>
  <Dialog.Content class="ring-foreground/20 sm:max-w-2xl" showCloseButton={false}>
    <Dialog.Header>
      <Dialog.Title>Register Forgejo Runner</Dialog.Title>
      <Dialog.Description>
        This token is shown once and expires in ten minutes. Gitadel requires
        Forgejo Runner {registration?.required_version ?? "13.0.0"}.
      </Dialog.Description>
    </Dialog.Header>

    <div class="rounded-md border border-amber-500/30 bg-amber-500/5 p-3 text-sm">
      <div class="flex gap-2">
        <ShieldAlert class="mt-0.5 size-4 shrink-0 text-amber-600" />
        <p class="leading-5 text-muted-foreground">
          Use a dedicated disposable host or VM. Configure Docker-only,
          digest-pinned labels with capacity one; disable privileged mode,
          volumes, devices, the Docker socket, and cache. Gitadel cannot verify
          runner isolation through the protocol.
        </p>
      </div>
    </div>

    <div class="relative rounded-md bg-muted p-3 pr-12 font-mono text-xs leading-5 break-all">
      {command}
      <Button
        type="button"
        variant="ghost"
        size="icon-sm"
        class="absolute top-2 right-2"
        aria-label="Copy registration command"
        onclick={() => void copyText(command)}
      >
        <Copy class="size-4" />
      </Button>
    </div>

    <Dialog.Footer>
      <Button type="button" onclick={onclose}>Done</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
