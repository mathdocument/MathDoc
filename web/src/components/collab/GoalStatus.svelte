<script lang="ts">
  // A goal's status badge: the page's word for it, the API's in `data-status`, its meaning on hover.
  import { goalStatusHelp, goalStatusLabel, type Goal } from "../../lib/coordination";
  let { status }: { status: Goal["status"] } = $props();
</script>

<span class="status" data-status={status} title={goalStatusHelp[status]}>{goalStatusLabel[status]}</span>
