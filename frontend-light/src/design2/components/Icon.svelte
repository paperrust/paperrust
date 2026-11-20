<script lang="ts">
  import { ico } from '../lib/icons'
  let { name, size = undefined, label = undefined }: { name: string; size?: string; label?: string } = $props()
</script>

<span class="d2-ico" style:font-size={size} role={label ? 'img' : undefined} aria-label={label}>{@html ico(name)}</span>
