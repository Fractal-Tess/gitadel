<script lang="ts">
  import ArrowUpRight from "@lucide/svelte/icons/arrow-up-right";
  import Copy from "@lucide/svelte/icons/copy";
  import FileCode from "@lucide/svelte/icons/file-code";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import GitCommitHorizontal from "@lucide/svelte/icons/git-commit-horizontal";
  import Info from "@lucide/svelte/icons/info";
  import Terminal from "@lucide/svelte/icons/terminal";
  import { toast } from "svelte-sonner";

  import { copyText } from "$lib/clipboard.js";
  import * as Alert from "$lib/components/ui/alert/index.js";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import { submoduleLinks } from "$lib/repository/submodule-links.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state }: { state: RepositoryPageState } = $props();
  const entry = $derived(state.browser.submodule);
  const details = $derived(entry?.submodule);
  const links = $derived(
    submoduleLinks(details?.url, state.httpCloneUrl, entry?.oid ?? ""),
  );
  const hasConfiguration = $derived(
    state.browser.repositoryTree?.entries.some(
      (item) => item.path === ".gitmodules" && item.kind === "blob",
    ),
  );
  const checkoutCommand = $derived(
    entry
      ? `git submodule update --init --recursive -- '${entry.path.replaceAll("'", "'\\''")}'`
      : "",
  );

  async function copy(value: string, label: string): Promise<void> {
    try {
      await copyText(value);
      toast.success(`${label} copied`);
    } catch {
      toast.error(`Could not copy ${label.toLowerCase()}.`);
    }
  }
</script>

{#if entry}
  <header
    class="flex min-h-12 shrink-0 items-center gap-2 border-b px-5 py-2 text-sm font-semibold"
  >
    <GitBranch class="size-4 shrink-0 text-muted-foreground" />
    <span class="min-w-0 truncate" title={entry.path}>{entry.path}</span>
    <Badge variant="secondary" class="ml-auto shrink-0">Submodule</Badge>
  </header>
  <div class="min-h-0 flex-1 overflow-y-auto p-5 xl:overscroll-contain lg:p-8">
    <div class="mx-auto flex max-w-3xl flex-col gap-6">
      <Card.Root>
        <Card.Header>
          <div
            class="mb-2 flex items-center gap-2 text-sm text-muted-foreground"
          >
            <GitBranch class="size-4" />
            <span>Linked repository</span>
            {#if links.host}<span aria-hidden="true">·</span><span
                >{links.host}</span
              >{/if}
          </div>
          <Card.Title class="break-words"
            >{links.label ?? details?.name ?? entry.name}</Card.Title
          >
          <Card.Description>
            These files live in another repository. {state.name} records the exact
            commit to use, rather than storing a copy of its files here.
          </Card.Description>
        </Card.Header>
        <Card.Content class="flex flex-col gap-5">
          {#if details?.url}
            <div class="flex flex-col gap-2">
              <span class="text-xs font-medium text-muted-foreground"
                >Repository URL</span
              >
              <div
                class="flex items-start gap-2 rounded-md border bg-muted/30 p-3"
              >
                <code class="min-w-0 flex-1 break-all text-xs leading-6"
                  >{details.url}</code
                >
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label="Copy repository URL"
                  onclick={() => copy(details.url!, "Repository URL")}
                >
                  <Copy />
                </Button>
              </div>
            </div>
          {/if}
          <dl class="grid gap-4 text-sm sm:grid-cols-2">
            <div class="flex min-w-0 flex-col gap-1.5">
              <dt class="flex items-center gap-1.5 text-muted-foreground">
                <GitCommitHorizontal class="size-4" />Pinned commit
              </dt>
              <dd class="flex items-start gap-2">
                <code class="text-xs leading-6" title={entry.oid}
                  >{entry.oid.slice(0, 12)}</code
                >
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label="Copy pinned commit"
                  onclick={() => copy(entry.oid, "Commit ID")}><Copy /></Button
                >
              </dd>
            </div>
            <div class="flex min-w-0 flex-col gap-1.5">
              <dt class="text-muted-foreground">Recorded in</dt>
              <dd class="break-all">
                {state.namespace}/{state.name}
                <span class="text-muted-foreground">at</span>
                <code>{state.revision}</code>
              </dd>
            </div>
            {#if details?.branch}
              <div class="flex min-w-0 flex-col gap-1.5 sm:col-span-2">
                <dt class="text-muted-foreground">Tracking branch</dt>
                <dd class="break-all font-mono">
                  {details.branch === "."
                    ? "Same branch as the parent repository"
                    : details.branch}
                </dd>
                <p class="text-xs text-muted-foreground">
                  Used when explicitly updating from upstream. The pinned commit
                  above still determines the checked-out files.
                </p>
              </div>
            {/if}
          </dl>
        </Card.Content>
        <Card.Footer class="flex-wrap gap-2">
          {#if links.repositoryUrl}
            <Button
              href={links.repositoryUrl}
              target="_blank"
              rel="noopener noreferrer"
              aria-label="Open repository (opens in a new tab)"
              >Open repository <ArrowUpRight data-icon="inline-end" /></Button
            >
          {/if}
          {#if links.commitUrl}
            <Button
              variant="outline"
              href={links.commitUrl}
              target="_blank"
              rel="noopener noreferrer"
              aria-label="View pinned commit (opens in a new tab)"
              ><GitCommitHorizontal data-icon="inline-start" />View pinned
              commit</Button
            >
          {/if}
          {#if hasConfiguration}
            <Button
              variant="ghost"
              onclick={() =>
                state.navigate("overview", { path: ".gitmodules" })}
              ><FileCode data-icon="inline-start" />View .gitmodules</Button
            >
          {/if}
        </Card.Footer>
      </Card.Root>
      {#if !links.repositoryUrl}
        <Alert.Root>
          <Info />
          <Alert.Title
            >{details?.url
              ? "No browser link available"
              : "Repository URL unavailable"}</Alert.Title
          >
          <Alert.Description>
            {details?.url
              ? "This URL cannot be opened as a supported web repository link. Review .gitmodules and use a trusted Git client to access the source."
              : "No repository URL is available from .gitmodules at this revision. The pinned commit is still recorded above."}
          </Alert.Description>
        </Alert.Root>
      {/if}
      <section
        class="flex flex-col gap-3"
        aria-label="Local submodule checkout"
      >
        <h3 class="flex items-center gap-2 text-sm font-semibold">
          <Terminal class="size-4 text-muted-foreground" />Get these files
          locally
        </h3>
        <p class="text-sm text-muted-foreground">
          In a trusted {state.name} clone, check out the revision you're browsing
          and review .gitmodules first. Then run this from the repository root to
          download the submodule and its nested dependencies.
        </p>
        <div
          class="flex items-start gap-2 rounded-lg border bg-background/50 p-4"
        >
          <code
            class="min-w-0 flex-1 whitespace-pre-wrap break-all text-xs leading-6"
            >{checkoutCommand}</code
          >
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="Copy submodule checkout command"
            onclick={() => copy(checkoutCommand, "Checkout command")}
            ><Copy /></Button
          >
        </div>
      </section>
    </div>
  </div>
{/if}
