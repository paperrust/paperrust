<!-- Port of common/input/permissions/permissions-group (Host <pngx-permissions-group> in the parent markup). -->
<script lang="ts">
  import '../../../../../styles/components/permissions-group.css'
  import NgSelect from '../../../ng-select/NgSelect.svelte'
  import { groups } from './groups.svelte'
  let { value = $bindable([]), disabled = false, onchange }: { value?: number[]; disabled?: boolean; onchange?: (v: number[]) => void } = $props()
  groups.load()
</script>

<div class="paperless-input-select" class:disabled>
    <div>
      <NgSelect name="inputId" {value} onchange={(v) => { value = v; onchange?.(v) }}
        {disabled}
        clearable={true}
        items={groups.list}
        multiple={true}
        bindLabel="name"
        bindValue="id">
      </NgSelect>
    </div>
</div>
