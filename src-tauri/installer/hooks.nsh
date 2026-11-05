; Lumen installer hooks (bundle > windows > nsis > installerHooks in tauri.conf.json).
;
; Lumen keeps its data (models, chats, settings, llama.cpp runtime) in LumenData next to the exe,
; which for an installed copy is inside $INSTDIR. Tauri's uninstaller only removes the files it
; installed, and its "Delete the application data" checkbox only covers %APPDATA%\<identifier>, so
; LumenData (often 10+ GB of models) was always left behind. Remove it when that box is ticked.
; Updates never tick it, so an update keeps everything.

; (llama-server needs no handling: the uninstaller closes Lumen first, and Lumen's job object stops it.)
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
    RMDir /r "$INSTDIR\LumenData"
    RMDir "$INSTDIR"
  ${EndIf}
!macroend
