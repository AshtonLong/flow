; Flow installer. Per-user, no administrator rights needed.
;
; It installs where the Electron build did (%LOCALAPPDATA%\Programs\Flow), under
; the same uninstall key and Start-menu shortcut, so it upgrades an Electron
; install in place. Settings, keys, history and models live elsewhere
; (%APPDATA%\Flow, %LOCALAPPDATA%\Flow) and are never touched.
;
; makensis /DVERSION=0.1.0 /DSTAGE=<staged app dir> /DOUTFILE=<installer path> flow.nsi

Unicode true
SetCompressor /SOLID lzma
RequestExecutionLevel user
ManifestDPIAware true

!ifndef VERSION
  !error "Pass /DVERSION=x.y.z"
!endif
!ifndef STAGE
  !error "Pass /DSTAGE=<staged app dir>"
!endif
!ifndef OUTFILE
  !define OUTFILE "Flow-Setup-${VERSION}.exe"
!endif

; electron-builder's key for appId computer.flow.dictation (a UUID v5 of it).
; Tests override it, SHORTCUT and RUN_VALUE, so a trial install leaves the real one alone.
!ifndef UNINSTALL_KEY
  !define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\cc05d316-dd59-5bb1-9f29-ab384ad1d9b1"
!endif
!ifndef SHORTCUT
  !define SHORTCUT "Flow"
!endif
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!ifndef RUN_VALUE
  !define RUN_VALUE "computer.flow.dictation"
!endif

Name "Flow"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\Flow"
InstallDirRegKey HKCU "${UNINSTALL_KEY}" "InstallLocation"
BrandingText "Flow ${VERSION}"
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "Flow"
VIAddVersionKey "FileDescription" "Flow Setup"
VIAddVersionKey "CompanyName" "Flow contributors"
VIAddVersionKey "LegalCopyright" "MIT License"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"

!include "MUI2.nsh"
!include "FileFunc.nsh"

!define MUI_ICON "..\..\resources\icons\icon.ico"
!define MUI_UNICON "..\..\resources\icons\icon.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\Flow.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start Flow"

!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

; Stops a running Flow from this folder only (never other programs named Flow.exe).
!macro StopFlow
  nsExec::Exec `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -Command "Get-CimInstance Win32_Process -Filter \"Name='Flow.exe'\" | Where-Object { $$_.ExecutablePath -like '$INSTDIR\*' } | ForEach-Object { Stop-Process -Id $$_.ProcessId -Force -ErrorAction SilentlyContinue }"`
  Pop $0
  Sleep 300
!macroend

Section "Flow" SecMain
  SetShellVarContext current
  !insertmacro StopFlow

  ; An Electron install: clear its Chromium payload first. User data lives elsewhere.
  IfFileExists "$INSTDIR\resources\app.asar" 0 +2
    RMDir /r "$INSTDIR"

  SetOutPath "$INSTDIR"
  File /r "${STAGE}\*.*"

  WriteUninstaller "$INSTDIR\Uninstall Flow.exe"
  CreateShortCut "$SMPROGRAMS\${SHORTCUT}.lnk" "$INSTDIR\Flow.exe" "" "$INSTDIR\Flow.exe" 0

  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "Flow"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "Flow contributors"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\Flow.exe,0"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\Uninstall Flow.exe" /currentuser'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall Flow.exe" /currentuser /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" "$0"

  ; An update (or --force-run) starts Flow again in the tray; a fresh silent install does not.
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "--force-run" $R1
  IfErrors done
    Exec '"$INSTDIR\Flow.exe" --hidden'
  done:
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  !insertmacro StopFlow
  Delete "$SMPROGRAMS\${SHORTCUT}.lnk"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  DeleteRegValue HKCU "${RUN_KEY}" "${RUN_VALUE}"
  RMDir /r "$INSTDIR"
SectionEnd
