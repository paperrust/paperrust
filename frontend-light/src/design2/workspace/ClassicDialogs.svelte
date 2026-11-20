<!-- Output of the classic dialogs (PDF editor, redact, shrink, rotate, merge …) in the workspace
     of the PaperRust design. Loaded lazily only on the first opening of such a dialog (classic-dialogs.svelte.ts);
     style like in the administration: Bootstrap + classic.css (dialogs as side sheet). -->
<script lang="ts">
  import 'bootstrap/dist/css/bootstrap.css'
  import '../../styles/global.scss'
  import '../manage/classic.css'
  import ModalHost from '../../components/common/confirm-dialog/ModalHost.svelte'
</script>

<ModalHost />
