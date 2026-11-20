<!-- Port of common/input/permissions/permissions-user (Host <pngx-permissions-user> in the parent markup). -->
<script lang="ts">
  import '../../../../../styles/components/permissions-user.css'
  import NgSelect from '../../../ng-select/NgSelect.svelte'
  import { data } from '../../../../../stores/data.svelte'
  let { value = $bindable([]), disabled = false, onchange }: { value?: number[]; disabled?: boolean; onchange?: (v: number[]) => void } = $props()
  const users = $derived([...data.users.values()])
</script>

<div class="paperless-input-select" class:disabled>
  <div>
    <NgSelect name="inputId" {value} onchange={(v) => { value = v; onchange?.(v) }}
      {disabled}
      clearable={true}
      items={users}
      multiple={true}
      bindLabel="username"
      bindValue="id">
    </NgSelect>
  </div>
</div>
