; SPDX-License-Identifier: GPL-2.0-only
;
; Per-user Windows installer for Modrix.
;
; Installs to %LOCALAPPDATA%\Programs\Modrix with RequestExecutionLevel user, so
; no admin rights are needed and the in-app updater can replace files without a
; UAC prompt. Run silently with `/S` (what the updater does): it force-closes any
; running instance, updates the files, and relaunches the app.
;
; Build:  makensis -DVERSION=<x.y.z> -DBINDIR=<dir-with-exes> installer\windows\modrix.nsi

Unicode true
!include "MUI2.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef BINDIR
  !define BINDIR "..\..\target\x86_64-pc-windows-msvc\release"
!endif

!define APPNAME "Modrix"
!define PUBLISHER "Modrix contributors"
!define GUIEXE "modrix-gui.exe"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}"

Name "${APPNAME} ${VERSION}"
OutFile "Modrix-Setup-${VERSION}.exe"
InstallDir "$LOCALAPPDATA\Programs\${APPNAME}"
InstallDirRegKey HKCU "Software\${APPNAME}" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma

!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${GUIEXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Launch Modrix"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

; Force-close any running Modrix so its files can be replaced (updates).
!macro CloseRunning
  nsExec::Exec 'taskkill /IM ${GUIEXE} /F'
  nsExec::Exec 'taskkill /IM modrix.exe /F'
!macroend

Section "Modrix" SecMain
  SectionIn RO
  !insertmacro CloseRunning
  SetOutPath "$INSTDIR"
  File "${BINDIR}\${GUIEXE}"
  File "${BINDIR}\modrix.exe"
  File /nonfatal "${BINDIR}\modrix-tui.exe"
  File /nonfatal "${BINDIR}\modrix-protocol.exe"

  CreateDirectory "$SMPROGRAMS\${APPNAME}"
  CreateShortcut "$SMPROGRAMS\${APPNAME}\${APPNAME}.lnk" "$INSTDIR\${GUIEXE}"

  WriteRegStr HKCU "Software\${APPNAME}" "InstallDir" "$INSTDIR"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayName" "${APPNAME}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTKEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayIcon" "$INSTDIR\${GUIEXE}"
  WriteRegStr HKCU "${UNINSTKEY}" "UninstallString" "$INSTDIR\uninstall.exe"
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoRepair" 1

  ; When the in-app updater runs us with /S, relaunch the app once updated.
  IfSilent 0 +2
    Exec '"$INSTDIR\${GUIEXE}"'
SectionEnd

Section "Uninstall"
  !insertmacro CloseRunning
  Delete "$INSTDIR\${GUIEXE}"
  Delete "$INSTDIR\modrix.exe"
  Delete "$INSTDIR\modrix-tui.exe"
  Delete "$INSTDIR\modrix-protocol.exe"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\${APPNAME}\${APPNAME}.lnk"
  RMDir "$SMPROGRAMS\${APPNAME}"
  DeleteRegKey HKCU "${UNINSTKEY}"
  DeleteRegKey HKCU "Software\${APPNAME}"
SectionEnd
