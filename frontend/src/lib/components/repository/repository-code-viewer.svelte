<script lang="ts">
  import { onMount, tick, type Snippet } from "svelte";
  import { toast } from "svelte-sonner";
  import ChevronDown from "@lucide/svelte/icons/chevron-down";
  import ChevronUp from "@lucide/svelte/icons/chevron-up";
  import Copy from "@lucide/svelte/icons/copy";
  import CornerDownRight from "@lucide/svelte/icons/corner-down-right";
  import Ellipsis from "@lucide/svelte/icons/ellipsis";
  import ExternalLink from "@lucide/svelte/icons/external-link";
  import FileCode from "@lucide/svelte/icons/file-code";
  import FolderClock from "@lucide/svelte/icons/folder-clock";
  import Link from "@lucide/svelte/icons/link";
  import ListTree from "@lucide/svelte/icons/list-tree";
  import Minus from "@lucide/svelte/icons/minus";
  import Pencil from "@lucide/svelte/icons/pencil";
  import Plus from "@lucide/svelte/icons/plus";
  import Search from "@lucide/svelte/icons/search";
  import SlidersHorizontal from "@lucide/svelte/icons/sliders-horizontal";
  import SquarePen from "@lucide/svelte/icons/square-pen";
  import X from "@lucide/svelte/icons/x";
  import { copyText } from "$lib/clipboard.js";
  import {
    blameSchema,
    codeSymbolsSchema,
    type Blob,
  } from "$lib/api/repositories.js";
  import { errorMessage, requestJson } from "$lib/api/transport.js";
  import { formatBytes } from "$lib/format.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
  import { Kbd } from "$lib/components/ui/kbd/index.js";
  import * as Popover from "$lib/components/ui/popover/index.js";
  import { Switch } from "$lib/components/ui/switch/index.js";
  import * as ToggleGroup from "$lib/components/ui/toggle-group/index.js";
  import * as Tooltip from "$lib/components/ui/tooltip/index.js";
  import IssueComposerDialog from "./issue-composer-dialog.svelte";
  import {
    languageForPath,
    markdownHashTarget,
    trustedHtml,
  } from "$lib/repository/format.js";
  import { repositoryApi } from "$lib/repository/state/shared.js";
  import type { RepositoryPageState } from "$lib/repository/repository-page-state.svelte.js";
  import {
    blameHunkStarts,
    codePermalink,
    compactAge,
    defaultReadingPreferences,
    fontSizeLimits,
    highlightedLines,
    isLineHash,
    markMatches,
    markdownSnippet,
    parseLineRange,
    rangeHash,
    readingPreferencesKey,
    readPreferences,
    sourceLines,
    tabSizes,
    type LineRange,
  } from "$lib/repository/code-viewer.js";
  import type { z } from "zod";

  let {
    state: pageState,
    blob,
    title,
  }: { state: RepositoryPageState; blob: Blob; title: Snippet } = $props();

  type Mode = "preview" | "source" | "blame";
  type Panel = "find" | "line" | null;

  /** Rows rendered at once; longer files move this window rather than grow. */
  const pageSize = 1000;

  let viewer = $state<HTMLElement | null>(null);
  let region = $state<HTMLElement | null>(null);
  let mode = $state<Mode>("source");
  let range = $state<LineRange | null>(null);
  let anchor: number | null = null;
  let preferences = $state({ ...defaultReadingPreferences });
  let settingsOpen = $state(false);
  let panel = $state<Panel>(null);
  let query = $state("");
  let matchIndex = $state(0);
  let findInput = $state<HTMLInputElement | null>(null);
  let lineValue = $state("");
  let lineError = $state("");
  let lineInput = $state<HTMLInputElement | null>(null);
  let visibleStart = $state(0);
  let outlineOpen = $state(false);
  let outlineFilter = $state("");
  let outline = $state.raw<z.infer<typeof codeSymbolsSchema> | null>(null);
  let outlineError = $state("");
  let blame = $state.raw<z.infer<typeof blameSchema> | null>(null);
  let blameError = $state("");
  let blameLoading = $state(false);
  let issueOpen = $state(false);
  let issueBody = $state("");
  let issueTitle = $state("");
  // A mouse press on a line number selects at once and may drag a range; the
  // click that follows it must not select a second time.
  let dragging = false;
  let pointerSelected = false;

  const lines = $derived(sourceLines(blob.content ?? ""));
  const visibleLines = $derived(
    lines.slice(visibleStart, visibleStart + pageSize),
  );
  const highlighted = $derived(
    highlightedLines(pageState.browser.highlighted, lines.length),
  );
  const searching = $derived(panel === "find" && query.length > 0);
  const matches = $derived.by(() => {
    if (!searching) return [];
    const needle = query.toLowerCase();
    const found: number[] = [];
    lines.forEach((line, index) => {
      if (line.toLowerCase().includes(needle)) found.push(index + 1);
    });
    return found;
  });
  const activeMatch = $derived(
    matches.length ? matches[matchIndex % matches.length] : null,
  );
  const attribution = $derived(
    new Map(blame?.lines.map((line) => [line.line, line]) ?? []),
  );
  const hunkStarts = $derived(blameHunkStarts(blame?.lines ?? []));
  const gutterDigits = $derived(String(Math.max(lines.length, 1)).length);
  const rangeVisible = $derived(
    range !== null &&
      range.start > visibleStart &&
      range.start <= visibleStart + pageSize,
  );
  const tabbableLine = $derived(
    range && rangeVisible ? range.start : visibleStart + 1,
  );
  const canCreateIssue = $derived(
    !!pageState.authStatus?.authenticated &&
      !pageState.repository?.archived_at,
  );
  const symbols = $derived.by(() => {
    const filter = outlineFilter.trim().toLowerCase();
    const all = outline?.symbols ?? [];
    return filter
      ? all.filter((symbol) => symbol.name.toLowerCase().includes(filter))
      : all;
  });
  const rangeLabel = $derived(
    range
      ? range.start === range.end
        ? `Line ${range.start.toLocaleString()}`
        : `Lines ${range.start.toLocaleString()}–${range.end.toLocaleString()}`
      : "",
  );
  const shortcutModifier = $derived(
    typeof navigator !== "undefined" && /Mac|iP(?:hone|ad)/.test(navigator.platform)
      ? "⌘"
      : "Ctrl",
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
    // A rendered document opens rendered unless the link asks for its source
    // or points at lines. Heading hashes keep the rendered view.
    mode =
      blob.rendered_html &&
      !new URLSearchParams(window.location.search).has("plain") &&
      !isLineHash(window.location.hash)
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
    if (!outlineOpen || outline) return;
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
  function setFontSize(size: number) {
    preferences.fontSize = Math.min(
      fontSizeLimits.max,
      Math.max(fontSizeLimits.min, size),
    );
    savePreferences();
  }
  function resetPreferences() {
    preferences = { ...defaultReadingPreferences };
    savePreferences();
  }

  function lineRow(line: number) {
    return viewer?.querySelector<HTMLElement>(`[data-code-line="${line}"]`);
  }
  /** Moves the rendered window so `line` is in it. */
  async function reveal(line: number) {
    const start = Math.floor((line - 1) / pageSize) * pageSize;
    if (start !== visibleStart) {
      visibleStart = start;
      await tick();
    }
  }
  async function scrollToLine(line: number, block: ScrollLogicalPosition = "center") {
    await reveal(line);
    lineRow(line)?.scrollIntoView({
      block,
      inline: "nearest",
      behavior: "instant",
    });
  }
  function writeHash(hash: string) {
    const url = new URL(window.location.href);
    url.hash = hash;
    window.history.replaceState(window.history.state, "", url);
  }
  function restoreSelection() {
    const hash = window.location.hash;
    if (!isLineHash(hash)) {
      range = null;
      anchor = null;
      if (mode === "preview" && viewer)
        markdownHashTarget(viewer, hash)?.scrollIntoView({ block: "start" });
      return;
    }
    range = parseLineRange(hash, lines.length);
    anchor = range?.start ?? null;
    if (range) {
      if (mode === "preview") mode = "source";
      void scrollToLine(range.start);
    }
  }
  function selectLine(line: number, extend = false) {
    const first = extend && anchor ? anchor : line;
    if (!extend || !anchor) anchor = line;
    range = { start: Math.min(first, line), end: Math.max(first, line) };
    writeHash(rangeHash(range));
  }
  function clearSelection() {
    range = null;
    anchor = null;
    writeHash("");
  }

  function linePointerDown(event: PointerEvent, line: number) {
    pointerSelected = false;
    if (event.button !== 0 || event.pointerType === "touch") return;
    // Keep the press from starting a text selection across the gutter.
    event.preventDefault();
    (event.currentTarget as HTMLElement).focus({ preventScroll: true });
    selectLine(line, event.shiftKey);
    dragging = true;
    pointerSelected = true;
  }
  function linePointerEnter(line: number) {
    if (dragging) selectLine(line, true);
  }
  function lineClick(event: MouseEvent, line: number) {
    // `detail` is 0 for Enter and Space, which always select.
    if (pointerSelected && event.detail !== 0) {
      pointerSelected = false;
      return;
    }
    selectLine(line, event.shiftKey);
  }
  async function lineKeydown(event: KeyboardEvent, line: number) {
    const steps: Record<string, number> = {
      ArrowDown: 1,
      ArrowUp: -1,
      PageDown: 20,
      PageUp: -20,
    };
    let target: number | null = null;
    if (event.key in steps) target = line + steps[event.key];
    else if (event.key === "Home") target = 1;
    else if (event.key === "End") target = lines.length;
    if (target === null) return;
    event.preventDefault();
    target = Math.min(lines.length, Math.max(1, target));
    if (event.shiftKey) {
      if (!range) selectLine(line);
      selectLine(target, true);
    }
    await reveal(target);
    const button = lineRow(target)?.querySelector<HTMLElement>(
      "[data-line-number]",
    );
    button?.focus({ preventScroll: true });
    button?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }

  function permalink() {
    const hash = window.location.hash;
    return codePermalink(
      window.location.origin,
      pageState.namespace,
      pageState.name,
      blob.commit_oid,
      blob.path,
      mode === "preview" ? null : range,
      mode === "preview"
        ? { plain: false, hash: isLineHash(hash) ? "" : hash }
        : {},
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
  function copyLink() {
    void copy(
      permalink(),
      range && mode !== "preview"
        ? `Permanent link to ${rangeLabel.toLowerCase()} copied.`
        : "Permanent link copied.",
    );
  }
  function snippet() {
    return markdownSnippet(
      selectedCode(),
      languageForPath(blob.path) ?? "",
      permalink(),
    );
  }
  function createIssue() {
    issueTitle = `Review ${blob.path}${range ? `:${range.start}-${range.end}` : ""}`;
    issueBody = snippet();
    issueOpen = true;
  }

  async function openPanel(next: Exclude<Panel, null>) {
    if (next === "find" && mode === "preview") mode = "source";
    panel = next;
    lineError = "";
    await tick();
    const input = next === "find" ? findInput : lineInput;
    input?.focus();
    input?.select();
  }
  function closePanel() {
    panel = null;
    lineError = "";
    region?.focus({ preventScroll: true });
  }
  async function changeMatch(direction: number) {
    if (!matches.length) return;
    matchIndex = (matchIndex + direction + matches.length) % matches.length;
    await scrollToLine(matches[matchIndex]);
  }
  function search(value: string) {
    query = value;
    // Start from the first match at or below the top of the current view.
    const top = range?.start ?? visibleStart + 1;
    const next = matches.findIndex((line) => line >= top);
    matchIndex = Math.max(0, next);
    if (matches.length) void scrollToLine(matches[matchIndex]);
  }
  function jump(event: SubmitEvent) {
    event.preventDefault();
    const selected = parseLineRange(lineValue, lines.length);
    if (!selected) {
      lineError = `Enter a line or range from 1 to ${lines.length.toLocaleString()}.`;
      return;
    }
    if (mode === "preview") mode = "source";
    anchor = selected.start;
    range = selected;
    writeHash(rangeHash(selected));
    panel = null;
    lineValue = "";
    void scrollToLine(selected.start);
  }
  function changeMode(value: string) {
    if (!value) return;
    mode = value as Mode;
    const url = new URL(window.location.href);
    if (mode === "preview") {
      url.searchParams.delete("plain");
      if (isLineHash(url.hash)) url.hash = "";
      range = null;
      anchor = null;
      panel = null;
    } else {
      url.searchParams.set("plain", "1");
      if (!isLineHash(url.hash)) url.hash = "";
    }
    window.history.replaceState(window.history.state, "", url);
  }
  function openHistory() {
    pageState.navigate("history", { path: blob.path, rev: blob.commit_oid });
  }
  function openRaw() {
    if (pageState.rawUrl) window.open(pageState.rawUrl, "_blank", "noopener");
  }

  function keyboard(event: KeyboardEvent) {
    if (event.defaultPrevented) return;
    const target = event.target instanceof HTMLElement ? event.target : null;
    if (target?.closest('[role="dialog"],[role="menu"]')) return;
    const inViewer =
      !!target && (viewer?.contains(target) || target === document.body);
    if (
      event.key.toLowerCase() === "f" &&
      (event.ctrlKey || event.metaKey) &&
      !event.altKey &&
      inViewer &&
      lines.length
    ) {
      event.preventDefault();
      void openPanel("find");
      return;
    }
    if (event.key === "Escape" && inViewer) {
      if (panel) {
        event.preventDefault();
        closePanel();
      } else if (range && mode !== "preview") {
        event.preventDefault();
        clearSelection();
      }
      return;
    }
    if (
      target?.closest('input,textarea,select,[contenteditable="true"]') ||
      event.ctrlKey ||
      event.metaKey ||
      event.altKey
    )
      return;
    if (event.key === "l" && lines.length) {
      event.preventDefault();
      void openPanel("line");
    } else if (event.key === "y") {
      event.preventDefault();
      copyLink();
    }
  }
</script>

<svelte:window
  onhashchange={restoreSelection}
  onkeydown={keyboard}
  onpointerup={() => (dragging = false)}
  onpointercancel={() => (dragging = false)}
/>

{#snippet hint(label: string, keys?: string[])}
  {label}
  {#if keys}
    <span class="flex gap-0.5">
      {#each keys as key (key)}<Kbd>{key}</Kbd>{/each}
    </span>
  {/if}
{/snippet}

{#snippet toolButton(
  label: string,
  icon: typeof Search,
  onclick: () => void,
  options: { pressed?: boolean; keys?: string[] } = {},
)}
  {@const Icon = icon}
  <Tooltip.Root>
    <Tooltip.Trigger>
      {#snippet child({ props })}
        <Button
          {...props}
          variant="ghost"
          size="icon-sm"
          aria-label={label}
          aria-pressed={options.pressed}
          class={options.pressed ? "bg-muted text-foreground" : "text-muted-foreground"}
          {onclick}><Icon /></Button
        >
      {/snippet}
    </Tooltip.Trigger>
    <Tooltip.Content side="bottom">
      {@render hint(label, options.keys)}
    </Tooltip.Content>
  </Tooltip.Root>
{/snippet}

<div bind:this={viewer} class="flex min-h-0 min-w-0 flex-1 flex-col">
  <!-- One header: what the file is on the left, how to read it on the right.
       Occasional actions live in the overflow menu. -->
  <header
    class="flex min-h-11 shrink-0 flex-wrap items-center gap-x-3 gap-y-1.5 border-b px-4 py-1.5"
  >
    <div class="flex min-w-0 flex-1 basis-60 items-center gap-2 text-sm">
      {@render title()}
      <span
        class="hidden shrink-0 text-xs tabular-nums text-muted-foreground sm:inline"
        ><span
          >{lines.length.toLocaleString()}
          {lines.length === 1 ? "line" : "lines"}</span
        > · {formatBytes(blob.size)}</span
      >
    </div>
    <div class="ml-auto flex max-w-full flex-wrap items-center justify-end gap-1">
      {#if lines.length}
        <ToggleGroup.Root
          type="single"
          value={mode}
          onValueChange={changeMode}
          variant="outline"
          size="sm"
          aria-label="File view"
          class="mr-1"
        >
          {#if blob.rendered_html}<ToggleGroup.Item value="preview" class="px-2.5"
              >Preview</ToggleGroup.Item
            >{/if}
          <ToggleGroup.Item value="source" class="px-2.5">Source</ToggleGroup.Item>
          <ToggleGroup.Item value="blame" class="px-2.5">Blame</ToggleGroup.Item>
        </ToggleGroup.Root>
      {/if}
      {#if mode !== "preview" && lines.length}
        {@render toolButton(
          "Find in file",
          Search,
          () => (panel === "find" ? closePanel() : void openPanel("find")),
          { pressed: panel === "find", keys: [shortcutModifier, "F"] },
        )}
        {@render toolButton(
          "Outline",
          ListTree,
          () => (outlineOpen = !outlineOpen),
          { pressed: outlineOpen },
        )}
        <Popover.Root bind:open={settingsOpen}>
          <Popover.Trigger>
            {#snippet child({ props: popoverProps })}
              <Tooltip.Root>
                <Tooltip.Trigger {...popoverProps}>
                  {#snippet child({ props })}
                    <Button
                      {...props}
                      variant="ghost"
                      size="icon-sm"
                      aria-label="Display settings"
                      class={settingsOpen
                        ? "bg-muted text-foreground"
                        : "text-muted-foreground"}><SlidersHorizontal /></Button
                    >
                  {/snippet}
                </Tooltip.Trigger>
                <Tooltip.Content side="bottom">Display settings</Tooltip.Content>
              </Tooltip.Root>
            {/snippet}
          </Popover.Trigger>
          <Popover.Content align="end" class="w-64 gap-3 p-3">
            <p class="text-xs font-medium text-muted-foreground">Display</p>
            <label
              class="flex cursor-pointer items-center justify-between gap-3 text-sm"
            >
              Wrap long lines
              <Switch
                bind:checked={preferences.wrap}
                onCheckedChange={savePreferences}
                aria-label="Wrap long lines"
              />
            </label>
            <div class="flex items-center justify-between gap-3 text-sm">
              <span id="code-font-size-label">Font size</span>
              <div
                class="flex items-center gap-1"
                role="group"
                aria-labelledby="code-font-size-label"
              >
                <Button
                  variant="outline"
                  size="icon-xs"
                  aria-label="Smaller text"
                  disabled={preferences.fontSize <= fontSizeLimits.min}
                  onclick={() => setFontSize(preferences.fontSize - 1)}
                  ><Minus /></Button
                >
                <output
                  class="w-11 text-center text-xs tabular-nums"
                  aria-live="polite">{preferences.fontSize}px</output
                >
                <Button
                  variant="outline"
                  size="icon-xs"
                  aria-label="Larger text"
                  disabled={preferences.fontSize >= fontSizeLimits.max}
                  onclick={() => setFontSize(preferences.fontSize + 1)}
                  ><Plus /></Button
                >
              </div>
            </div>
            <div class="flex items-center justify-between gap-3 text-sm">
              <span id="code-tab-size-label">Tab width</span>
              <ToggleGroup.Root
                type="single"
                value={String(preferences.tabSize)}
                onValueChange={(value) => {
                  if (!value) return;
                  preferences.tabSize = Number(value);
                  savePreferences();
                }}
                variant="outline"
                size="sm"
                aria-labelledby="code-tab-size-label"
              >
                {#each tabSizes as size (size)}
                  <ToggleGroup.Item
                    value={String(size)}
                    aria-label={`${size} spaces`}
                    class="px-2.5 tabular-nums">{size}</ToggleGroup.Item
                  >
                {/each}
              </ToggleGroup.Root>
            </div>
            <Button
              variant="ghost"
              size="xs"
              class="self-start text-muted-foreground"
              onclick={resetPreferences}>Reset to defaults</Button
            >
          </Popover.Content>
        </Popover.Root>
      {/if}
      {@render toolButton("Copy permanent link", Link, copyLink, {
        keys: ["Y"],
      })}
      <DropdownMenu.Root>
        <DropdownMenu.Trigger>
          {#snippet child({ props })}
            <Button
              {...props}
              variant="ghost"
              size="icon-sm"
              class="text-muted-foreground"
              aria-label="More file actions"><Ellipsis /></Button
            >
          {/snippet}
        </DropdownMenu.Trigger>
        <DropdownMenu.Content align="end" class="w-56">
          {#if lines.length}
            <DropdownMenu.Item onSelect={() => void openPanel("line")}
              ><CornerDownRight />Go to line…<DropdownMenu.Shortcut
                >L</DropdownMenu.Shortcut
              ></DropdownMenu.Item
            >
          {/if}
          <DropdownMenu.Item onSelect={openHistory}
            ><FolderClock />File history</DropdownMenu.Item
          >
          <DropdownMenu.Separator />
          <DropdownMenu.Item
            onSelect={() => void copy(blob.content ?? "", "File contents copied.")}
            ><Copy />Copy file contents</DropdownMenu.Item
          >
          <DropdownMenu.Item
            onSelect={() => void copy(blob.path, "File path copied.")}
            ><FileCode />Copy path</DropdownMenu.Item
          >
          <DropdownMenu.Separator />
          <DropdownMenu.Item onSelect={openRaw}
            ><ExternalLink />Open raw file</DropdownMenu.Item
          >
        </DropdownMenu.Content>
      </DropdownMenu.Root>
      {#if pageState.canEditFile}
        <Button
          variant="ghost"
          size="sm"
          class="text-muted-foreground"
          aria-label="Edit file"
          onclick={() => pageState.startEditing()}
          ><Pencil data-icon="inline-start" /><span class="hidden sm:inline"
            >Edit</span
          ></Button
        >
      {/if}
    </div>
  </header>

  {#if mode === "preview" && blob.rendered_html}
    <div class="min-h-0 flex-1 xl:overflow-auto">
      <div
        class="prose max-w-none p-6 prose-img:my-0 prose-img:inline-block prose-code:before:content-none prose-code:after:content-none dark:prose-invert lg:p-8"
        {@attach trustedHtml(
          blob.rendered_html,
          {
            namespace: pageState.namespace,
            name: pageState.name,
            revision: blob.revision,
            commit_oid: blob.commit_oid,
            path: blob.path,
          },
          { headingAnchors: true },
        )}
      ></div>
    </div>
  {:else}
    <div class="flex min-h-0 min-w-0 flex-1 flex-col-reverse lg:flex-row">
      <div class="relative flex min-h-0 min-w-0 flex-1 flex-col">
        <!-- Zero-height sticky rails carry the overlays: they float over the
             code instead of pushing it down, and stay in view while a long
             file scrolls the page on narrow screens. -->
        <div class="sticky top-0 z-20 h-0">
          {#if panel === "find"}
            <div
              role="search"
              aria-label="Find in file"
              class="motion-fade absolute inset-x-3 top-2 flex items-center gap-1 rounded-lg border bg-popover py-1 pl-2.5 pr-1 shadow-lg sm:left-auto"
            >
              <Search class="size-3.5 shrink-0 text-muted-foreground" />
              <input
                bind:this={findInput}
                value={query}
                oninput={(event) => search(event.currentTarget.value)}
                onkeydown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    void changeMatch(event.shiftKey ? -1 : 1);
                  }
                }}
                aria-label="Find in file"
                placeholder="Find in file"
                class="h-7 min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground sm:w-52 sm:flex-none"
              />
              <span
                class="min-w-12 shrink-0 text-right text-xs tabular-nums text-muted-foreground"
                aria-live="polite"
                >{query
                  ? matches.length
                    ? `${(matchIndex % matches.length) + 1} of ${matches.length}`
                    : "No results"
                  : ""}</span
              >
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label="Previous match"
                title="Previous match (Shift+Enter)"
                disabled={!matches.length}
                onclick={() => void changeMatch(-1)}><ChevronUp /></Button
              >
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label="Next match"
                title="Next match (Enter)"
                disabled={!matches.length}
                onclick={() => void changeMatch(1)}><ChevronDown /></Button
              >
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label="Close find"
                title="Close (Esc)"
                onclick={closePanel}><X /></Button
              >
            </div>
          {:else if panel === "line"}
            <form
              onsubmit={jump}
              class="motion-fade absolute inset-x-3 top-2 flex flex-col gap-1 rounded-lg border bg-popover p-1.5 shadow-lg sm:left-auto"
            >
              <div class="flex items-center gap-1">
                <CornerDownRight
                  class="ml-1 size-3.5 shrink-0 text-muted-foreground"
                />
                <input
                  bind:this={lineInput}
                  bind:value={lineValue}
                  aria-label="Line or range"
                  aria-invalid={!!lineError}
                  aria-describedby="go-to-line-hint"
                  placeholder={`Line or range, 1–${lines.length.toLocaleString()}`}
                  inputmode="numeric"
                  class="h-7 min-w-0 flex-1 bg-transparent px-1 text-sm outline-none placeholder:text-muted-foreground sm:w-52 sm:flex-none"
                />
                <Button type="submit" size="xs">Go</Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  aria-label="Close go to line"
                  onclick={closePanel}><X /></Button
                >
              </div>
              <p
                id="go-to-line-hint"
                class={[
                  "px-1 text-xs",
                  lineError ? "text-destructive" : "text-muted-foreground",
                ]}
                role={lineError ? "alert" : undefined}
              >
                {lineError || "For example 42 or 42-60"}
              </p>
            </form>
          {/if}
        </div>

        <!-- svelte-ignore a11y_no_noninteractive_tabindex (The scrollable source region needs keyboard access.) -->
        <div
          bind:this={region}
          role="region"
          aria-label="File source. Line numbers select lines: Shift extends a selection, arrow keys move between lines."
          tabindex="0"
          class="min-h-0 min-w-0 flex-1 overflow-auto bg-background/35 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
        >
          {#if mode === "blame" && blameLoading}<p
              class="sticky left-0 px-4 pt-3 text-xs text-muted-foreground"
              role="status"
            >
              Loading line history…
            </p>{/if}
          {#if mode === "blame" && blameError}<p
              class="sticky left-0 px-4 pt-3 text-sm text-destructive"
              role="alert"
            >
              {blameError}
            </p>{/if}
          {#if !lines.length}<p class="p-5 text-sm text-muted-foreground">
              This file is empty.
            </p>{/if}
          {#if visibleStart > 0}
            <div
              class="sticky left-0 flex items-center gap-3 border-b px-4 py-1.5 text-xs text-muted-foreground"
            >
              <Button
                variant="ghost"
                size="xs"
                onclick={() => void scrollToLine(visibleStart, "end")}
                ><ChevronUp data-icon="inline-start" />Show lines {Math.max(
                  1,
                  visibleStart - pageSize + 1,
                ).toLocaleString()}–{visibleStart.toLocaleString()}</Button
              >
              <span class="tabular-nums"
                >{(visibleStart + 1).toLocaleString()}–{Math.min(
                  visibleStart + pageSize,
                  lines.length,
                ).toLocaleString()} of {lines.length.toLocaleString()}</span
              >
            </div>
          {/if}
          <div
            class={["py-2 font-mono leading-[1.7]", range && "pb-20"]}
            style:font-size={`${preferences.fontSize}px`}
            style:tab-size={preferences.tabSize}
            style:--gutter={`calc(${gutterDigits}ch + 1.75rem)`}
          >
            {#each visibleLines as line, offset (visibleStart + offset)}
              {@const index = visibleStart + offset}
              {@const number = index + 1}
              {@const selected =
                range !== null && number >= range.start && number <= range.end}
              {@const owner = mode === "blame" ? attribution.get(number) : null}
              {@const commit = owner ? blame?.commits[owner.commit_oid] : null}
              {@const hunkStart = mode === "blame" && hunkStarts.has(number)}
              <div
                id={`L${number}`}
                data-code-line={number}
                class={[
                  "flex",
                  preferences.wrap ? "min-w-0" : "min-w-max",
                  hunkStart && number > 1 && "border-t border-border/60",
                  selected
                    ? "bg-accent"
                    : activeMatch === number
                      ? "bg-muted/70"
                      : "hover:bg-muted/25",
                ]}
              >
                <div
                  class={[
                    "sticky left-0 z-10 flex shrink-0 border-r",
                    selected
                      ? "bg-accent shadow-[inset_2px_0_0_var(--color-foreground)]"
                      : "bg-background",
                  ]}
                >
                  {#if mode === "blame"}
                    <div
                      class="flex w-28 shrink-0 items-center gap-2 overflow-hidden border-r px-2 font-sans text-[0.85em] text-muted-foreground sm:w-60"
                    >
                      {#if owner && commit && hunkStart}
                        <button
                          type="button"
                          class="shrink-0 font-mono hover:text-foreground hover:underline"
                          title={`${commit.title}\n${commit.author} · ${new Date(commit.timestamp * 1000).toLocaleString()}`}
                          onclick={() =>
                            pageState.navigate("commit", {
                              oid: owner.commit_oid,
                            })}>{owner.commit_oid.slice(0, 7)}</button
                        >
                        <span class="hidden min-w-0 flex-1 truncate sm:inline"
                          >{commit.author}</span
                        >
                        <span
                          class="ml-auto shrink-0 tabular-nums"
                          title={new Date(commit.timestamp * 1000).toLocaleString()}
                          >{compactAge(commit.timestamp)}</span
                        >
                      {/if}
                    </div>
                  {/if}
                  <button
                    type="button"
                    data-line-number
                    tabindex={number === tabbableLine ? 0 : -1}
                    class={[
                      "shrink-0 cursor-pointer select-none px-3 text-right text-[0.85em] tabular-nums hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
                      selected ? "text-foreground" : "text-muted-foreground",
                    ]}
                    style:width="var(--gutter)"
                    aria-label={`Select line ${number}`}
                    aria-pressed={selected}
                    onpointerdown={(event) => linePointerDown(event, number)}
                    onpointerenter={() => linePointerEnter(number)}
                    onclick={(event) => lineClick(event, number)}
                    onkeydown={(event) => void lineKeydown(event, number)}
                    >{number}</button
                  >
                </div>
                <code
                  class={preferences.wrap
                    ? "min-w-0 flex-1 whitespace-pre-wrap break-words px-4 [overflow-wrap:anywhere]"
                    : "whitespace-pre px-4"}
                  {@attach trustedHtml(
                    (searching
                      ? markMatches(highlighted[index] ?? "", query)
                      : highlighted[index]) || "&#8203;",
                  )}
                ></code>
              </div>
            {/each}
          </div>
          {#if visibleStart + pageSize < lines.length}
            <div
              class="sticky left-0 flex items-center gap-3 border-t px-4 py-1.5 text-xs text-muted-foreground"
            >
              <Button
                variant="ghost"
                size="xs"
                onclick={() =>
                  void scrollToLine(visibleStart + pageSize + 1, "start")}
                ><ChevronDown data-icon="inline-start" />Show lines {(
                  visibleStart +
                  pageSize +
                  1
                ).toLocaleString()}–{Math.min(
                  visibleStart + 2 * pageSize,
                  lines.length,
                ).toLocaleString()}</Button
              >
              <span class="tabular-nums"
                >{(visibleStart + 1).toLocaleString()}–{(
                  visibleStart + pageSize
                ).toLocaleString()} of {lines.length.toLocaleString()}</span
              >
            </div>
          {/if}
        </div>

        <div class="sticky bottom-0 z-20 h-0">
          {#if range}
            <div
              class="pointer-events-none absolute inset-x-0 bottom-3 flex justify-center px-3"
            >
              <div
                role="toolbar"
                aria-label={`${rangeLabel} selected`}
                class="motion-fade pointer-events-auto flex max-w-full items-center gap-0.5 overflow-x-auto rounded-lg border bg-popover p-1 shadow-lg"
              >
                <span
                  class="shrink-0 px-2 text-xs font-medium tabular-nums"
                  aria-live="polite">{rangeLabel}</span
                >
                <span class="mx-0.5 h-4 w-px shrink-0 bg-border"></span>
                <Button
                  variant="ghost"
                  size="sm"
                  aria-label="Copy code"
                  onclick={() => void copy(selectedCode(), `${rangeLabel} copied.`)}
                  ><Copy /><span class="hidden sm:inline">Copy</span></Button
                >
                <Button
                  variant="ghost"
                  size="sm"
                  aria-label="Copy as Markdown"
                  title="Fenced code block with a permanent link"
                  onclick={() => void copy(snippet(), "Markdown snippet copied.")}
                  ><FileCode /><span class="hidden sm:inline">Markdown</span></Button
                >
                <Button
                  variant="ghost"
                  size="sm"
                  aria-label="Copy link"
                  onclick={copyLink}
                  ><Link /><span class="hidden sm:inline">Link</span></Button
                >
                {#if canCreateIssue}
                  <Button
                    variant="ghost"
                    size="sm"
                    aria-label="Create issue"
                    onclick={createIssue}
                    ><SquarePen /><span class="hidden sm:inline">Issue</span></Button
                  >
                {/if}
                <span class="mx-0.5 h-4 w-px shrink-0 bg-border"></span>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label="Clear selection"
                  title="Clear selection (Esc)"
                  onclick={clearSelection}><X /></Button
                >
              </div>
            </div>
          {/if}
        </div>
      </div>

      {#if outlineOpen}
        <aside
          aria-label="File outline"
          class="flex max-h-56 shrink-0 flex-col border-b lg:max-h-none lg:w-60 lg:border-b-0 lg:border-l"
        >
          <div class="flex items-center justify-between gap-2 px-3 pb-1 pt-2">
            <p class="text-xs font-medium text-muted-foreground">Outline</p>
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label="Close outline"
              onclick={() => (outlineOpen = false)}><X /></Button
            >
          </div>
          {#if (outline?.symbols.length ?? 0) > 8}
            <div class="px-3 pb-2">
              <input
                bind:value={outlineFilter}
                aria-label="Filter declarations"
                placeholder="Filter declarations"
                class="h-7 w-full rounded-md border bg-transparent px-2 text-xs outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring"
              />
            </div>
          {/if}
          <div class="min-h-0 flex-1 overflow-y-auto px-1.5 pb-2">
            {#if outlineError}<p class="px-1.5 text-xs text-destructive">
                {outlineError}
              </p>
            {:else if !outline}<p
                role="status"
                class="px-1.5 text-xs text-muted-foreground"
              >
                Loading outline…
              </p>
            {:else if !outline.symbols.length}<p
                class="px-1.5 text-xs text-muted-foreground"
              >
                No declarations found in this file.
              </p>
            {:else if !symbols.length}<p
                class="px-1.5 text-xs text-muted-foreground"
              >
                No declarations match.
              </p>
            {:else}<ul class="flex flex-col">
                {#each symbols as symbol (`${symbol.line}:${symbol.name}`)}<li>
                    <button
                      type="button"
                      class={[
                        "flex w-full min-w-0 items-center gap-2 rounded px-1.5 py-1 text-left text-xs hover:bg-muted",
                        range?.start === symbol.line && "bg-muted",
                      ]}
                      title={`${symbol.kind} · line ${symbol.line}`}
                      onclick={() => {
                        selectLine(symbol.line);
                        void scrollToLine(symbol.line, "start");
                      }}
                      ><span
                        class="w-12 shrink-0 truncate text-[10px] text-muted-foreground"
                        >{symbol.kind}</span
                      ><span class="min-w-0 flex-1 truncate font-mono"
                        >{symbol.name}</span
                      ><span class="tabular-nums text-muted-foreground"
                        >{symbol.line}</span
                      ></button
                    >
                  </li>{/each}
              </ul>{/if}
          </div>
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
    /* Search hits are the one place the viewer uses colour of its own, so
       they read against both syntax colours and the selection. */
    background: oklch(0.78 0.15 80 / 0.32);
    color: inherit;
    border-radius: 2px;
    box-shadow: 0 0 0 1px oklch(0.78 0.15 80 / 0.55);
  }
</style>
