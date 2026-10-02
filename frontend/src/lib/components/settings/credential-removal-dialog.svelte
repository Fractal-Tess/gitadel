<script lang="ts" module>
  export type CredentialRemoval = {
    kind: "passkey" | "ssh-key" | "api-token";
    id: string;
    name: string;
  };
</script>

<script lang="ts">
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";

  let {
    open = $bindable(false),
    target,
    working,
    onconfirm,
  }: {
    open?: boolean;
    /** The credential awaiting confirmation; kept while the dialog closes. */
    target: CredentialRemoval | null;
    working: boolean;
    onconfirm: () => void;
  } = $props();
</script>

<AlertDialog.Root bind:open>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>
        {target?.kind === "api-token" ? "Revoke" : "Remove"}
        {target?.name ?? "credential"}?
      </AlertDialog.Title>
      <AlertDialog.Description>
        {#if target?.kind === "api-token"}
          Any script or integration using this token will lose access
          immediately.
        {:else if target?.kind === "ssh-key"}
          This key will no longer authenticate Git operations over SSH.
        {:else}
          This passkey will no longer be available for sign-in.
        {/if}
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action
        variant="destructive"
        disabled={working}
        onclick={onconfirm}
      >
        {target?.kind === "api-token" ? "Revoke token" : "Remove"}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
