<!-- Replica of <color-slider> (ngx-color 10, SliderComponent): hue bar + 5 lightness fields, same DOM;
     the library styles (Emulated in ngx-color) are set as style attributes. Reports the new color as #rrggbb. -->
<script lang="ts">
  import { hexToRgb, rgbToHsl } from '../../../../lib/appearance'
  import { componentToHex, hslToRgb } from '../../../../lib/matching'
  let { color = '', onchange }: { color?: string; onchange?: (hex: string) => void } = $props()

  // ngx-color remembers h for grayscale; here the value computed from the color is enough
  const hsl = $derived.by(() => {
    if (!/^#[0-9a-f]{6}$/i.test(color ?? '')) return { h: 0, s: 0.5, l: 0.5 }
    const { r, g, b } = hexToRgb(color)
    const [h, s, l] = rgbToHsl(r, g, b)
    return { h: h * 360, s, l }
  })
  const SWATCHES = [0.8, 0.65, 0.5, 0.35, 0.2]
  const active = (l: number, s: number) => Math.round(hsl.l * 100) / 100 === l && Math.round(hsl.s * 100) / 100 === s
  const toHex = (h: number, s: number, l: number) => {
    const [r, g, b] = hslToRgb(h / 360, s, l)
    return `#${componentToHex(r)}${componentToHex(g)}${componentToHex(b)}`
  }
  let container: HTMLDivElement
  function hueAt(e: MouseEvent) {
    const rect = container.getBoundingClientRect()
    const left = e.pageX - (rect.left + window.pageXOffset)
    const h = left < 0 ? 0 : left > rect.width ? 359 : (360 * ((left * 100) / rect.width)) / 100
    if (h !== hsl.h) onchange?.(toHex(h, hsl.s, hsl.l))
  }
  function down(e: MouseEvent) {
    hueAt(e)
    const move = (ev: MouseEvent) => hueAt(ev)
    const up = () => {
      window.removeEventListener('mousemove', move)
      window.removeEventListener('mouseup', up)
    }
    window.addEventListener('mousemove', move)
    window.addEventListener('mouseup', up)
  }
</script>

<color-slider><div class="slider-picker"><div class="slider-hue" style={"height:12px;position:relative"}><color-hue><div class="color-hue color-hue-horizontal" style={"position:absolute;inset:0;border-radius:2px;background:linear-gradient(to right,red,#ff0 17%,#0f0 33%,#0ff,#00f 67%,#f0f 83%,red)"}><!-- svelte-ignore a11y_no_static_element_interactions --><div class="color-hue-container" style={"margin:0 2px;position:relative;height:100%"} bind:this={container} onmousedown={down}><div class="color-hue-pointer" style="position:absolute;left:{(hsl.h * 100) / 360}%"><div class="color-hue-slider" style={"margin-top:1px;box-shadow:0 1px 4px 0 rgba(0, 0, 0, 0.37);width:14px;height:14px;border-radius:6px;transform:translate(-7px, -2px);background-color:rgb(248, 248, 248)"}></div></div></div></div></color-hue></div><div class="slider-swatches"><color-slider-swatches><div class="slider-swatches" style={"margin-top:20px"}>{#each SWATCHES as l, i}<div class="slider-swatch-wrap" style={"box-sizing:border-box;width:20%;padding-right:1px;float:left"}><color-slider-swatch><!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions --><div class="slider-swatch" class:first={i === 0} class:last={i === 4} class:active={active(l, 0.5)} style="height:12px;cursor:pointer;background:hsl({hsl.h}, 50%, {l * 100}%);{i === 0 ? 'border-radius:2px 0 0 2px;' : ''}{i === 4 ? 'border-radius:0 2px 2px 0;' : ''}{active(l, 0.5) ? 'transform:scaleY(1.8);border-radius:3.6px/2px;' : ''}" onclick={() => onchange?.(toHex(hsl.h, 0.5, l))}></div></color-slider-swatch></div>{/each}</div></color-slider-swatches></div></div></color-slider>
