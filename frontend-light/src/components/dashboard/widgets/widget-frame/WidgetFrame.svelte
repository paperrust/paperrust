<!-- Port of dashboard/widgets/widget-frame/widget-frame.component.html. Host <pngx-widget-frame> in the
     parent markup; projections [title-badge]/[header-buttons]/[content] as snippets. cdkDrag: markup/classes only
     (reordering by dragging follows). -->
<script lang="ts">
  import '../../../../styles/components/widget-frame.css'
  import { onMount, type Snippet } from 'svelte'
  import { t } from '../../../../lib/i18n'
  import { icon } from '../../../common/bootstrap-icon/icon'

  let {
    loading = false,
    title = '',
    titleIcon = undefined,
    draggable = undefined,
    cardless = false,
    badge = null,
    titleBadge,
    headerButtons,
    children,
  }: {
    loading?: boolean
    title?: string
    titleIcon?: string
    draggable?: unknown
    cardless?: boolean
    badge?: string | number | null
    titleBadge?: Snippet
    headerButtons?: Snippet
    children: Snippet
  } = $props()

  let show = $state(false)
  onMount(() => (show = true))
</script>

{#if !cardless}
  <div class="card bg-light fade cdk-drag" class:cdk-drag-disabled={!draggable} class:show>
    <div class="card-header">
      <div class="d-flex justify-content-between align-items-center">
        <div class="d-flex align-items-center">
          {#if draggable}
            <div class="ms-n2 me-1 cdk-drag-handle">
              <pngx-bs-icon name="grip-vertical">{@html icon('grip-vertical')}</pngx-bs-icon>
            </div>
          {/if}
          <h6 class="card-title mb-0">
            {#if titleIcon}
              <pngx-bs-icon class="me-2">{@html icon(titleIcon)}</pngx-bs-icon>
            {/if}
            {title}
          </h6>
          {@render titleBadge?.()}
          {#if badge !== null && badge !== undefined}
            <span class="badge bg-info text-dark ms-2">{badge}</span>
          {/if}
        </div>
        {#if loading}
          <div class="spinner-border spinner-border-sm fw-normal ms-2 me-auto" role="status"></div>
          <div class="visually-hidden">{t('common.loading')}</div>
        {/if}
        {@render headerButtons?.()}
      </div>
    </div>
    <div class="card-body text-dark">
      {@render children()}
    </div>
  </div>
{:else}
  <div class="fade" class:show>
    {@render children()}
  </div>
{/if}
