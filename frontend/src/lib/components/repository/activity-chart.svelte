<script lang="ts">
  import type { RepositoryActivity } from "$lib/api.js";
  import ChartTooltip from "./chart-tooltip.svelte";

  let { activity }: { activity: RepositoryActivity } = $props();

  type ChartDay = {
    date: string;
    count: number;
    outsideRange: boolean;
    latest: boolean;
  };

  type ChartWeek = {
    key: string;
    days: ChartDay[];
    month: string | null;
  };

  const DAY_MS = 86_400_000;
  const WEEKDAYS = ["S", "M", "T", "W", "T", "F", "S"];
  const dayFormatter = new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
    timeZone: "UTC",
  });
  const monthFormatter = new Intl.DateTimeFormat(undefined, {
    month: "short",
    timeZone: "UTC",
  });
  const openingFormatter = new Intl.DateTimeFormat(undefined, {
    month: "long",
    year: "numeric",
    timeZone: "UTC",
  });

  let maxCount = $derived(Math.max(0, ...activity.days.map((day) => day.count)));

  function lastDayOfMonth(date: Date): number {
    return new Date(
      Date.UTC(date.getUTCFullYear(), date.getUTCMonth() + 1, 0),
    ).getUTCDate();
  }

  function monthLabel(date: Date): string {
    return date.getUTCMonth() === 0
      ? `${monthFormatter.format(date)} ’${String(date.getUTCFullYear()).slice(2)}`
      : monthFormatter.format(date);
  }

  let weeks = $derived.by(() => {
    const counts = new Map(activity.days.map((day) => [day.date, day.count]));
    const start = new Date(`${activity.start_date}T00:00:00Z`);
    const end = new Date(`${activity.end_date}T00:00:00Z`);
    const chartStart = start.getTime() - start.getUTCDay() * DAY_MS;
    const chartEnd = end.getTime() + (6 - end.getUTCDay()) * DAY_MS;
    const result: ChartWeek[] = [];

    for (
      let weekStart = chartStart;
      weekStart <= chartEnd;
      weekStart += 7 * DAY_MS
    ) {
      const days: ChartDay[] = [];
      let month: string | null = null;

      for (let offset = 0; offset < 7; offset += 1) {
        const timestamp = weekStart + offset * DAY_MS;
        const date = new Date(timestamp);
        const key = date.toISOString().slice(0, 10);
        const outsideRange =
          timestamp < start.getTime() || timestamp > end.getTime();

        // Weeks read newest first, so descending the column moves back in
        // time. Marking the week that closes a month puts every rule exactly
        // on the boundary it names: below the rule is that month and earlier.
        if (!outsideRange && date.getUTCDate() === lastDayOfMonth(date)) {
          month = monthLabel(date);
        }

        days.push({
          date: key,
          count: counts.get(key) ?? 0,
          outsideRange,
          latest: key === activity.end_date,
        });
      }

      result.push({
        key: new Date(weekStart).toISOString().slice(0, 10),
        days,
        month,
      });
    }

    result.reverse();

    // The newest week never closes a month, so name it outright — the top of
    // the column is the part people read first and it needs an anchor.
    const newest = result[0];
    if (newest) {
      newest.month = monthLabel(new Date(`${activity.end_date}T00:00:00Z`));
    }

    return result;
  });

  let activeDays = $derived(
    weeks
      .flatMap((week) => week.days)
      .filter((day) => !day.outsideRange && day.count > 0),
  );

  let openedOn = $derived(
    openingFormatter.format(new Date(`${activity.start_date}T00:00:00Z`)),
  );

  let summary = $derived(
    `${activity.total_commits.toLocaleString()} commit${activity.total_commits === 1 ? "" : "s"} across ${weeks.length} weeks since ${openedOn}.${maxCount > 0 ? ` Busiest day: ${maxCount.toLocaleString()} commit${maxCount === 1 ? "" : "s"}.` : ""}`,
  );

  function level(count: number): number {
    if (count === 0 || maxCount === 0) return 0;
    return Math.max(
      1,
      Math.ceil((Math.log1p(count) / Math.log1p(maxCount)) * 4),
    );
  }

  function dayLabel(day: ChartDay): string {
    const date = dayFormatter.format(new Date(`${day.date}T00:00:00Z`));
    return `${day.count.toLocaleString()} commit${day.count === 1 ? "" : "s"} on ${date}`;
  }

  let panel = $state<HTMLElement | null>(null);
  let tip = $state<{ label: string; left: number; top: number } | null>(null);

  // The grid scrolls, so the tooltip lives outside it (a scroll container
  // would clip it) and is placed from measured coordinates instead.
  function peek(event: PointerEvent): void {
    const cell = (event.target as HTMLElement).closest<HTMLElement>(
      "[data-day]",
    );
    if (!cell || !panel) {
      tip = null;
      return;
    }

    const cellBox = cell.getBoundingClientRect();
    const panelBox = panel.getBoundingClientRect();
    tip = {
      label: cell.dataset.day ?? "",
      left: cellBox.left - panelBox.left,
      top: cellBox.top - panelBox.top + cellBox.height / 2,
    };
  }
</script>

<section
  class="spine relative flex h-full flex-col"
  aria-label={summary}
  bind:this={panel}
>
  <header class="shrink-0">
    <p
      class="font-mono text-[9px] font-medium uppercase tracking-[0.16em] text-muted-foreground"
    >
      Commit activity
    </p>
    <p class="mt-1.5 font-mono text-2xl leading-none tracking-tight tabular-nums">
      {activity.total_commits.toLocaleString()}
    </p>
    <p class="mt-1.5 text-[11px] leading-snug text-muted-foreground">
      commit{activity.total_commits === 1 ? "" : "s"} since {openedOn}
    </p>
  </header>

  <div
    class="mt-5 grid shrink-0 grid-cols-[var(--gutter)_repeat(7,var(--cell))] gap-x-[var(--gap)] pb-1.5 font-mono text-[9px] text-muted-foreground/60"
    aria-hidden="true"
  >
    <span></span>
    {#each WEEKDAYS as weekday, index (index)}
      <span class="text-center">{weekday}</span>
    {/each}
  </div>

  <div
    class="min-h-0 overflow-y-auto overscroll-contain pr-0.5"
    aria-hidden="true"
    onpointerover={peek}
    onpointerleave={() => (tip = null)}
    onscroll={() => (tip = null)}
  >
    {#each weeks as week (week.key)}
      <div
        class="grid grid-cols-[var(--gutter)_repeat(7,var(--cell))] items-center gap-x-[var(--gap)] py-[calc(var(--gap)/2)]"
        class:month-start={week.month !== null}
      >
        <span
          class="whitespace-nowrap pr-1.5 text-right font-mono text-[9px] leading-none text-muted-foreground/75"
          >{week.month ?? ""}</span
        >
        {#each week.days as day (day.date)}
          {#if day.outsideRange}
            <span class="size-[var(--cell)]"></span>
          {:else}
            <span
              class="cell size-[var(--cell)] rounded-[2px]"
              class:latest={day.latest}
              style:--fill={`var(--activity-${level(day.count)})`}
              data-day={dayLabel(day)}
            ></span>
          {/if}
        {/each}
      </div>
    {/each}
  </div>

  {#if maxCount > 0}
    <footer
      class="mt-2.5 flex shrink-0 items-center gap-1.5 font-mono text-[9px] tabular-nums text-muted-foreground/70"
      aria-hidden="true"
    >
      <span>1</span>
      {#each [1, 2, 3, 4] as step (step)}
        <span
          class="cell size-2 rounded-[2px]"
          style:--fill={`var(--activity-${step})`}
        ></span>
      {/each}
      <span>{maxCount.toLocaleString()} / day</span>
    </footer>
  {:else}
    <p class="mt-2.5 shrink-0 text-[10px] leading-snug text-muted-foreground">
      Push a commit to start the year.
    </p>
  {/if}

  {#if tip}
    <!-- Opens to the left: the panel sits against the right edge of the page,
         so that is the only direction with room. -->
    <ChartTooltip
      style={`left:${tip.left}px;top:${tip.top}px;transform:translate(calc(-100% - 0.4rem),-50%)`}
    >
      {tip.label}
    </ChartTooltip>
  {/if}

  <ul class="sr-only">
    {#each activeDays as day (day.date)}
      <li>{dayLabel(day)}</li>
    {/each}
  </ul>
</section>

<style>
  .spine {
    --cell: 0.55rem;
    --gap: 0.16rem;
    --gutter: 2.6rem;
  }

  .month-start {
    border-top: 1px solid color-mix(in oklab, var(--border) 70%, transparent);
  }

  .cell {
    background: var(--fill);
  }

  .cell.latest {
    outline: 1px solid oklch(0.85 0 0 / 0.55);
    outline-offset: 1px;
  }
</style>
