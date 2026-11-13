<!--
  A year of commit activity as a week-by-week grid, one cell per day, shaded
  by how busy the day was relative to the busiest one. Days with commits link
  to a list of them.
-->
<script lang="ts">
  import {
    namespaceDaySchema,
    type NamespaceActivity,
    type NamespaceDay,
  } from "$lib/api/profile.js";
  import { requestJson } from "$lib/api/transport.js";
  import { authorizationCacheScopeKey } from "$lib/cache-scope.js";
  import { summarizeActivityDay } from "$lib/profile-activity.js";
  import { useAppState } from "$lib/state/app-state.svelte.js";
  import * as HoverCard from "$lib/components/ui/hover-card/index.js";

  let {
    activity,
    namespace,
    selected = null,
    href,
  }: {
    activity: NamespaceActivity;
    namespace: string;
    /** The day being shown in detail, outlined in the grid. */
    selected?: string | null;
    /** Where a day with commits links to. */
    href: (date: string) => string;
  } = $props();

  const app = useAppState();
  let graph = $state<HTMLDivElement | null>(null);
  let openDate = $state<string | null>(null);
  let focusedDate = $state<string | null>(null);
  let details = $state.raw<NamespaceDay | null>(null);
  let detailsFailed = $state(false);
  const cache = new Map<string, NamespaceDay>();
  const context = $derived(
    `${namespace}\0${authorizationCacheScopeKey(app.authorizationScope)}`,
  );
  const tabDate = $derived(focusedDate ?? selected ?? activity.end_date);
  const summary = $derived(details ? summarizeActivityDay(details) : null);
  const timeFormat = new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    timeZone: "UTC",
  });

  $effect(() => {
    context;
    cache.clear();
    openDate = null;
    focusedDate = null;
  });
  $effect(() => {
    const date = openDate;
    const scope = context;
    details = null;
    detailsFailed = false;
    if (
      !date ||
      !activity.days.some((day) => day.date === date && day.count > 0)
    )
      return;
    const key = `${scope}\0${date}`;
    const cached = cache.get(key);
    if (cached) {
      details = cached;
      return;
    }
    const controller = new AbortController();
    // Brief pointer passes should not walk the namespace's Git histories.
    const timer = setTimeout(() => {
      requestJson(
        `/api/v1/namespaces/${encodeURIComponent(namespace)}/activity/${date}`,
        namespaceDaySchema,
        { signal: controller.signal },
      )
        .then((day) => {
          if (controller.signal.aborted) return;
          if (cache.size >= 30) cache.delete(cache.keys().next().value!);
          cache.set(key, day);
          details = day;
        })
        .catch(() => {
          if (!controller.signal.aborted) detailsFailed = true;
        });
    }, 150);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  });

  const DAY = 86_400_000;
  const CELL = 11;
  const GAP = 3;
  const MONTHS = [
    "Jan",
    "Feb",
    "Mar",
    "Apr",
    "May",
    "Jun",
    "Jul",
    "Aug",
    "Sep",
    "Oct",
    "Nov",
    "Dec",
  ];
  const LEVEL_CLASSES = [
    "bg-muted dark:bg-white/[0.06]",
    "bg-orange-500/40",
    "bg-orange-500/65",
    "bg-orange-500/85",
    "bg-orange-500",
  ];
  const dateFormat = new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeZone: "UTC",
  });

  function parseDate(value: string): number {
    const [year, month, day] = value.split("-").map(Number);
    return Date.UTC(year, month - 1, day);
  }

  type Cell = { date: string; count: number; level: number };

  const grid = $derived.by(() => {
    const counts = new Map(activity.days.map((day) => [day.date, day.count]));
    const busiest = Math.max(1, ...counts.values());
    const start = parseDate(activity.start_date);
    const end = parseDate(activity.end_date);
    // Columns are weeks that begin on Sunday, as on GitHub.
    const firstColumn = start - new Date(start).getUTCDay() * DAY;
    const weeks: (Cell | null)[][] = [];
    for (let column = firstColumn; column <= end; column += 7 * DAY) {
      const week: (Cell | null)[] = [];
      for (let row = 0; row < 7; row += 1) {
        const time = column + row * DAY;
        if (time < start || time > end) {
          week.push(null);
          continue;
        }
        const date = new Date(time).toISOString().slice(0, 10);
        const count = counts.get(date) ?? 0;
        const level = count === 0 ? 0 : Math.ceil((count / busiest) * 4);
        week.push({ date, count, level: Math.min(4, level) });
      }
      weeks.push(week);
    }

    // Name a month above the first week that reaches it, skipping any label
    // that would sit on top of the next one.
    const labels: { column: number; text: string }[] = [];
    let previousMonth = -1;
    weeks.forEach((week, column) => {
      const first = week.find((cell) => cell !== null);
      if (!first) return;
      const month = Number(first.date.slice(5, 7)) - 1;
      if (month === previousMonth) return;
      previousMonth = month;
      const last = labels.at(-1);
      if (last && column - last.column < 3) labels.pop();
      labels.push({ column, text: MONTHS[month] });
    });
    if (labels.length > 1 && labels[1].column < 3) labels.shift();
    return { weeks, labels };
  });

  function describe(cell: Cell): string {
    const day = dateFormat.format(parseDate(cell.date));
    if (cell.count === 0) return `No commits on ${day}`;
    return `${cell.count} commit${cell.count === 1 ? "" : "s"} on ${day}`;
  }

  function moveDay(event: KeyboardEvent, date: string) {
    const offset: Record<string, number> = {
      ArrowLeft: -7,
      ArrowRight: 7,
      ArrowUp: -1,
      ArrowDown: 1,
    };
    if (!(event.key in offset)) return;
    event.preventDefault();
    const next = Math.min(
      parseDate(activity.end_date),
      Math.max(
        parseDate(activity.start_date),
        parseDate(date) + offset[event.key] * DAY,
      ),
    );
    focusedDate = new Date(next).toISOString().slice(0, 10);
    const target = graph?.querySelector<HTMLAnchorElement>(
      `[data-contribution-date="${focusedDate}"]`,
    );
    target?.focus({ preventScroll: true });
    target?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }

  // When the year is wider than the card, start scrolled to the selected day,
  // or else to the present.
  function scrollToLatest(node: HTMLElement): void {
    const current = node.querySelector<HTMLElement>("[aria-current]");
    node.scrollLeft = current
      ? current.offsetLeft - node.clientWidth / 2
      : node.scrollWidth;
  }
</script>

<div bind:this={graph} class="overflow-x-auto pb-1" {@attach scrollToLatest}>
  <div class="mx-auto w-max">
    <div
      class="relative ml-8 h-4 text-[11px] text-muted-foreground"
      style:width={`${grid.weeks.length * (CELL + GAP)}px`}
      aria-hidden="true"
    >
      {#each grid.labels as label (label.column)}
        <span
          class="absolute top-0"
          style:left={`${label.column * (CELL + GAP)}px`}>{label.text}</span
        >
      {/each}
    </div>
    <div class="mt-1 flex gap-2">
      <div
        class="grid w-6 shrink-0 text-[11px] leading-none text-muted-foreground"
        style:grid-template-rows={`repeat(7, ${CELL}px)`}
        style:row-gap={`${GAP}px`}
        aria-hidden="true"
      >
        <span></span><span class="-translate-y-px">Mon</span><span></span><span
          class="-translate-y-px">Wed</span
        ><span></span><span class="-translate-y-px">Fri</span><span></span>
      </div>
      <div
        class="grid grid-flow-col"
        style:grid-template-rows={`repeat(7, ${CELL}px)`}
        style:grid-auto-columns={`${CELL}px`}
        style:gap={`${GAP}px`}
        role="group"
        aria-label={`${activity.total_commits} commits in the last year`}
      >
        {#each grid.weeks as week, column (column)}
          {#each week as cell, row (row)}
            {#if cell}
              <HoverCard.Root
                openDelay={200}
                closeDelay={150}
                open={openDate === cell.date}
                onOpenChange={(open) => {
                  if (open) openDate = cell.date;
                  else if (openDate === cell.date) openDate = null;
                }}
              >
                <HoverCard.Trigger
                  style={`--stagger:${column}`}
                  class={[
                    "motion-cell rounded-[3px] outline-offset-1 hover:outline-2 hover:outline-foreground/60 focus-visible:outline-2 focus-visible:outline-ring",
                    LEVEL_CLASSES[cell.level],
                    cell.date === selected && "outline-2 outline-foreground",
                  ]}
                  href={href(cell.date)}
                  aria-label={describe(cell)}
                  aria-current={cell.date === selected ? "date" : undefined}
                  data-contribution-date={cell.date}
                  tabindex={tabDate === cell.date ? 0 : -1}
                  onfocus={() => (focusedDate = cell.date)}
                  onkeydown={(event) => moveDay(event, cell.date)}
                  data-sveltekit-noscroll
                />
                <HoverCard.Content
                  side="top"
                  align="center"
                  sideOffset={8}
                  collisionPadding={12}
                  class="w-72 max-w-[calc(100vw-2rem)] p-3"
                  aria-label={`Activity on ${cell.date}`}
                >
                  {@const loaded = details?.date === cell.date ? details : null}
                  <p class="text-xs text-muted-foreground">
                    {dateFormat.format(parseDate(cell.date))} · UTC
                  </p>
                  <p class="mt-1 text-base font-semibold tabular-nums">
                    {(loaded?.total_commits ?? cell.count).toLocaleString()} commit{(loaded?.total_commits ??
                      cell.count) === 1
                      ? ""
                      : "s"}
                  </p>
                  {#if cell.count === 0}
                    <p class="mt-2 text-xs text-muted-foreground">
                      No commits recorded on this day.
                    </p>
                  {:else if loaded && summary}
                    <p class="mt-1 text-xs text-muted-foreground">
                      Across {loaded.repositories.length} repositor{loaded
                        .repositories.length === 1
                        ? "y"
                        : "ies"}
                    </p>
                    <ul
                      class="mt-3 flex flex-col gap-1.5"
                      aria-label="Commits by repository"
                    >
                      {#each summary.repositories.slice(0, 3) as repository (`${repository.namespace}/${repository.name}`)}
                        <li
                          class="flex min-w-0 items-center justify-between gap-3 text-xs"
                        >
                          <a
                            class="min-w-0 truncate hover:underline"
                            href={`/${encodeURIComponent(repository.namespace)}/${encodeURIComponent(repository.name)}?view=history`}
                            title={`${repository.namespace}/${repository.name}`}
                            >{repository.namespace}/{repository.name}</a
                          >
                          <span
                            class="shrink-0 tabular-nums text-muted-foreground"
                            >{repository.commits.length}</span
                          >
                        </li>
                      {/each}
                    </ul>
                    {#if loaded.repositories.length > 3}<p
                        class="mt-2 text-xs text-muted-foreground"
                      >
                        +{loaded.repositories.length - 3} more repositories
                      </p>{/if}
                    {#if summary.authors.length}
                      <p class="mt-3 text-xs text-muted-foreground">
                        {summary.authors.length === 1
                          ? "Author"
                          : "Contributors"}: {summary.authors
                          .slice(0, 3)
                          .map((author) => author.name)
                          .join(", ")}{summary.authors.length > 3
                          ? ` +${summary.authors.length - 3} more`
                          : ""}
                      </p>
                    {/if}
                    {#if summary.first !== null && summary.last !== null}<p
                        class="mt-1 text-xs text-muted-foreground"
                      >
                        {timeFormat.format(
                          summary.first * 1000,
                        )}{summary.last !== summary.first
                          ? `–${timeFormat.format(summary.last * 1000)}`
                          : ""} UTC
                      </p>{/if}
                  {:else if detailsFailed}
                    <p class="mt-2 text-xs text-muted-foreground">
                      Repository details could not be loaded. Open the day to
                      try again.
                    </p>
                  {:else}
                    <p class="mt-2 text-xs text-muted-foreground" role="status">
                      Loading repository details…
                    </p>
                  {/if}
                  <a
                    class="mt-3 block border-t pt-2 text-xs font-medium hover:underline"
                    href={href(cell.date)}>View this day's activity →</a
                  >
                </HoverCard.Content>
              </HoverCard.Root>
            {:else}
              <span></span>
            {/if}
          {/each}
        {/each}
      </div>
    </div>
    <div
      class="mt-3 flex items-center justify-end gap-1.5 text-[11px] text-muted-foreground"
      aria-hidden="true"
    >
      Less
      {#each LEVEL_CLASSES as levelClass (levelClass)}
        <span class={["size-[11px] rounded-[3px]", levelClass]}></span>
      {/each}
      More
    </div>
  </div>
</div>
