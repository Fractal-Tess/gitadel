<script lang="ts">
  import ArrowDown from "@lucide/svelte/icons/arrow-down";
  import ArrowUp from "@lucide/svelte/icons/arrow-up";
  import ExternalLink from "@lucide/svelte/icons/external-link";
  import Globe from "@lucide/svelte/icons/globe";
  import Pencil from "@lucide/svelte/icons/pencil";
  import Plus from "@lucide/svelte/icons/plus";
  import X from "@lucide/svelte/icons/x";
  import { tick } from "svelte";
  import { z } from "zod";
  import { toast } from "svelte-sonner";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { errorMessage, jsonBody, requestJson } from "$lib/api/transport.js";
  import {
    websiteHost,
    websiteProblem,
    withWebsiteScheme,
  } from "$lib/repository/websites.js";
  import { repositoryApi } from "$lib/repository/state/shared.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state: pageState }: { state: RepositoryPageState } = $props();
  const limit = 10;
  const schema = z.array(z.object({ label: z.string(), url: z.string() }));
  let websites = $state<z.infer<typeof schema>>([]);
  let draft = $state<Array<{ id: string; label: string; url: string }>>([]);
  let editing = $state(false);
  let pending = $state(false);
  let submitted = $state(false);
  let error = $state("");
  let form = $state<HTMLFormElement | null>(null);

  const problems = $derived(
    draft.map((website, index) =>
      websiteProblem(
        website.url,
        website.label,
        draft.slice(0, index).map((other) => other.url),
      ),
    ),
  );

  $effect(() => {
    const repository = pageState;
    const controller = new AbortController();
    editing = false;
    websites = [];
    error = "";
    requestJson(repositoryApi(repository, "/websites"), schema, {
      signal: controller.signal,
    })
      .then((links) => {
        if (!controller.signal.aborted) websites = links;
      })
      .catch((caught) => {
        if (!controller.signal.aborted) error = errorMessage(caught);
      });
    return () => controller.abort();
  });

  let nextDraftId = 0;
  async function edit() {
    draft = websites.map((website) => ({
      ...website,
      id: String(++nextDraftId),
    }));
    submitted = false;
    editing = true;
    if (!draft.length) await add();
    else {
      await tick();
      form?.querySelector<HTMLInputElement>("input")?.focus();
    }
  }
  async function add() {
    const id = String(++nextDraftId);
    draft.push({ id, label: "", url: "" });
    await tick();
    document.getElementById(`website-url-${id}`)?.focus();
  }
  function move(index: number, offset: number) {
    const [website] = draft.splice(index, 1);
    draft.splice(index + offset, 0, website);
  }
  function remove(index: number) {
    draft.splice(index, 1);
  }
  function cancel() {
    editing = false;
  }
  async function save(event: SubmitEvent) {
    event.preventDefault();
    submitted = true;
    // Blank rows are dropped rather than rejected, so "Add" can be undone by
    // leaving the row empty.
    const links = draft
      .map(({ label, url }) => ({
        label: label.trim(),
        url: withWebsiteScheme(url),
      }))
      .filter((link) => link.url);
    if (problems.some(Boolean)) {
      form?.querySelector<HTMLInputElement>('[aria-invalid="true"]')?.focus();
      return;
    }
    pending = true;
    try {
      websites = await requestJson(
        repositoryApi(pageState, "/websites"),
        schema,
        { method: "PUT", body: jsonBody({ websites: links }) },
      );
      editing = false;
      error = "";
      toast.success(
        websites.length ? "Website links saved." : "Website links removed.",
      );
    } catch (caught) {
      toast.error(errorMessage(caught));
    } finally {
      pending = false;
    }
  }
</script>

<section class="min-w-0 p-4">
  <div class="flex min-h-6 items-center justify-between gap-2">
    <h2
      class="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-muted-foreground"
    >
      <Globe class="size-3.5" />Websites
    </h2>
    {#if pageState.repository?.can_manage && !editing && websites.length}
      <Button
        variant="ghost"
        size="icon-xs"
        aria-label="Edit website links"
        title="Edit website links"
        onclick={edit}><Pencil /></Button
      >
    {/if}
  </div>
  {#if editing}
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions (Escape anywhere in the editor cancels it.) -->
    <form
      bind:this={form}
      class="mt-3 flex flex-col gap-2"
      onsubmit={save}
      onkeydown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          cancel();
        }
      }}
      novalidate
    >
      <ol class="flex flex-col gap-2" aria-label="Website links in display order">
        {#each draft as website, index (website.id)}
          {@const problem = submitted ? problems[index] : ""}
          <li
            class="flex min-w-0 flex-col gap-1.5 rounded-md border bg-muted/20 p-2"
          >
            <div class="flex min-w-0 items-center gap-1">
              <span class="mr-auto text-[11px] text-muted-foreground tabular-nums"
                >Link {index + 1}</span
              >
              <Button
                variant="ghost"
                size="icon-xs"
                type="button"
                aria-label={`Move website ${index + 1} up`}
                disabled={pending || index === 0}
                onclick={() => move(index, -1)}><ArrowUp /></Button
              >
              <Button
                variant="ghost"
                size="icon-xs"
                type="button"
                aria-label={`Move website ${index + 1} down`}
                disabled={pending || index === draft.length - 1}
                onclick={() => move(index, 1)}><ArrowDown /></Button
              >
              <Button
                variant="ghost"
                size="icon-xs"
                type="button"
                aria-label={`Remove website ${index + 1}`}
                disabled={pending}
                onclick={() => remove(index)}><X /></Button
              >
            </div>
            <Input
              id={`website-url-${website.id}`}
              bind:value={website.url}
              onblur={() => (website.url = withWebsiteScheme(website.url))}
              aria-label={`Website ${index + 1} URL`}
              aria-invalid={!!problem}
              aria-describedby={problem ? `website-error-${website.id}` : undefined}
              type="url"
              inputmode="url"
              autocomplete="url"
              maxlength={2048}
              disabled={pending}
              placeholder="https://example.com"
              class="h-7 text-xs"
            />
            <Input
              id={`website-label-${website.id}`}
              bind:value={website.label}
              aria-label={`Website ${index + 1} name`}
              maxlength={80}
              disabled={pending}
              placeholder={websiteHost(website.url)
                ? `Name, defaults to ${websiteHost(website.url)}`
                : "Name (optional)"}
              class="h-7 text-xs"
            />
            {#if problem}
              <p
                id={`website-error-${website.id}`}
                class="text-xs text-destructive"
              >
                {problem}
              </p>
            {/if}
          </li>
        {/each}
      </ol>
      {#if !draft.length}
        <p class="text-xs text-muted-foreground">
          Saving with no links removes them all.
        </p>
      {/if}
      <div class="flex flex-wrap items-center justify-between gap-2">
        <Button
          type="button"
          variant="ghost"
          size="xs"
          disabled={pending || draft.length >= limit}
          onclick={() => void add()}
          ><Plus data-icon="inline-start" />Add link</Button
        >
        <span class="text-[11px] tabular-nums text-muted-foreground"
          >{draft.length} of {limit}</span
        >
      </div>
      <div class="flex justify-end gap-2">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={pending}
          onclick={cancel}>Cancel</Button
        >
        <Button type="submit" size="sm" disabled={pending}
          >{pending ? "Saving…" : "Save"}</Button
        >
      </div>
    </form>
  {:else if error}
    <p role="status" class="mt-3 text-xs text-destructive">{error}</p>
  {:else if websites.length}
    <ul class="mt-2 flex min-w-0 flex-col">
      {#each websites as website (website.url)}
        {@const host = websiteHost(website.url)}
        <li class="min-w-0">
          <a
            href={website.url}
            target="_blank"
            rel="noopener noreferrer"
            class="group/link -mx-2 flex min-w-0 items-center gap-2 rounded-md px-2 py-1.5 hover:bg-muted/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            title={website.url}
          >
            <span class="flex min-w-0 flex-1 flex-col">
              <span class="truncate text-sm">{website.label}</span>
              {#if host && host !== website.label}
                <span class="truncate text-xs text-muted-foreground"
                  >{host}</span
                >
              {/if}
            </span>
            <ExternalLink
              aria-hidden="true"
              class="size-3 shrink-0 text-muted-foreground opacity-60 group-hover/link:opacity-100"
            />
            <span class="sr-only">(opens in a new tab)</span>
          </a>
        </li>
      {/each}
    </ul>
  {:else if pageState.repository?.can_manage}
    <Button
      variant="ghost"
      size="xs"
      class="-ml-2 mt-2 text-muted-foreground"
      onclick={() => void edit()}
      ><Plus data-icon="inline-start" />Add a website</Button
    >
  {:else}
    <p class="mt-2 text-xs text-muted-foreground">No websites added.</p>
  {/if}
</section>
