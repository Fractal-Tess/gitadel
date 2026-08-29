<script lang="ts">
  import { labelForeground } from "$lib/repository/format.js";
  import type { IssueLabel } from "$lib/api/issues.js";

  let { label }: { label: IssueLabel } = $props();
</script>

<span
  class="inline-flex max-w-48 items-center truncate rounded-full px-2 py-0.5 text-[11px] font-semibold"
  style:background-color={`#${label.color}`}
  style:color={labelForeground(label.color)}
  title={label.description || label.name}
>
  {label.name}
</span>
