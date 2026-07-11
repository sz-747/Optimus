!include "WinMessages.nsh"

!macro OPTIMUS_PATH_HELPER ACTION PREVIOUS RESULT
  ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\optimus-user-path.ps1" -Action "${ACTION}" -Path "$INSTDIR" -LegacyPath "$LOCALAPPDATA\Programs\Optimus\bin" -PreviousPath "${PREVIOUS}"' ${RESULT}
!macroend

!macro OPTIMUS_BROADCAST_ENV RESULT
  System::Call 'user32::SendMessageTimeoutW(i 0xffff, i ${WM_SETTINGCHANGE}, i 0, w "Environment", i 0x2, i 5000, *i .${RESULT})'
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Keep a previous Tauri opt-in when the install directory changes.
  ReadRegStr $0 HKCU "${MANUPRODUCTKEY}" "CliPathAdded"
  StrCmp $0 "" optimus_path_check_legacy
  !insertmacro OPTIMUS_PATH_HELPER "Add" "$0" $1
  StrCmp $1 0 optimus_path_record optimus_path_failed

  ; Preserve an explicit choice made by the legacy Inno installer. The helper reports 10 only
  ; when that old PATH segment existed and was migrated.
  optimus_path_check_legacy:
    !insertmacro OPTIMUS_PATH_HELPER "MigrateLegacy" "" $1
    StrCmp $1 10 optimus_path_record
    StrCmp $1 0 optimus_path_prompt optimus_path_failed

  optimus_path_prompt:
    IfSilent optimus_path_done
    MessageBox MB_YESNO|MB_ICONQUESTION "Add the Optimus CLI to your user PATH?" /SD IDNO IDYES optimus_path_add IDNO optimus_path_done

  optimus_path_add:
    !insertmacro OPTIMUS_PATH_HELPER "Add" "" $1
    StrCmp $1 0 optimus_path_record optimus_path_failed

  optimus_path_record:
    WriteRegStr HKCU "${MANUPRODUCTKEY}" "CliPathAdded" "$INSTDIR"
    !insertmacro OPTIMUS_BROADCAST_ENV r2
    Goto optimus_path_done

  optimus_path_failed:
    DetailPrint "Optimus CLI PATH update failed with exit code $1; PATH was left unchanged."

  optimus_path_done:
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ReadRegStr $0 HKCU "${MANUPRODUCTKEY}" "CliPathAdded"
  StrCmp $0 "" optimus_unpath_done

  !insertmacro OPTIMUS_PATH_HELPER "Remove" "$0" $1
  StrCmp $1 0 optimus_unpath_removed optimus_unpath_failed

  optimus_unpath_removed:
    DeleteRegValue HKCU "${MANUPRODUCTKEY}" "CliPathAdded"
    !insertmacro OPTIMUS_BROADCAST_ENV r2
    Goto optimus_unpath_done

  optimus_unpath_failed:
    DetailPrint "Optimus CLI PATH cleanup failed with exit code $1; the opt-in marker was retained."

  optimus_unpath_done:
!macroend
