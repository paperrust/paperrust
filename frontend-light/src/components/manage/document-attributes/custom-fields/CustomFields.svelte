<!-- Port of manage/document-attributes/custom-fields/custom-fields.component.html. Host <pngx-custom-fields>
     in the parent markup; state (CustomFieldsState) comes from the parent component. -->
<script lang="ts">
  import '../../../../styles/components/custom-fields.css'
  import { t } from '../../../../lib/i18n'
  import { icon } from '../../../common/bootstrap-icon/icon'
  import { Dropdown } from '../../../../lib/dropdown.svelte'
  import { router } from '../../../../router.svelte'
  import { session } from '../../../../stores/settings.svelte'
  import type { CustomField } from '../../../../api/v1'
  import { type CustomFieldsState, documentFilterPath } from './custom-fields.svelte'

  let { cf }: { cf: CustomFieldsState } = $props()
  const menus = new Map<number, Dropdown>()
  const menu = (id: number) => {
    let d = menus.get(id)
    if (!d) menus.set(id, (d = new Dropdown()))
    return d
  }
  function filterDocuments(e: Event, field: CustomField) {
    e.preventDefault()
    router.navigate(documentFilterPath(field))
  }
</script>

<ul class="list-group">

  <li class="list-group-item">
    <div class="row">
      <div class="col">{t('common.name')}</div>
      <div class="col">{t('common.data_type')}</div>
      <div class="col">{t('common.actions')}</div>
    </div>
  </li>

  {#if cf.loading}
    <li class="list-group-item">
      <div class="spinner-border spinner-border-sm me-2" role="status"></div>{t('common.loading')}
    </li>
  {/if}

  {#each cf.fields as field (field)}
    {@const dd = menu(field.id)}
    <li class="list-group-item">
      <div class="row fade" class:show={cf.show}>
        <div class="col d-flex align-items-center"><button class="btn btn-link p-0 text-start" type="button" onclick={() => cf.editField(field)} disabled={!session.can('change', 'customfield')}>{field.name}</button></div>
        <div class="col d-flex align-items-center">{cf.getDataType(field)}</div>
        <div class="col">
          <div class="btn-toolbar gap-2">
            <div class="btn-group d-block d-sm-none">
              <div class="d-inline-block {dd.placementClass}" class:show={dd.open} use:dd.hostEl>
                <button type="button" class="btn btn-link dropdown-toggle" id="actionsMenuMobile" onclick={(e) => { e.stopPropagation(); dd.toggle(e) }} aria-label={t('common.actions')} aria-expanded={dd.open} use:dd.anchorEl>
                  <pngx-bs-icon name="three-dots-vertical">{@html icon('three-dots-vertical')}</pngx-bs-icon>
                </button>
                {#if dd.open}
                  <div aria-labelledby="actionsMenuMobile" class="dropdown-menu show" use:dd.menuEl>
                    {#if session.can('change', 'customfield')}
                      <button onclick={() => cf.editField(field)} class="dropdown-item">{t('common.edit')}</button>
                    {/if}
                    {#if session.can('delete', 'customfield')}
                      <button class="text-danger dropdown-item" onclick={() => cf.deleteField(field)}>{t('common.delete')}</button>
                    {/if}
                    {#if (field.document_count ?? 0) > 0}
                      {#if session.can('view', 'document')}
                        <a class="dropdown-item" href={router.href(documentFilterPath(field))} onclick={(e) => filterDocuments(e, field)}>{t('manage.attributes.filter_documents', { INTERPOLATION: field.document_count })}</a>
                      {/if}
                    {/if}
                  </div>
                {/if}
              </div>
            </div>
            <div class="btn-group d-none d-sm-inline-block">
              {#if session.can('change', 'customfield')}
                <button class="btn btn-sm btn-outline-secondary" type="button" onclick={() => cf.editField(field)}>
                  <pngx-bs-icon width="1em" height="1em" name="pencil" class="me-1">{@html icon('pencil', '1em', '1em')}</pngx-bs-icon>{t('common.edit')}
                </button>
              {/if}
              {#if session.can('delete', 'customfield')}
                <button class="btn btn-sm btn-outline-danger" type="button" onclick={() => cf.deleteField(field)}>
                  <pngx-bs-icon width="1em" height="1em" name="trash" class="me-1">{@html icon('trash', '1em', '1em')}</pngx-bs-icon>{t('common.delete')}
                </button>
              {/if}
            </div>
            {#if (field.document_count ?? 0) > 0}
              <div class="btn-group d-none d-sm-inline-block">
                <a
                  class="btn btn-sm btn-outline-secondary"
                  href={router.href(documentFilterPath(field))}
                  onclick={(e) => filterDocuments(e, field)}
                >
                  <pngx-bs-icon width="1em" height="1em" name="filter" class="me-1">{@html icon('filter', '1em', '1em')}</pngx-bs-icon>{t('common.documents_2')}<span class="badge bg-light text-secondary ms-2">{field.document_count}</span>
                </a>
              </div>
            {/if}
          </div>
        </div>
      </div>
    </li>
  {/each}
  {#if !cf.loading && cf.fields.length === 0}
    <li class="list-group-item">{t('manage.attributes.no_fields_defined')}</li>
  {/if}
</ul>
