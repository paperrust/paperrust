<!-- Port of common/hotkey-dialog (HotkeyDialogComponent). Opened by AppFrame with "?" (HotKeyService). -->
<script lang="ts">
  import { t } from '../../../lib/i18n'
  import type { ModalRef } from '../../../lib/modal.svelte'
  import { formatKey } from './hotkeys'

  let { modal, hotkeys = [] }: { modal: ModalRef<any>; hotkeys?: [string, string][] } = $props()
</script>

<pngx-hotkey-dialog><div class="modal-header">
    <h4 class="modal-title" id="modal-basic-title">{t('common.keyboard_shortcuts')}</h4>
    <button type="button" class="btn-close" aria-label="Close" onclick={() => modal.close()}></button>
</div>
<div class="modal-body">
    <table class="table">
        <tbody>
            {#each hotkeys as key (key[0])}
                <tr>
                  <td>{key[1]}</td>
                  <td class="d-flex justify-content-end">
                    <kbd>{formatKey(key[0])}</kbd>
                    {#if key[0].includes('control')}
                        &nbsp;(macOS&nbsp;<kbd>{formatKey(key[0], true)}</kbd>)
                    {/if}
                  </td>
                </tr>
            {/each}
        </tbody>
    </table>
</div>
<div class="modal-footer">
</div></pngx-hotkey-dialog>
