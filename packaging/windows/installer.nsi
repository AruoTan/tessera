Unicode true
!include "MUI2.nsh"

!ifndef APP_VERSION
  !error "Pass APP_VERSION, APP_EXE, APP_LICENSE, APP_ICON and APP_OUTPUT to makensis."
!endif
Name "Tessera"
OutFile "${APP_OUTPUT}"
InstallDir "$LOCALAPPDATA\Tessera"
InstallDirRegKey HKCU "Software\Tessera" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
!define MUI_ICON "${APP_ICON}"
!define MUI_UNICON "${APP_ICON}"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\tessera.exe"
!insertmacro MUI_PAGE_LICENSE "${APP_LICENSE}"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function .onInit
  SetShellVarContext current
  FindWindow $0 "TesseraRustApplication"
  StrCmp $0 0 ready
  MessageBox MB_OK|MB_ICONINFORMATION "Right-click the Tessera taskbar widget and choose Exit before installing."
  Abort
  ready:
FunctionEnd

Section "Tessera" Main
  SetOutPath "$INSTDIR"
  File /oname=tessera.exe "${APP_EXE}"
  File /oname=LICENSE.txt "${APP_LICENSE}"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateDirectory "$SMPROGRAMS\Tessera"
  CreateShortcut "$SMPROGRAMS\Tessera\Tessera.lnk" "$INSTDIR\tessera.exe"
  CreateShortcut "$SMPROGRAMS\Tessera\Uninstall.lnk" "$INSTDIR\uninstall.exe"
  WriteRegStr HKCU "Software\Tessera" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tessera" "DisplayName" "Tessera"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tessera" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tessera" "DisplayIcon" "$INSTDIR\tessera.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tessera" "UninstallString" '$\"$INSTDIR\uninstall.exe$\"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tessera" "QuietUninstallString" '$\"$INSTDIR\uninstall.exe$\" /S'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tessera" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tessera" "NoRepair" 1
SectionEnd

Function un.onInit
  SetShellVarContext current
  FindWindow $0 "TesseraRustApplication"
  StrCmp $0 0 ready
  MessageBox MB_OK|MB_ICONINFORMATION "Right-click the Tessera taskbar widget and choose Exit before uninstalling."
  Abort
  ready:
FunctionEnd

Section "Uninstall"
  Delete "$INSTDIR\tessera.exe"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\Tessera\Tessera.lnk"
  Delete "$SMPROGRAMS\Tessera\Uninstall.lnk"
  RMDir "$SMPROGRAMS\Tessera"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Tessera"
  DeleteRegKey HKCU "Software\Tessera"
  # Preserve the user's settings in the roaming profile.
SectionEnd
