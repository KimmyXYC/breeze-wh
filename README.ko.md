<div align="center">

# Breeze

**Windows Hello 얼굴 인식 후 "확인" 클릭, 이제 필요 없습니다.**

[![CI](https://github.com/evan-choi/breeze-wh/actions/workflows/ci.yml/badge.svg)](https://github.com/evan-choi/breeze-wh/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/breeze-wh)](https://crates.io/crates/breeze-wh)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](LICENSE-MIT)

[English](README.md) | [한국어](#어떻게-동작하나요)

</div>

---

Windows Hello 얼굴 인식은 잘 됩니다. 근데 인식이 끝나도 매번 "확인"을 눌러야 해요. 쓸데없는 한 클릭입니다.

**Breeze**는 그 클릭을 없애줍니다. Windows Hello 인증 창이 뜨면 얼굴 인식 성공 여부를 확인하고, 알아서 확인 버튼을 눌러줍니다. Windows 서비스로 백그라운드에서 조용히 돌아갑니다.

## 어떻게 동작하나요

```
Windows Hello가 얼굴을 인식함
        ↓
인증 다이얼로그에 "확인" 버튼이 나타남
        ↓
Breeze가 UI Automation API로 감지
        ↓
자동으로 확인 (PIN 자동 입력은 아래에서 별도로 활성화)
        ↓
클릭 없이 바로 통과
```

## 설치

```powershell
cargo install breeze-wh
breeze-wh install
```

끝입니다.

## PIN 자동 입력

Windows Hello를 사용할 Windows 사용자 계정으로 다음 명령을 실행하세요:

```powershell
breeze-wh pin set
```

기존 Windows Hello PIN을 두 번 입력하세요. 입력 내용은 화면에 표시되지 않습니다. PIN은 현재 사용자의 Windows 자격 증명 관리자에 저장되며, 지원되는 PIN 창에서 자동 입력과 확인이 활성화됩니다. Windows Hello PIN 자체는 변경하지 않습니다. 관리자 권한이나 서비스 재시작은 필요 없으며, 다음 인증 창부터 적용됩니다.

```powershell
breeze-wh pin status  # PIN 저장 여부 확인 (PIN 값은 표시하지 않음)
breeze-wh pin clear   # 저장된 PIN 삭제 및 자동 입력 비활성화
```

PIN을 저장하기 전에는 자동 입력이 비활성화되어 있습니다. PIN은 현재 컴퓨터의 현재 사용자에게만 저장되며, `config.toml`이나 로그에는 기록되지 않습니다. Windows Hello PIN을 변경했다면 `pin set`을 다시 실행하세요. 서비스 제거 시 저장된 자격 증명은 남으므로, 필요하면 제거 전에 `pin clear`를 실행하세요.

활성화하면 지원되는 인증 창에서 직접 PIN을 입력하지 않아도 인증할 수 있습니다. 각 창에서 PIN 입력과 확인은 한 번만 시도하며, 실패하면 수동으로 처리할 수 있도록 둡니다. 저장된 PIN을 수정한 후 다시 시도하려면 인증 창을 닫고 다시 여세요.

시스템 `CredentialUIBroker.exe`의 `PasswordField_4` 컨트롤과 UI Automation Value 패턴을 지원합니다. 프로그램 입력을 허용하지 않는 컨트롤은 수동 입력이 필요합니다. 인증 방식 전환, Windows 로그인 화면 잠금 해제, UAC / 보안 데스크톱 창은 지원하지 않습니다.

## 업그레이드

```powershell
breeze-wh upgrade
```

GitHub Releases에서 최신 `breeze-wh.exe`를 받아 현재 바이너리를 교체합니다. 기존 exe는 `breeze-wh.exe.old`로 남았다가 다음 재부팅 때 정리됩니다 (실행 중인 exe는 자기 자신을 지울 수 없는 Windows 제약 때문).

서비스 상태는 그대로 유지됩니다:

- **실행 중** → 중지 → 교체 → 재시작
- **중지됨** → 교체만
- **미설치** → 교체만

> **참고:** `upgrade`는 crates.io가 아니라 GitHub Release에서 바이너리를 받아옵니다. 업그레이드 후에도 `cargo install --list`는 이전 버전으로 보일 수 있으니, 실제 설치된 버전은 `breeze-wh --version`으로 확인하세요.

## 명령어

- `breeze-wh install` — 서비스 설치 및 시작
- `breeze-wh uninstall` — 서비스 중지 및 삭제
- `breeze-wh start` — 서비스 시작
- `breeze-wh stop` — 서비스 중지
- `breeze-wh status` — 현재 서비스 상태 출력
- `breeze-wh upgrade` — 최신 릴리스로 업그레이드 (위 섹션 참고)
- `breeze-wh pin set|clear|status` — 현재 사용자의 PIN 자동 입력 관리
- `breeze-wh --version` — 설치된 버전 확인

`install` / `uninstall` / `start` / `stop` / `upgrade`는 관리자 권한이 필요하지만, UAC로 자동 상승되므로 관리자 셸을 따로 띄울 필요는 없습니다.

## 구조

Breeze는 하나의 바이너리가 두 가지 모드로 동작합니다:

- **Service 모드** — Session 0에서 Windows 서비스로 실행됩니다. 사용자 로그온/로그오프를 감지하고, 유저 세션에 helper 프로세스를 띄웁니다. helper가 죽으면 자동으로 다시 띄웁니다.

- **Helper 모드** — 유저 세션에서 관리자 권한으로 실행됩니다. UI Automation 이벤트를 통해 `Credential Dialog Xaml Host` 창을 감지하고, UI 트리를 한 번에 스캔합니다:
  - `PasswordField_4`이 있으면 → PIN 모드 → 저장된 PIN이 있으면 **한 번 입력 및 확인**, 없으면 무시
  - `PasswordField` 없이 `OkButton`이 있으면 → 얼굴 인식 → **클릭**
  - 컨트롤이 준비되지 않았으면 → 구조 및 활성화/표시 상태 변경 이벤트로 기다림

감지에 사용하는 `AutomationId`와 `ClassName`은 언어에 의존하지 않으므로, Windows 표시 언어가 뭐든 상관없이 동작합니다.

## 설정

설정 파일: `C:\ProgramData\Breeze-WH\config.toml` (설치 시 자동 생성)

```toml
enabled = true
debounce_ms = 2000
log_level = "info"
log_max_files = 7
```

로그: `C:\ProgramData\Breeze-WH\logs\`

## 요구 사항

- Windows 10 / 11
- Windows Hello 얼굴 인식 또는 PIN이 설정되어 있어야 함
- [Rust](https://rustup.rs/) 툴체인 (cargo로 설치 시)

## 라이선스

MIT OR Apache-2.0
