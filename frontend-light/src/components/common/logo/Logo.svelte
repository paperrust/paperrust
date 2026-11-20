<!-- Port of common/logo/logo.component.html (Patch 06, PaperRust-Logo). Host <pngx-logo> in the parent markup. -->
<script lang="ts">
  let { extraClasses = '', height = '6em', customLogo = null }: { extraClasses?: string; height?: string; customLogo?: string | null } = $props()
  const classes = $derived(['logo'].concat(extraClasses ? [extraClasses] : []).join(' '))
</script>

{#if customLogo}
  <img src={customLogo} class={classes} style={'height:' + height} alt="" />
{:else}
  <svg class={classes} xmlns="http://www.w3.org/2000/svg" viewBox="0 0 580 140" role="img" aria-label="PaperRust" style={'height:' + height}>
    <path class="paper" fill="#1F1A17" d="M24 12 H80 L104 36 V122 Q104 128 98 128 H30 Q24 128 24 122 V18 Q24 12 30 12 Z"/>
    <path class="corner" fill="#B7410E" d="M80 12 V30 Q80 36 86 36 H104 Z"/>
    <g class="lines" fill="#F4EFE6">
      <rect x="38" y="56" width="50" height="7" rx="3.5"/>
      <rect x="38" y="74" width="38" height="7" rx="3.5"/>
      <rect x="38" y="92" width="46" height="7" rx="3.5"/>
    </g>
    <text x="128" y="101" font-family="Archivo, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif" font-weight="800" font-size="80" letter-spacing="-1"><tspan class="text" fill="#1F1A17">Paper</tspan><tspan class="leaf" fill="#B7410E">Rust</tspan></text>
  </svg>
{/if}
