<!-- Replica of <ngb-pagination> (ng-bootstrap 21): same classes/ARIA, page calculation like NgbPagination
     (maxSize, rotate, ellipses=true, boundaryLinks=false, directionLinks=true). Host in the parent markup. -->
<script lang="ts">
  import { paginationPages } from './pages'
  let {
    page = 1,
    pageSize,
    collectionSize,
    maxSize = 0,
    rotate = false,
    onpage,
  }: { page?: number; pageSize: number; collectionSize: number; maxSize?: number; rotate?: boolean; onpage: (p: number) => void } = $props()

  const pageCount = $derived(Math.max(Math.ceil(collectionSize / pageSize), 1))
  const pages = $derived(paginationPages(page, pageCount, maxSize, rotate, true))

  function go(e: Event, p: number) {
    e.preventDefault()
    if (p >= 1 && p <= pageCount && p !== page) onpage(p)
  }
</script>

<ul class="pagination pagination-sm"><li class="page-item" class:disabled={page <= 1}><a aria-label="Previous" href="" class="page-link" tabindex={page <= 1 ? -1 : undefined} aria-disabled={page <= 1 ? 'true' : undefined} onclick={(e) => go(e, page - 1)}><span aria-hidden="true">«</span></a></li>{#each pages as p}{#if p === -1}<li class="page-item disabled"><a class="page-link" tabindex="-1" aria-disabled="true">...</a></li>{:else}<li class="page-item" class:active={p === page}><a href="" class="page-link" aria-current={p === page ? 'page' : undefined} onclick={(e) => go(e, p)}>{p}</a></li>{/if}{/each}<li class="page-item" class:disabled={page >= pageCount}><a aria-label="Next" href="" class="page-link" tabindex={page >= pageCount ? -1 : undefined} aria-disabled={page >= pageCount ? 'true' : undefined} onclick={(e) => go(e, page + 1)}><span aria-hidden="true">»</span></a></li></ul>
