<script lang="ts">
  import type { RepositoryActivity } from "$lib/api.js";
  import ChartTooltip from "./chart-tooltip.svelte";

  let { activity }: { activity: RepositoryActivity } = $props();

  const DAY_MS = 86_400_000;
  const CHART_HEIGHT = 30;
  const CHART_TOP = 3;
  const CHART_BOTTOM = 27;
  const dateFormatter = new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    timeZone: "UTC",
  });

  type ChartPoint = {
    count: number;
    date: string;
    x: number;
    y: number;
  };

  let hovered = $state<number | null>(null);

  function smoothPath(points: ChartPoint[]): string {
    if (points.length === 0) return "";
    if (points.length === 1) return `M ${points[0]!.x} ${points[0]!.y}`;

    let path = `M ${points[0]!.x} ${points[0]!.y}`;
    for (let index = 0; index < points.length - 1; index += 1) {
      const previous = points[Math.max(0, index - 1)]!;
      const current = points[index]!;
      const next = points[index + 1]!;
      const following = points[Math.min(points.length - 1, index + 2)]!;
      const minimumY = Math.min(current.y, next.y);
      const maximumY = Math.max(current.y, next.y);
      const controlOneX = current.x + (next.x - previous.x) / 6;
      const controlOneY = Math.max(
        minimumY,
        Math.min(maximumY, current.y + (next.y - previous.y) / 6),
      );
      const controlTwoX = next.x - (following.x - current.x) / 6;
      const controlTwoY = Math.max(
        minimumY,
        Math.min(maximumY, next.y - (following.y - current.y) / 6),
      );
      path += ` C ${controlOneX} ${controlOneY}, ${controlTwoX} ${controlTwoY}, ${next.x} ${next.y}`;
    }
    return path;
  }

  let chart = $derived.by(() => {
    const start = Date.parse(`${activity.start_date}T00:00:00Z`);
    const end = Date.parse(`${activity.end_date}T00:00:00Z`);
    const dayCount = Math.max(1, Math.round((end - start) / DAY_MS) + 1);
    const counts = Array.from({ length: dayCount }, () => 0);
    for (const day of activity.days) {
      const index = Math.round(
        (Date.parse(`${day.date}T00:00:00Z`) - start) / DAY_MS,
      );
      if (index >= 0 && index < counts.length) counts[index] += day.count;
    }

    const peak = Math.max(0, ...counts);
    const scale = Math.max(1, peak);
    const points: ChartPoint[] = counts.map((count, index) => ({
      count,
      date: new Date(start + index * DAY_MS).toISOString().slice(0, 10),
      x: counts.length === 1 ? 50 : 2 + (index / (counts.length - 1)) * 96,
      y: CHART_BOTTOM - (count / scale) * (CHART_BOTTOM - CHART_TOP),
    }));
    const path = smoothPath(points);
    const first = points[0]!;
    const last = points.at(-1)!;

    return {
      area: `${path} L ${last.x} ${CHART_BOTTOM} L ${first.x} ${CHART_BOTTOM} Z`,
      dayCount,
      path,
      peak,
      points,
      range: `${dateFormatter.format(new Date(start))} – ${dateFormatter.format(new Date(end))}`,
    };
  });

  let reading = $derived(
    hovered === null ? null : (chart.points[hovered] ?? null),
  );
  let activeDays = $derived(chart.points.filter((point) => point.count > 0));

  // The plot insets each end by 2 viewBox units, so invert that before
  // snapping the pointer to the nearest day.
  function track(event: PointerEvent): void {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    if (bounds.width === 0) return;
    if (chart.points.length === 1) {
      hovered = 0;
      return;
    }

    const units = ((event.clientX - bounds.left) / bounds.width) * 100;
    const index = Math.round(((units - 2) / 96) * (chart.points.length - 1));
    hovered = Math.min(chart.points.length - 1, Math.max(0, index));
  }

  function dayLabel(point: ChartPoint): string {
    return `${point.count.toLocaleString()} commit${point.count === 1 ? "" : "s"} on ${dateFormatter.format(new Date(`${point.date}T00:00:00Z`))}`;
  }
</script>

<div class="min-w-0">
  <div
    class="text-right font-mono text-[10px] tabular-nums whitespace-nowrap text-muted-foreground"
    aria-hidden="true"
  >
    {activity.total_commits.toLocaleString()}
    commit{activity.total_commits === 1 ? "" : "s"}
  </div>

  <div
    class="relative mt-1"
    aria-hidden="true"
    onpointermove={track}
    onpointerleave={() => (hovered = null)}
  >
    <svg
      class="block h-10 w-full overflow-visible"
      viewBox={`0 0 100 ${CHART_HEIGHT}`}
      preserveAspectRatio="none"
    >
      <line
        x1="2"
        x2="98"
        y1={CHART_BOTTOM}
        y2={CHART_BOTTOM}
        class="stroke-border/70"
        vector-effect="non-scaling-stroke"
      />
      <path d={chart.area} class="fill-activity-3/12" />
      <path
        d={chart.path}
        fill="none"
        class="stroke-activity-4"
        stroke-width="1.75"
        stroke-linecap="round"
        stroke-linejoin="round"
        vector-effect="non-scaling-stroke"
      />
    </svg>

    {#if reading}
      <!-- Positioned in HTML rather than SVG: preserveAspectRatio="none"
           stretches the viewBox unevenly and would turn a circle into an
           ellipse. -->
      <span
        class="pointer-events-none absolute top-0 bottom-0 w-px bg-foreground/25"
        style:left={`${reading.x}%`}
        aria-hidden="true"
      ></span>
      <span
        class="pointer-events-none absolute size-1.5 -translate-x-1/2 -translate-y-1/2 rounded-full bg-activity-4 ring-2 ring-background"
        style:left={`${reading.x}%`}
        style:top={`${(reading.y / CHART_HEIGHT) * 100}%`}
        aria-hidden="true"
      ></span>
      <!-- Pinned to the right edge rather than following the pointer: the row
           is only wide enough for one placement that never clips, and a fixed
           anchor keeps the label from jittering while tracking. -->
      <ChartTooltip style="right:0;bottom:calc(100% + 0.25rem)">
        {dateFormatter.format(new Date(`${reading.date}T00:00:00Z`))} ·
        {reading.count.toLocaleString()}
        commit{reading.count === 1 ? "" : "s"}
      </ChartTooltip>
    {/if}
  </div>

  <div class="sr-only">
    <p>
      {activity.total_commits.toLocaleString()}
      commit{activity.total_commits === 1 ? "" : "s"} over {chart.dayCount} days,
      {chart.range}. Peak {chart.peak.toLocaleString()}
      commit{chart.peak === 1 ? "" : "s"} in one day.
    </p>
    <ul>
      {#each activeDays as point (point.date)}
        <li>{dayLabel(point)}</li>
      {/each}
    </ul>
  </div>
</div>
