// Page list like NgbPagination._updatePages (ng-bootstrap): -1 = ellipsis "...".
export function paginationPages(page: number, pageCount: number, maxSize: number, rotate: boolean, ellipses: boolean): number[] {
  let pages = Array.from({ length: pageCount }, (_, i) => i + 1)
  page = Math.min(Math.max(page, 1), pageCount)
  if (maxSize > 0 && pageCount > maxSize) {
    let start = 0
    let end = pageCount
    if (rotate) {
      const leftOffset = Math.floor(maxSize / 2)
      const rightOffset = maxSize % 2 === 0 ? leftOffset - 1 : leftOffset
      if (page <= leftOffset) end = maxSize
      else if (pageCount - page < leftOffset) start = pageCount - maxSize
      else {
        start = page - leftOffset - 1
        end = page + rightOffset
      }
    } else {
      const p = Math.ceil(page / maxSize) - 1
      start = p * maxSize
      end = start + maxSize
    }
    pages = pages.slice(start, end)
    if (ellipses) {
      if (start > 0) {
        if (start > 2) pages.unshift(-1)
        else if (start === 2) pages.unshift(2)
        pages.unshift(1)
      }
      if (end < pageCount) {
        if (end < pageCount - 2) pages.push(-1)
        else if (end === pageCount - 2) pages.push(pageCount - 1)
        pages.push(pageCount)
      }
    }
  }
  return pages
}
