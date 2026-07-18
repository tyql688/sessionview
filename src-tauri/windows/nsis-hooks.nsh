; SessionView stores its database and image cache in %USERPROFILE%\.sessionview
; (shared with the headless server); %LOCALAPPDATA%\sessionview is the legacy
; location. Match Tauri's built-in "Delete application data" behavior for both
; without deleting data during updates or unconfirmed uninstalls.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    RMDir /r "$LOCALAPPDATA\sessionview"
    RMDir /r "$PROFILE\.sessionview"
  ${EndIf}
!macroend
