<!--
  A year of commit activity as a week-by-week grid, one cell per day, shaded
  by how busy the day was relative to the busiest one. Days with commits link
  to a list of them.
-->
<script lang="ts">
  import type { NamespaceActivity } from "$lib/api/profile.js";

  let {
    activity,
    selected = null,
    href,
  }: {
    activity: NamespaceActivity;
    /** The day being shown in detail, outlined in the grid. */
    selected?: string | null;
    /** Where a day with commits links to. */
    href: (date: string) => string;
  } = $props();

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

  // When the year is wider than the card, start scrolled to the selected day,
  // or else to the present.
  function scrollToLatest(node: HTMLElement): void {
    const current = node.querySelector<HTMLElement>("[aria-current]");
    node.scrollLeft = current
      ? current.offsetLeft - node.clientWidth / 2
      : node.scrollWidth;
  }
</script>

<div class="overflow-x-auto pb-1" {@attach scrollToLatest}>
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
            {#if cell && cell.count > 0}
              <a
                style:--stagger={column}
                class={[
                  "motion-cell rounded-[3px] outline-offset-1 hover:outline-2 hover:outline-foreground/60 focus-visible:outline-2 focus-visible:outline-ring",
                  LEVEL_CLASSES[cell.level],
                  cell.date === selected && "outline-2 outline-foreground",
                ]}
                href={href(cell.date)}
                title={describe(cell)}
                aria-label={describe(cell)}
                aria-current={cell.date === selected ? "date" : undefined}
                data-sveltekit-noscroll
              ></a>
            {:else if cell}
              <span
                style:--stagger={column}
                class={[
                  "motion-cell rounded-[3px]",
                  LEVEL_CLASSES[cell.level],
                ]}
                title={describe(cell)}
              ></span>
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
