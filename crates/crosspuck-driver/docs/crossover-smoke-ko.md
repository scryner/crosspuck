# CrossOver Steam Guest Driver 수동 검증 절차서

이 문서는 `crosspuck-driver`가 생성한 production `hid.dll`을 CrossOver Steam bottle에 배치하고, Steam에서 실제 인식/입력/feature/write 경로를 확인하기 위한 절차입니다.

## 0. 전제

- macOS host app이 로컬에서 Steam Controller Puck HID 장치를 정상 인식해야 합니다.
- CrossOver bottle에 Steam이 설치되어 있어야 합니다.
- 이 검증은 완전 자동화가 아닙니다. DLL 설치와 로그 점검은 스크립트로 보조하고, Steam UI 확인은 직접 수행합니다.

현재 로컬에서 확인된 기본 bottle 경로는 다음입니다.

```text
~/Library/Application Support/CrossOver/Bottles/Steam
```

Steam 실행 파일은 보통 다음 위치입니다.

```text
~/Library/Application Support/CrossOver/Bottles/Steam/drive_c/Program Files (x86)/Steam/steam.exe
```

설치 스크립트는 이 `steam.exe`가 있는 디렉터리에 `hid.dll`을 복사합니다.

## 1. DLL 빌드

CrossOver 검증용 local build는 GNU target을 사용합니다. PoC도 이 경로로 macOS에서 `hid.dll`을 빌드해 검증했습니다.

```sh
rustup target add x86_64-pc-windows-gnu
cargo build -p crosspuck-driver --release --target x86_64-pc-windows-gnu
```

산출물:

```text
target/x86_64-pc-windows-gnu/release/hid.dll
```

`x86_64-pc-windows-msvc` target도 check/build 자체는 지원하지만, 실제 release link에는 Windows MSVC `link.exe`가 필요합니다. macOS에서 바로 CrossOver smoke를 돌릴 때는 GNU target을 사용합니다.

## 2. CrossOver Steam bottle에 설치

Steam이 실행 중이면 완전히 종료합니다.

기본 Steam bottle을 사용하면:

```sh
tools/install-driver.sh --bottle Steam
```

DLL 경로를 직접 지정하려면:

```sh
tools/install-driver.sh \
  --bottle Steam \
  --driver target/x86_64-pc-windows-gnu/release/hid.dll \
  --no-build
```

설치 스크립트가 하는 일:

- `drive_c` 아래에서 `steam.exe`를 찾습니다.
- `steam.exe`와 같은 디렉터리에 `hid.dll`을 복사합니다.
- 기존 `hid.dll`이 있으면 `crosspuck-backups/` 아래에 백업합니다.
- Steam 디렉터리에 `crosspuck-driver.log`를 초기화합니다.
- bottle에 `crosspuck-wine-override.reg`를 생성하고 CrossOver `regedit`로
  import합니다.
- guest runtime용 `CROSSPUCK_*` registry/environment 값은 쓰지 않습니다.
  runtime 설정은 기본값을 쓰고, override가 필요한 값은 macOS host app이
  bridge connection으로 전달합니다.

주의:

```text
drive_c/windows/system32/hid.dll
```

에는 복사하지 않습니다. production driver는 non-virtual HID 호출을 real System32 `hid.dll`로 forwarding합니다.

## 3. Wine loader override

설치 스크립트는 loader-only Wine override 파일을 자동으로 생성하고
import합니다.

생성 후 bottle에 남는 registry 파일:

```text
~/Library/Application Support/CrossOver/Bottles/Steam/crosspuck-wine-override.reg
```

스크립트가 같은 bottle의 CrossOver `regedit`로 위 `.reg` 파일을 import합니다. 이 파일은 runtime 설정이 아니라 loader 설정만 담습니다.

설정되는 DLL override:

```text
HKCU\Software\Wine\DllOverrides
hid = native,builtin
```

`hid=native,builtin`은 Steam이 `hid.dll`을 로드할 때 Steam 디렉터리에 복사된 CrossPuck `hid.dll`을 먼저 사용하게 하고, 우리 DLL 내부에서 real HID 처리가 필요할 때 Wine builtin `hid`로 fallback할 수 있게 둡니다.

guest log severity는 registry/env가 아니라 macOS host app이 bridge
connection으로 전달하는 override로 제어합니다. 예:

```sh
open -a CrossPuck --args --override-log-level --log-level debug
```

등록 후 Steam이 이미 떠 있었다면 반드시 완전히 종료하고 다시 시작합니다. CrossOver가 override를 바로 반영하지 않으면 bottle 또는 CrossOver 앱을 재시작합니다.

## 4. Host app 실행

macOS에서 CrossPuck host app을 먼저 실행합니다.

확인할 것:

- macOS가 Input Monitoring 권한을 요청하면 CrossPuck에 허용합니다. 이 권한이 deny되면 host bridge는 listening 상태여도 Steam Controller HID 장치를 열 수 없어 guest handshake가 실패할 수 있습니다. 이전에 거부했다면 System Settings에서 허용한 뒤 CrossPuck을 재시작합니다.
- host app이 controller를 인식합니다.
- host bridge가 listening 상태입니다.
- native Steam이나 다른 프로세스가 controller를 독점하지 않습니다.

## 5. 로그 감시 시작

별도 터미널에서:

```sh
tail -f "$HOME/Library/Application Support/CrossOver/Bottles/Steam/drive_c/Program Files (x86)/Steam/crosspuck-driver.log"
```

Steam 설치 경로가 다르면 설치 스크립트 출력의 `Driver log file` 경로를 사용합니다.

## 6. CrossOver Steam 실행

CrossOver에서 Steam bottle의 Steam을 실행합니다.

초기 기대 로그:

```text
[crosspuck] crosspuck-driver attached ... host_bridge=true required=true ...
[crosspuck] startup bridge connect skipped: lazy connect enabled
```

`hook install ok`와 API discovery 세부 로그는 debug level 로그이므로 host app이 debug 또는 trace guest severity override를 내렸을 때만 나옵니다.

host bridge는 Steam이 HID discovery를 하거나 synthetic path를 여는 시점에 lazy로 연결됩니다.

```text
[crosspuck] lazy bridge connect ok reason=... identity=Live profiles=5 open_handles=0
```

host app을 늦게 켠 경우에는 다음 로그가 먼저 나올 수 있습니다.

```text
[crosspuck] lazy bridge connect failed reason=...: ...
```

이 경우 host app을 켠 뒤 Steam에서 controller 관련 화면을 다시 열어 lazy reconnect가 발생하는지 확인합니다.

## 7. Steam UI smoke

Steam에서 다음을 직접 확인합니다.

1. Controller settings 또는 controller test UI로 이동합니다.
2. Steam이 Steam Controller/Puck 호환 장치를 표시하는지 확인합니다.
3. 컨트롤러 버튼을 눌러 UI 반응이 있는지 확인합니다.
4. stick/pad/trigger 입력이 가능하면 각각 한 번씩 움직입니다.
5. rumble/ping/test action이 있으면 실행합니다.
6. host app을 종료하고 5초 정도 기다립니다.
7. Steam이 crash하지 않는지 확인합니다.
8. host app을 다시 실행합니다.
9. controller settings/test UI를 다시 열거나 입력을 다시 수행해 복구 여부를 확인합니다.

입력/feature/write가 실제로 호출되고 host-owned diagnostic 설정이 충분히 자세하면 다음 계열 로그가 나옵니다.

```text
SDL_hid_enumerate
SDL_hid_open_path
SDL_hid_read_timeout
SDL_hid_get_feature_report
SDL_hid_send_feature_report
SDL_hid_write
ReadFile
HidD_GetInputReport
HidD_GetFeature
HidD_SetFeature
HidD_SetOutputReport
WriteFile
DeviceIoControl
```

모든 로그가 반드시 한 번에 나와야 하는 것은 아닙니다. Steam UI에서 해당 API 경로를 실제로 밟아야 기록됩니다.

Steam/CrossOver가 SDL hidapi 경로를 먼저 쓰는 경우에는 `SDL_hid_*` 로그가 Win32 `ReadFile`/`HidD_*` 로그보다 더 중요합니다. `SDL3.dll load for hid hooks` 뒤에 `optional hook installed SDL3.dll!SDL_hid_enumerate` 계열 로그가 나오면 SDL hook 설치까지 완료된 상태입니다.

## 8. Smoke check 스크립트 실행

Steam UI 확인 후:

```sh
tools/smoke-check.sh --bottle Steam
```

결과 해석:

- `OK installed driver`: `steam.exe` 옆에 `hid.dll`이 있습니다.
- `OK driver log`: 로그 파일이 있습니다.
- `WARN legacy env registry`: 이전 smoke에서 남은 env registry 파일이 있으며 새 경로에서는 사용하지 않습니다.
- `WARN log marker missing`: 해당 API 경로를 아직 밟지 않았거나, Steam이 DLL을 로드하지 않았거나, host app이 실행 중이 아니었을 수 있습니다.

## 9. 통과 기준

최소 통과:

- Steam process가 crash하지 않습니다.
- 로그에 `crosspuck-driver attached`가 있습니다.
- debug 또는 trace log level에서는 로그에 `hook install ok`가 있습니다.
- host app 실행 상태에서 `lazy bridge connect ok` 또는 이후 host-backed HID 호출 trace가 있습니다.
- Steam UI에서 controller가 연결된 장치로 표시되거나 입력 반응이 있습니다.
- host app 종료/재실행 후 Steam이 crash하지 않고, 이후 controller 관련 동작이 복구됩니다.

권장 추가 통과:

- `HidP_GetCaps` 또는 SetupAPI discovery 관련 로그가 관찰됩니다.
- `ReadFile` 또는 `HidD_GetInputReport` trace가 관찰됩니다.
- `HidD_GetFeature`, `HidD_SetFeature`, `HidD_SetOutputReport`, `WriteFile` 중 하나 이상이 관찰됩니다.
- 5분 idle 동안 CPU hog나 로그 폭증이 없습니다.

## 10. 실패 시 수집할 자료

문제가 생기면 다음을 저장합니다.

```sh
tools/smoke-check.sh --bottle Steam
```

그리고 다음 파일:

```text
~/Library/Application Support/CrossOver/Bottles/Steam/drive_c/Program Files (x86)/Steam/crosspuck-driver.log
```

가능하면 함께 알려줄 것:

- Steam이 crash했는지, controller만 안 보이는지, input만 안 되는지
- host app 실행 여부
- `lazy bridge connect ok`가 있었는지
- `ReadFile`/`HidD_GetInputReport` trace가 있었는지
- `HidD_GetFeature`/`HidD_SetFeature`/`WriteFile` trace가 있었는지

## 11. Rollback

Steam을 완전히 종료한 뒤:

```sh
rm "$HOME/Library/Application Support/CrossOver/Bottles/Steam/drive_c/Program Files (x86)/Steam/hid.dll"
```

기존 local `hid.dll`이 있었다면 다음 위치에서 복구합니다.

```text
~/Library/Application Support/CrossOver/Bottles/Steam/drive_c/Program Files (x86)/Steam/crosspuck-backups/
```

이전 smoke에서 남은 `crosspuck-driver-env.reg` 또는 `HKCU\Environment` 아래 `CROSSPUCK_*` 값은 더 이상 runtime 설정 경로가 아닙니다. 깨끗한 bottle을 원하면 `regedit`에서 삭제합니다.
