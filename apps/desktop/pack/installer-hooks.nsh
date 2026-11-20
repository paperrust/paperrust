; PaperRust Desktop – NSIS hook for the Tauri bundler (bundle.windows.nsis.installerHooks).
;
; Finding from installer test 2026-11-19 (laptop): an update over an older version left the files of the
; previous version behind (UI with hash names: 756 files in light\assets and static\frontend). The
; new uninstaller only knows the files of the new package – after update + uninstall the
; program folder therefore stayed behind.
;
; Remedy: the subfolders that PaperRust brings itself are removed completely before installing and after
; uninstalling. Only these fixed names in the program folder ($INSTDIR) – never the
; data folder (%LOCALAPPDATA%\PaperRust, which is separate). Empty $INSTDIR → do nothing.

!macro PAPERRUST_REMOVE_OWN_DIRS
  ${If} "$INSTDIR" != ""
  ${AndIf} ${FileExists} "$INSTDIR\*.*"
    RMDir /r "$INSTDIR\light"
    RMDir /r "$INSTDIR\static"
    RMDir /r "$INSTDIR\pgsql"
    RMDir /r "$INSTDIR\tesseract"
    RMDir /r "$INSTDIR\pdfium"
    RMDir /r "$INSTDIR\fonts"
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; first check whether the app is still running (otherwise files would be locked and half-deleted)
  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  !insertmacro PAPERRUST_REMOVE_OWN_DIRS
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro PAPERRUST_REMOVE_OWN_DIRS
  ; remove the program folder only if it is empty now (the user's own files stay)
  ${If} "$INSTDIR" != ""
    RMDir "$INSTDIR"
  ${EndIf}
!macroend
