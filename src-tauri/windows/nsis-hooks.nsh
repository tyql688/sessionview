; SessionView stores its database and image cache in
; %LOCALAPPDATA%\sessionview instead of Tauri's bundle-identifier directory.
; Match Tauri's built-in "Delete application data" behavior for that legacy
; location without deleting data during updates or unconfirmed uninstalls.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    RMDir /r "$LOCALAPPDATA\sessionview"
  ${EndIf}
!macroend
