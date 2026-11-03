# 실제 Mac 검증

macOS 14.2 이상인 Mac에서 소스 묶음을 풀고 프로젝트 디렉터리에서 실행한다. Rustup, Python 3.9 이상과 Apple command line tools/SDK가 필요하다. Rustup은 `rust-toolchain.toml`에 고정한 Rust 1.91.1·Clippy·rustfmt를 사용한다. `RUSTUP_TOOLCHAIN`을 별도로 지정했다면 고정된 버전을 덮어쓰지 않는지 확인한다. 첫 빌드에서 Cargo가 고정된 dependency를 받는다. C++ 버퍼 ABI 회귀 검사도 command line tools의 컴파일러를 사용한다.

0.2.0을 시험했던 폴더와 로그는 보존하고, 0.2.1을 별도 폴더에 풀어 아래 검사를 실행한다.

```bash
./scripts/verify-macos.sh native
open dist/WallpaperMac.app
```

자동 검사는 포맷·strict Clippy·Rust tests·bundle 빌드/서명·실제 Metal offscreen submit/readback을 실행한다. `verification/macos/<시각>/report.json`, stdout/stderr와 Metal PNG를 보존한다. Node가 있으면 웹 bridge도 검사한다. 바탕화면·로그인 항목·화면보호기 설치·캡처 권한·Steam 계정은 변경하지 않는다.

0.2.1의 Clippy는 `--keep-going`, 테스트는 `--no-fail-fast`로 실행해 일부 실패가 뒤쪽 crate의 결과를 가리지 않게 한다. Metal 허용 오차는 기존과 같은 채널당 3이다. `metal/report.json`은 전체 오차와 함께 HDR·depth·blend·뼈 투명도·음향 배열 영역별 오차를 남긴다.

앱에서 `dist/WallpaperMac.app/Contents/Resources/Demo`를 열면 기본 데모가 나온다. 하위 프로젝트는 `animated`, `effects`, `text`, `sound`, `video`, `web`이다. 별도 데모도 만들 수 있다.

```bash
dist/WallpaperMac.app/Contents/MacOS/wpe demo out/mac-demo
open dist/WallpaperMac.app --args --project "$PWD/out/mac-demo/animated"
```

| 시험 | 실제 확인할 결과 |
|---|---|
| 동적 scene | 카드 이동·triangle 회전·입자가 함께 보인다. speed·particles 속성이 반영되고 카드 클릭으로 색이 바뀐다. |
| GPU 효과 | effects 데모의 생성 이미지가 시간에 따라 변형된다. Diagnostics에 shader/texture 오류가 없다. |
| 텍스트 | text 데모의 줄바꿈·아랍어 결합/방향·윤곽, 실제 프로젝트의 시스템 font·emoji를 확인한다. |
| 영상·소리 | video의 2초 pattern과 tone이 반복된다. sound는 클릭 후 재생한다. Pause·volume이 실제 출력에 적용된다. |
| 웹 | web 데모의 입력·속성·audio listener 등록과 해제가 동작한다. |
| 일시정지 | 시간·움직임·소리가 멈춘다. paused editor zoom/속성 변경으로 한 frame을 그려도 VM timer와 입자 수명이 진행되지 않는다. |
| desktop·화면 | Finder icons, Spaces, 전체화면, Stage Manager, 다중 화면의 배율/해상도, 화면 분리·재연결을 확인한다. |
| capture·규칙 | Audio response에서 Screen Recording 권한을 승인하고 반응을 확인한다. 앱 규칙·sleep/battery도 확인한다. microphone은 지원되는 OS에서 별도 선택한다. |
| 모델·FBX | Import model에서 `crates/wpe-formats/tests/fixtures/owned-skinned-animation-ascii.fbx`를 가져와 Model preview로 형상을 확인한다. move clip 재생/정지, 같은 bone 이름의 FBX animation 추가, undo/redo와 저장 결과를 확인한다. |
| 편집기 | 복사본을 열어 속성·paint·geometry를 수정한다. 저장 전 preview, undo/redo, 재실행 후 저장 결과를 확인한다. |
| 내보내기 | PNG sequence/H.264 파일의 frame·길이와 취소 완료를 확인한다. 외부 FFmpeg 형식은 명시적으로 설정한 executable을 사용한다. |
| 화면보호기 | dist/WallpaperMac.saver를 열어 설치한 뒤 Options에서 project/assets를 고른다. 작은 preview·전체화면·시작/종료·설정 sheet를 확인한다. |
| media | owned/Music/Spotify를 선택하고 필요한 Automation 권한 승인 후 제목·재생상태·artwork가 실제 재생과 일치하는지 확인한다. |
| Steam | 별도 공식 SDK의 header probe 후 연결한다. subscription/query/download와 설치 파일을 확인한다. 게시 시험은 사용자가 선택한 프로젝트/계정에서 수행한다. |
| application | 명시한 Mac 앱/실행파일 또는 설치한 Wine/CrossOver로 창이 표시되고 중지 후 소유 프로세스/캡처가 정리되는지 확인한다. |

Windows desktop IPC와 다른 프로세스의 오디오 음량, 비공개 범용 Now Playing API는 에뮬레이션하지 않는다. application mute는 소유 프로세스 그룹을 pause한다. Steam·권한·장치가 필요한 시험은 해당 provider를 실제 사용할 때 수행한다.

문제가 생기면 자동 검사의 report.json, 실패 명령 로그와 앱 Diagnostics의 resource/shader 오류를 보존한다. Mac 모델·OS·화면 수/배율·프로젝트를 함께 기록하면 재현에 도움이 된다. 원본 동등성 판정에는 같은 프로젝트·입력·시간에서 원본 출력을 비교하는 과정이 추가로 필요하다.

## 0.2.1 수정과 재검증


0.2.1은 초기 Mac 검증에서 발견된 Clippy·경로 테스트·Metal 배열 배치 오류를 수정했다. 검증 요약은 [verification/report.json](verification/report.json)에 기록한다.

| 항목 | 0.2.1 수정 |
|---|---|
| Rust 버전별 lint 차이 | 검증용 툴체인을 1.91.1로 고정하고 CPU sampler의 `else if`를 정리했다. |
| `/var`와 `/private/var` 경로 비교 | fixture 경로를 정규화하고 심볼릭 링크 경유 프로젝트 로드 회귀 검사를 추가했다. |
| Metal 배열 저장 간격 | Naga가 std140의 16-byte 간격을 C++ 배열의 4/8-byte 간격으로 출력하던 부분에 원소 padding과 subscript 접근을 보존했다. host 값·reflection·배열 인자 변환은 유지한다. |
| 검증 보강 | 생성된 실제 MSL의 버퍼 선언을 C++로 컴파일해 음향·뼈 alpha·Bloom·중첩 배열의 packed bytes를 읽어 검사한다. 이 검사는 Apple Metal 컴파일이나 GPU 실행을 대신하지 않는다. |

수정본의 실제 Metal 성공 여부는 이 문서의 명령을 Mac에서 다시 실행해 판정한다. Linux에서 Mac 통과 결과를 만들어 기록하지 않는다.
