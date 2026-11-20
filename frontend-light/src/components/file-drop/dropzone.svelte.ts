// SettingsService.globalDropzoneEnabled / globalDropzoneActive (Angular signals)
class DropzoneState {
  enabled = $state(true)
  active = $state(false)
}

export const dropzone = new DropzoneState()
