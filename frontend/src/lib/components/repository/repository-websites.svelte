<script lang="ts">
  import ExternalLink from "@lucide/svelte/icons/external-link";
  import Globe from "@lucide/svelte/icons/globe";
  import Pencil from "@lucide/svelte/icons/pencil";
  import Plus from "@lucide/svelte/icons/plus";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import { z } from "zod";
  import { toast } from "svelte-sonner";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { errorMessage, jsonBody, requestJson } from "$lib/api/transport.js";
  import { repositoryApi } from "$lib/repository/state/shared.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";

  let { state: pageState }: { state: RepositoryPageState } = $props();
  const schema = z.array(z.object({ label: z.string(), url: z.string() }));
  let websites = $state<z.infer<typeof schema>>([]);
  let draft = $state<Array<{ id: string; label: string; url: string }>>([]);
  let editing = $state(false);
  let pending = $state(false);
  let error = $state("");

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
  function edit() {
    draft = websites.map((website) => ({
      ...website,
      id: String(++nextDraftId),
    }));
    if (!draft.length) add();
    editing = true;
  }
  function add() {
    draft.push({ id: String(++nextDraftId), label: "", url: "" });
  }
  async function save(event: SubmitEvent) {
    event.preventDefault();
    pending = true;
    try {
      websites = await requestJson(
        repositoryApi(pageState, "/websites"),
        schema,
        {
          method: "PUT",
          body: jsonBody({
            websites: draft.map(({ label, url }) => ({ label, url })),
          }),
        },
      );
      editing = false;
      error = "";
      toast.success("Website links saved.");
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
    {#if pageState.repository?.can_manage && !editing}
      <Button
        variant="ghost"
        size="icon-xs"
        aria-label="Edit website links"
        onclick={edit}><Pencil /></Button
      >
    {/if}
  </div>
  {#if editing}
    <form class="mt-3 flex flex-col gap-3" onsubmit={save}>
      <Field.Group>
        {#each draft as website, index (website.id)}
          <Field.Set disabled={pending} class="min-w-0">
            <Field.Legend variant="label">Website {index + 1}</Field.Legend>
            <div class="flex justify-end">
              <Button
                variant="ghost"
                size="icon-xs"
                type="button"
                aria-label={`Remove website ${index + 1}`}
                onclick={() =>
                  (draft = draft.filter((item) => item.id !== website.id))}
                ><Trash2 /></Button
              >
            </div>
            <Field.Group>
              <Field.Field>
                <Field.Label for={`website-label-${website.id}`}
                  >Name</Field.Label
                >
                <Input
                  id={`website-label-${website.id}`}
                  bind:value={website.label}
                  maxlength={80}
                  placeholder="Live demo"
                />
              </Field.Field>
              <Field.Field>
                <Field.Label for={`website-url-${website.id}`}>URL</Field.Label>
                <Input
                  id={`website-url-${website.id}`}
                  bind:value={website.url}
                  type="url"
                  required
                  maxlength={2048}
                  placeholder="https://example.com"
                />
              </Field.Field>
            </Field.Group>
          </Field.Set>
        {/each}
      </Field.Group>
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={pending || draft.length >= 10}
        onclick={add}><Plus data-icon="inline-start" />Add website</Button
      >
      <div class="flex flex-wrap justify-end gap-2">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={pending}
          onclick={() => (editing = false)}>Cancel</Button
        >
        <Button type="submit" size="sm" disabled={pending}
          >{pending ? "Saving…" : "Save"}</Button
        >
      </div>
    </form>
  {:else if error}
    <p role="status" class="mt-3 text-xs text-destructive">{error}</p>
  {:else if websites.length}
    <ul class="mt-3 flex min-w-0 flex-col gap-3">
      {#each websites as website (website.url)}
        <li class="min-w-0">
          <a
            href={website.url}
            target="_blank"
            rel="noopener noreferrer"
            class="flex min-w-0 items-start gap-2 text-sm hover:underline"
            title={website.url}
          >
            <Globe class="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
            <span class="min-w-0 flex-1 break-words [overflow-wrap:anywhere]"
              >{website.label}</span
            >
            <ExternalLink
              class="mt-0.5 size-3 shrink-0 text-muted-foreground"
            />
          </a>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="mt-3 text-xs text-muted-foreground">No websites added.</p>
  {/if}
</section>
