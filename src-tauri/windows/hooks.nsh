; NSIS installer hooks (#206), wired via bundle.windows.nsis.installerHooks.
;
; The uninstaller's "Delete the application data" checkbox only removes the
; identifier-named folders (WebView storage). The app's own data lives in
; product-named `college-course-map` folders, so remove those too, under the
; same conditions: checkbox ticked, and not an updater-driven reinstall.
; Config (settings, themes) is in %APPDATA%; data and caches are in
; %LOCALAPPDATA% (#205).
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    RMDir /r "$APPDATA\college-course-map"
    RMDir /r "$LOCALAPPDATA\college-course-map"
  ${EndIf}
!macroend
