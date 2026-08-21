<script lang="ts">
  import { Copy, GitBranch } from "lucide-svelte";

  import RepositoryContent from "$lib/components/repository/repository-content.svelte";
  import RepositorySidebar from "$lib/components/repository/repository-sidebar.svelte";
  import RepositoryTree from "$lib/components/repository/repository-tree.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Tabs from "$lib/components/ui/tabs/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state }: { state: RepositoryPageState } = $props();

  function initialRepositoryCommands(): string {
    const repository = state.repository;
    if (!repository) return "";
    return `echo "# ${repository.name}" >> README.md
git init
git add README.md
git commit -m "Initial commit"
git branch -M main
git remote add origin ${repository.ssh_clone_url}
git push -u origin main`;
  }

  function existingRepositoryCommands(): string {
    const repository = state.repository;
    if (!repository) return "";
    return `git remote add origin ${repository.ssh_clone_url}
git branch -M main
git push -u origin main`;
  }
</script>

{#snippet commandBlock(commands: string)}
  <div class="relative overflow-hidden rounded-lg border bg-background/55">
    <pre class="overflow-x-auto p-4 pr-14 text-left text-xs leading-6"><code
        >{commands}</code
      ></pre>
    <Button
      class="absolute right-2 top-2"
      size="icon-sm"
      variant="outline"
      aria-label="Copy Git commands"
      onclick={() => void navigator.clipboard.writeText(commands)}
    >
      <Copy class="size-3.5" />
    </Button>
  </div>
{/snippet}

{#if state.emptyRepository}
  <section class="min-h-72 rounded-lg border bg-card p-6 shadow-sm sm:p-8">
    <div class="mx-auto max-w-3xl">
      <div class="text-center">
        <GitBranch
          class="mx-auto size-9 text-muted-foreground"
          strokeWidth={1.4}
        />
        <h2 class="mt-4 font-semibold">This repository is empty</h2>
        <p class="mt-2 text-sm text-muted-foreground">
          Push the first commit over SSH to begin the archive.
        </p>
      </div>

      <Tabs.Root value="new" class="mt-7">
        <Tabs.List class="grid w-full grid-cols-2">
          <Tabs.Trigger value="new">Create a new project</Tabs.Trigger>
          <Tabs.Trigger value="existing">Push an existing project</Tabs.Trigger>
        </Tabs.List>
        <Tabs.Content value="new" class="mt-3">
          {@render commandBlock(initialRepositoryCommands())}
        </Tabs.Content>
        <Tabs.Content value="existing" class="mt-3">
          {@render commandBlock(existingRepositoryCommands())}
        </Tabs.Content>
      </Tabs.Root>
    </div>
  </section>
{:else}
  <div
    class="grid overflow-hidden rounded-md border bg-card/20 xl:grid-cols-[18rem_minmax(0,1fr)_18rem]"
  >
    <RepositoryTree {state} />
    <RepositoryContent {state} />
    <RepositorySidebar {state} />
  </div>
{/if}
