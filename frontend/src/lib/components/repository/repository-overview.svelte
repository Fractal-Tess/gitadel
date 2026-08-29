<script lang="ts">
  import { Copy, GitBranch } from "lucide-svelte";
  import { MediaQuery } from "svelte/reactivity";

  import RepositoryContent from "$lib/components/repository/repository-content.svelte";
  import RepositoryTree from "$lib/components/repository/repository-tree.svelte";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Resizable from "$lib/components/ui/resizable/index.js";
  import * as Tabs from "$lib/components/ui/tabs/index.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state }: { state: RepositoryPageState } = $props();

  // 1280px is Tailwind's `xl`, the width where the tree stops stacking above
  // the content and becomes a resizable side column.
  const wideLayout = new MediaQuery("min-width: 1280px");

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
  <div class="xl:h-full xl:overflow-y-auto xl:overscroll-contain">
    <section
      class="m-5 min-h-72 rounded-lg border bg-card p-6 shadow-sm sm:p-8"
    >
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
            <Tabs.Trigger value="existing"
              >Push an existing project</Tabs.Trigger
            >
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
  </div>
{:else}
  <!-- The tree is only a side column on wide viewports, and a horizontal pane
       group cannot stack, so the resizable layout is gated on the same 1280px
       breakpoint Tailwind's `xl` uses. -->
  {#if wideLayout.current}
    <Resizable.PaneGroup
      direction="horizontal"
      autoSaveId="gitadel:repository-overview"
      class="h-full min-h-0 bg-card/20"
    >
      <Resizable.Pane defaultSize={22} minSize={12} maxSize={50}>
        <RepositoryTree {state} />
      </Resizable.Pane>
      <!-- The 1px divider keeps the seam the fixed layout had. The grip makes
           the column draggable at a glance, and the wider ::after strip is the
           grab area, tinted on hover and while dragging. -->
      <Resizable.Handle
        withHandle
        class="after:w-3 hover:after:bg-primary/30 data-[active]:after:bg-primary/50 [&>div]:h-8 [&>div]:w-1.5 [&>div]:bg-muted-foreground/60 [&>div]:transition-colors hover:[&>div]:bg-primary data-[active]:[&>div]:bg-primary"
      />
      <Resizable.Pane defaultSize={78} minSize={30}>
        <RepositoryContent {state} />
      </Resizable.Pane>
    </Resizable.PaneGroup>
  {:else}
    <div class="grid bg-card/20">
      <RepositoryTree {state} />
      <RepositoryContent {state} />
    </div>
  {/if}
{/if}
