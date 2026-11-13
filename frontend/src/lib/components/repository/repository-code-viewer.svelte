<script lang="ts">
  import { onMount, tick } from "svelte";
  import { copyText } from "$lib/clipboard.js";
  import { toast } from "svelte-sonner";
  import Copy from "@lucide/svelte/icons/copy";
  import Link from "@lucide/svelte/icons/link";
  import ListTree from "@lucide/svelte/icons/list-tree";
  import Search from "@lucide/svelte/icons/search";
  import ChevronUp from "@lucide/svelte/icons/chevron-up";
  import ChevronDown from "@lucide/svelte/icons/chevron-down";
  import {
    blameSchema,
    codeSymbolsSchema,
    type Blob,
  } from "$lib/api/repositories.js";
  import { errorMessage, requestJson } from "$lib/api/transport.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import * as Field from "$lib/components/ui/field/index.js";
  import * as ToggleGroup from "$lib/components/ui/toggle-group/index.js";
  import IssueComposerDialog from "./issue-composer-dialog.svelte";
  import { trustedHtml, languageForPath } from "$lib/repository/format.js";
  import { repositoryApi } from "$lib/repository/state/shared.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";
  import {
    codePermalink,
    defaultReadingPreferences,
    highlightedLines,
    markdownSnippet,
    parseLineRange,
    rangeHash,
    readingPreferencesKey,
    readPreferences,
    searchMarkup,
    sourceLines,
    type LineRange,
  } from "$lib/repository/code-viewer.js";
  import type { z } from "zod";

  let { state: pageState, blob }: { state: RepositoryPageState; blob: Blob } =
    $props();
  let viewer = $state<HTMLElement | null>(null);
  let mode = $state("source");
  let range = $state<LineRange | null>(null);
  let anchor = $state<number | null>(null);
  let preferences = $state({ ...defaultReadingPreferences });
  let query = $state("");
  let searchInput = $state<HTMLInputElement | null>(null);
  let matchIndex = $state(0);
  let visibleStart = $state(0);
  const pageSize = 1000;
  let jumpOpen = $state(false);
  let jumpValue = $state("");
  let jumpError = $state("");
  let jumpInput = $state<HTMLInputElement | null>(null);
  let outlineOpen = $state(false);
  let outline = $state<z.infer<typeof codeSymbolsSchema> | null>(null);
  let outlineError = $state("");
  let blame = $state<z.infer<typeof blameSchema> | null>(null);
  let blameError = $state("");
  let blameLoading = $state(false);
  let issueOpen = $state(false);
  let issueBody = $state("");
  let issueTitle = $state("");

  const lines = $derived(sourceLines(blob.content ?? ""));
  const visibleLines = $derived(
    lines.slice(visibleStart, visibleStart + pageSize),
  );
  const highlighted = $derived(
    highlightedLines(pageState.browser.highlighted, lines.length),
  );
  const matches = $derived(
    query
      ? lines.flatMap((line, index) =>
          line.toLowerCase().includes(query.toLowerCase()) ? [index + 1] : [],
        )
      : [],
  );
  const activeMatch = $derived(
    matches.length ? matches[matchIndex % matches.length] : null,
  );
  const attribution = $derived(
    new Map(blame?.lines.map((line) => [line.line, line]) ?? []),
  );

  onMount(() => {
    try {
      preferences = readPreferences(
        localStorage.getItem(readingPreferencesKey),
      );
    } catch {
      /* Storage can be disabled. */
    }
    pageState.browser.wrapLines = preferences.wrap;
    mode =
      blob.rendered_html &&
      !new URLSearchParams(window.location.search).has("plain") &&
      !window.location.hash
        ? "preview"
        : "source";
    restoreSelection();
  });

  $effect(() => {
    if (mode !== "blame") return;
    const controller = new AbortController();
    blameLoading = true;
    blameError = "";
    requestJson(
      `${repositoryApi(pageState, "/blame")}?${new URLSearchParams({ rev: blob.commit_oid, path: blob.path })}`,
      blameSchema,
      { signal: controller.signal },
    )
      .then((value) => {
        if (!controller.signal.aborted) blame = value;
      })
      .catch((caught) => {
        if (!controller.signal.aborted) blameError = errorMessage(caught);
      })
      .finally(() => {
        if (!controller.signal.aborted) blameLoading = false;
      });
    return () => controller.abort();
  });
  $effect(() => {
    if (!outlineOpen) return;
    const controller = new AbortController();
    outlineError = "";
    requestJson(
      `${repositoryApi(pageState, "/symbols")}?${new URLSearchParams({ rev: blob.commit_oid, path: blob.path })}`,
      codeSymbolsSchema,
      { signal: controller.signal },
    )
      .then((value) => {
        if (!controller.signal.aborted) outline = value;
      })
      .catch((caught) => {
        if (!controller.signal.aborted) outlineError = errorMessage(caught);
      });
    return () => controller.abort();
  });

  function savePreferences() {
    pageState.browser.wrapLines = preferences.wrap;
    try {
      localStorage.setItem(readingPreferencesKey, JSON.stringify(preferences));
    } catch {
      /* Preferences still apply for this visit. */
    }
  }
  async function scrollToLine(line: number) {
    visibleStart = Math.floor((line - 1) / pageSize) * pageSize;
    await tick();
    viewer
      ?.querySelector(`[data-code-line="${line}"]`)
      ?.scrollIntoView({
        block: "center",
        inline: "nearest",
        behavior: "instant",
      });
  }
  function restoreSelection() {
    range = parseLineRange(window.location.hash, lines.length);
    anchor = range?.start ?? null;
    if (range) {
      if (mode === "preview") mode = "source";
      void scrollToLine(range.start);
    }
  }
  function selectLine(line: number, extend = false) {
    const first = extend && anchor ? anchor : line;
    if (!extend) anchor = line;
    range = { start: Math.min(first, line), end: Math.max(first, line) };
    const url = new URL(window.location.href);
    url.hash = rangeHash(range);
    window.history.replaceState(window.history.state, "", url);
  }
  function permalink() {
    return codePermalink(
      window.location.origin,
      pageState.namespace,
      pageState.name,
      blob.commit_oid,
      blob.path,
      range,
    );
  }
  function selectedCode() {
    return range
      ? lines.slice(range.start - 1, range.end).join("\n")
      : (blob.content ?? "");
  }
  async function copy(value: string, message: string) {
    try {
      await copyText(value);
      toast.success(message);
    } catch {
      toast.error("Could not copy to the clipboard.");
    }
  }
  function copyMarkdown() {
    void copy(
      markdownSnippet(
        selectedCode(),
        languageForPath(blob.path) ?? "",
        permalink(),
      ),
      "Markdown snippet copied.",
    );
  }
  function createIssue() {
    issueTitle = `Review ${blob.path}${range ? `:${range.start}-${range.end}` : ""}`;
    issueBody = markdownSnippet(
      selectedCode(),
      languageForPath(blob.path) ?? "",
      permalink(),
    );
    issueOpen = true;
  }
  async function changeMatch(direction: number) {
    if (!matches.length) return;
    matchIndex = (matchIndex + direction + matches.length) % matches.length;
    await scrollToLine(matches[matchIndex]);
  }
  function search(value: string) {
    query = value;
    matchIndex = 0;
    if (matches[0]) void scrollToLine(matches[0]);
  }
  async function openJump() {
    jumpOpen = true;
    jumpError = "";
    await tick();
    jumpInput?.focus();
  }
  function jump(event: SubmitEvent) {
    event.preventDefault();
    const selected = parseLineRange(jumpValue, lines.length);
    if (!selected) {
      jumpError = `Enter a line or range between 1 and ${lines.length}.`;
      return;
    }
    mode = "source";
    anchor = selected.start;
    range = selected;
    const url = new URL(window.location.href);
    url.hash = rangeHash(selected);
    window.history.replaceState(window.history.state, "", url);
    jumpOpen = false;
    void scrollToLine(selected.start);
  }
  function changeMode(value: string) {
    if (!value) return;
    mode = value;
    const url = new URL(window.location.href);
    if (value === "preview") {
      url.searchParams.delete("plain");
      url.hash = "";
      range = null;
      anchor = null;
    } else url.searchParams.set("plain", "1");
    window.history.replaceState(window.history.state, "", url);
  }
  async function keyboard(event: KeyboardEvent) {
    const target = event.target instanceof HTMLElement ? event.target : null;
    if (target?.closest('[role="dialog"]')) return;
    if (
      event.key === "f" &&
      (event.ctrlKey || event.metaKey) &&
      mode !== "preview" &&
      viewer?.contains(target)
    ) {
      event.preventDefault();
      searchInput?.focus();
      searchInput?.select();
      return;
    }
    if (
      target?.closest('input,textarea,select,[contenteditable="true"]') ||
      event.ctrlKey ||
      event.metaKey ||
      event.altKey
    )
      return;
    if (event.key === "l") {
      event.preventDefault();
      await openJump();
    }
    if (event.key === "y") {
      event.preventDefault();
      void copy(permalink(), "Permanent link copied.");
    }
  }
</script>

<svelte:window onhashchange={restoreSelection} onkeydown={keyboard} />

<div bind:this={viewer} class="flex min-h-0 min-w-0 flex-1 flex-col">
  <div
    class="flex shrink-0 flex-wrap items-center gap-2 border-b bg-muted/15 px-3 py-2"
  >
    <span class="mr-auto text-xs tabular-nums text-muted-foreground"
      >{lines.length.toLocaleString()} lines{range
        ? ` · ${range.end - range.start + 1} selected`
        : ""}</span
    >
    <ToggleGroup.Root
      type="single"
      value={mode}
      onValueChange={changeMode}
      variant="outline"
      size="sm"
      aria-label="File view"
    >
      {#if blob.rendered_html}<ToggleGroup.Item value="preview"
          >Preview</ToggleGroup.Item
        >{/if}
      <ToggleGroup.Item value="source">Source</ToggleGroup.Item>
      <ToggleGroup.Item value="blame">Blame</ToggleGroup.Item>
    </ToggleGroup.Root>
    <Button
      variant="ghost"
      size="sm"
      onclick={() =>
        pageState.navigate("history", {
          path: blob.path,
          rev: blob.commit_oid,
        })}>History</Button
    >
    <Button
      variant="ghost"
      size="icon-sm"
      aria-label="Copy file path"
      title="Copy file path"
      onclick={() => void copy(blob.path, "File path copied.")}><Copy /></Button
    >
    <Button
      variant="ghost"
      size="sm"
      onclick={() => void copy(blob.content ?? "", "File copied.")}
      >Copy file</Button
    >
    <Button
      variant="outline"
      size="sm"
      onclick={() => void copy(permalink(), "Permanent link copied.")}
      ><Link data-icon="inline-start" />Copy link</Button
    >
  </div>

  {#if mode !== "preview"}
    <div class="flex shrink-0 flex-wrap items-center gap-2 border-b px-3 py-2">
      <div class="flex min-w-0 flex-1 basis-48 items-center gap-1">
        <Search class="size-3.5 shrink-0 text-muted-foreground" />
        <Input
          bind:ref={searchInput}
          value={query}
          oninput={(event) => search(event.currentTarget.value)}
          aria-label="Find in file"
          placeholder="Find in file…"
          onkeydown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              void changeMatch(event.shiftKey ? -1 : 1);
            }
          }}
        />
        {#if query}<span
            class="shrink-0 text-xs tabular-nums text-muted-foreground"
            >{matches.length
              ? (matchIndex % matches.length) + 1
              : 0}/{matches.length}</span
          >{/if}
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Previous matching line"
          disabled={!matches.length}
          onclick={() => void changeMatch(-1)}><ChevronUp /></Button
        >
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Next matching line"
          disabled={!matches.length}
          onclick={() => void changeMatch(1)}><ChevronDown /></Button
        >
      </div>
      <Button
        variant="ghost"
        size="sm"
        onclick={openJump}
        disabled={!lines.length}>Go to line</Button
      >
      <Button
        variant="ghost"
        size="sm"
        aria-pressed={preferences.wrap}
        onclick={() => {
          preferences.wrap = !preferences.wrap;
          savePreferences();
        }}>Wrap</Button
      >
      <Button
        variant="ghost"
        size="sm"
        aria-pressed={outlineOpen}
        onclick={() => (outlineOpen = !outlineOpen)}
        ><ListTree data-icon="inline-start" />Outline</Button
      >
      <details class="relative text-xs">
        <summary class="cursor-pointer rounded px-2 py-1.5"
          >Reading preferences</summary
        >
        <Field.Group class="mt-2 min-w-40 rounded-md border bg-background p-3">
          <Field.Field
            ><Field.Label for="code-font-size">Font size</Field.Label><Input
              id="code-font-size"
              type="number"
              min={10}
              max={24}
              bind:value={preferences.fontSize}
              onchange={() => {
                preferences.fontSize = Math.min(
                  24,
                  Math.max(10, preferences.fontSize || 12),
                );
                savePreferences();
              }}
            /></Field.Field
          >
          <Field.Field
            ><Field.Label for="code-tab-size">Tab width</Field.Label><Input
              id="code-tab-size"
              type="number"
              min={2}
              max={8}
              bind:value={preferences.tabSize}
              onchange={() => {
                preferences.tabSize = Math.min(
                  8,
                  Math.max(2, Math.round(preferences.tabSize || 4)),
                );
                savePreferences();
              }}
            /></Field.Field
          >
        </Field.Group>
      </details>
    </div>
    {#if jumpOpen}
      <form
        onsubmit={jump}
        class="flex shrink-0 flex-wrap items-end gap-2 border-b px-3 py-2"
      >
        <Field.Group class="min-w-0 flex-1"
          ><Field.Field
            ><Field.Label for="jump-to-line">Line or range</Field.Label><Input
              id="jump-to-line"
              bind:ref={jumpInput}
              bind:value={jumpValue}
              placeholder="42 or 42-60"
              aria-invalid={!!jumpError}
            />{#if jumpError}<Field.Error>{jumpError}</Field.Error
              >{/if}</Field.Field
          ></Field.Group
        >
        <Button type="submit" size="sm">Go</Button><Button
          type="button"
          size="sm"
          variant="ghost"
          onclick={() => (jumpOpen = false)}>Cancel</Button
        >
      </form>
    {/if}
    {#if range}
      <div
        class="flex shrink-0 flex-wrap items-center gap-2 border-b bg-accent/35 px-3 py-2"
      >
        <span class="mr-auto text-xs"
          >{range.start === range.end
            ? `Line ${range.start}`
            : `Lines ${range.start}–${range.end}`}</span
        >
        <Button
          variant="outline"
          size="sm"
          onclick={() => void copy(selectedCode(), "Selected code copied.")}
          >Copy code</Button
        >
        <Button variant="outline" size="sm" onclick={copyMarkdown}
          >Copy Markdown</Button
        >
        {#if pageState.authStatus?.authenticated && !pageState.repository?.archived_at}<Button
            variant="outline"
            size="sm"
            onclick={createIssue}>Create issue</Button
          >{/if}
        <Button
          variant="ghost"
          size="sm"
          onclick={() => {
            range = null;
            anchor = null;
            const url = new URL(window.location.href);
            url.hash = "";
            window.history.replaceState(window.history.state, "", url);
          }}>Clear</Button
        >
      </div>
    {/if}
  {/if}

  {#if mode !== "preview" && lines.length > pageSize}
    <div
      class="flex shrink-0 flex-wrap items-center justify-between gap-2 border-b px-3 py-2 text-xs"
    >
      <span class="text-muted-foreground"
        >Showing lines {visibleStart + 1}–{Math.min(
          visibleStart + pageSize,
          lines.length,
        )} of {lines.length.toLocaleString()}</span
      >
      <div class="flex gap-2">
        <Button
          size="sm"
          variant="outline"
          disabled={visibleStart === 0}
          onclick={() =>
            void scrollToLine(Math.max(1, visibleStart - pageSize + 1))}
          >Previous lines</Button
        >
        <Button
          size="sm"
          variant="outline"
          disabled={visibleStart + pageSize >= lines.length}
          onclick={() => void scrollToLine(visibleStart + pageSize + 1)}
          >Next lines</Button
        >
      </div>
    </div>
  {/if}
  {#if mode === "preview" && blob.rendered_html}
    <div class="min-h-0 flex-1 overflow-auto">
      <div
        class="prose max-w-none p-6 prose-img:my-0 prose-img:inline-block prose-code:before:content-none prose-code:after:content-none dark:prose-invert lg:p-8"
        {@attach trustedHtml(blob.rendered_html, {
          namespace: pageState.namespace,
          name: pageState.name,
          revision: blob.revision,
          commit_oid: blob.commit_oid,
          path: blob.path,
        })}
      ></div>
    </div>
  {:else}
    <div class="flex min-h-0 min-w-0 flex-1 flex-col-reverse lg:flex-row">
      <!-- svelte-ignore a11y_no_noninteractive_tabindex (The scrollable source region needs keyboard access.) -->
      <div
        role="region"
        aria-label="File source; click a line number to select, Shift-click to select a range"
        tabindex="0"
        class="min-h-0 min-w-0 flex-1 overflow-auto bg-background/35 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      >
        {#if mode === "blame" && blameLoading}<p
            class="p-3 text-sm text-muted-foreground"
            role="status"
          >
            Loading line history…
          </p>{/if}
        {#if mode === "blame" && blameError}<p
            class="p-3 text-sm text-destructive"
            role="alert"
          >
            {blameError}
          </p>{/if}
        {#if !lines.length}<p class="p-5 text-sm text-muted-foreground">
            This file is empty.
          </p>{/if}
        <div
          class="py-3 font-mono leading-6"
          style:font-size={`${preferences.fontSize}px`}
          style:tab-size={preferences.tabSize}
        >
          {#each visibleLines as line, offset (visibleStart + offset)}
            {@const index = visibleStart + offset}
            {@const number = index + 1}
            {@const selected =
              range && number >= range.start && number <= range.end}
            {@const owner = mode === "blame" ? attribution.get(number) : null}
            {@const commit = owner ? blame?.commits[owner.commit_oid] : null}
            <div
              id={`L${number}`}
              data-code-line={number}
              class={[
                "flex min-h-6",
                preferences.wrap ? "min-w-0" : "min-w-max",
                selected
                  ? "bg-accent"
                  : activeMatch === number
                    ? "bg-muted"
                    : "",
              ]}
            >
              {#if mode === "blame"}
                <div
                  class="sticky left-0 flex w-44 shrink-0 items-center gap-2 overflow-hidden border-r bg-background px-2 text-[10px] text-muted-foreground"
                >
                  {#if owner && commit}
                    <button
                      type="button"
                      class="shrink-0 hover:underline"
                      title={commit.title}
                      onclick={() =>
                        pageState.navigate("commit", { oid: owner.commit_oid })}
                      >{owner.commit_oid.slice(0, 7)}</button
                    >
                    <span
                      class="truncate"
                      title={`${commit.author} · ${new Date(commit.timestamp * 1000).toLocaleString()} · ${commit.title}`}
                      >{commit.author}</span
                    >
                  {/if}
                </div>
              {/if}
              <button
                type="button"
                class="sticky left-0 min-w-12 shrink-0 select-none border-r bg-background px-3 text-right text-xs tabular-nums text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                style:left={mode === "blame" ? "11rem" : "0"}
                aria-label={`Select line ${number}`}
                aria-pressed={!!selected}
                onclick={(event) => selectLine(number, event.shiftKey)}
                >{number}</button
              >
              <code
                class={preferences.wrap
                  ? "min-w-0 flex-1 whitespace-pre-wrap break-words px-3 [overflow-wrap:anywhere]"
                  : "whitespace-pre px-3"}
                {@attach trustedHtml(
                  (query
                    ? searchMarkup(line, query)
                    : highlighted[index] || searchMarkup(line, "")) ||
                    "&#8203;",
                )}
              ></code>
            </div>
          {/each}
        </div>
      </div>
      {#if outlineOpen}
        <aside
          aria-label="File outline"
          class="max-h-48 shrink-0 overflow-y-auto border-b p-3 lg:max-h-none lg:w-56 lg:border-b-0 lg:border-l"
        >
          <p class="mb-2 text-xs font-semibold">Declarations</p>
          {#if outlineError}<p class="text-xs text-destructive">
              {outlineError}
            </p>
          {:else if !outline}<p
              role="status"
              class="text-xs text-muted-foreground"
            >
              Loading outline…
            </p>
          {:else if !outline.symbols.length}<p
              class="text-xs text-muted-foreground"
            >
              No supported declarations found.
            </p>
          {:else}<ul class="flex flex-col gap-1">
              {#each outline.symbols as symbol (`${symbol.line}:${symbol.name}`)}<li
                >
                  <button
                    type="button"
                    class="flex w-full min-w-0 items-center gap-2 rounded px-2 py-1 text-left text-xs hover:bg-muted"
                    title={`${symbol.kind} · line ${symbol.line}`}
                    onclick={() => {
                      selectLine(symbol.line);
                      void scrollToLine(symbol.line);
                    }}
                    ><span class="min-w-0 flex-1 truncate font-mono"
                      >{symbol.name}</span
                    ><span class="text-muted-foreground">{symbol.line}</span
                    ></button
                  >
                </li>{/each}
            </ul>{/if}
        </aside>
      {/if}
    </div>
  {/if}
</div>
<IssueComposerDialog
  bind:open={issueOpen}
  state={pageState}
  initialTitle={issueTitle}
  initialBody={issueBody}
/>

<style>
  code :global(mark) {
    background: var(--color-primary);
    color: var(--color-primary-foreground);
    border-radius: 2px;
  }
</style>
