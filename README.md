# WallpaperMac

Wallpaper Engine 프로젝트를 macOS에서 재생하고 편집하기 위한 실험적 독립 구현이다. Rust로 작성했으며 AppKit·Metal·AVFoundation·WebKit을 사용한다. 현재 버전은 **0.2.1**이다.

실제 Mac에서 0.2.1의 Metal 출력을 재검증하는 단계다. 지원 범위와 차이는 [COMPATIBILITY.md](COMPATIBILITY.md), 검사 결과는 [verification/report.json](verification/report.json)에 기록한다. 원본 엔진과의 완전한 기능·픽셀 동등성은 확립하지 않았다.

## Mac에서 빌드하기

macOS 14.2 이상, Rustup, Python 3.9 이상과 Apple command line tools/SDK가 필요하다. `rust-toolchain.toml`이 Rust·Clippy·rustfmt를 1.91.1로 고정하고, dependency는 Cargo.lock으로 고정한다. 버퍼 ABI 테스트는 command line tools의 C++ 컴파일러도 사용한다.

```bash
git clone https://github.com/EunhakChoi/WallpaperMac.git
cd WallpaperMac
./scripts/verify-macos.sh native
open dist/WallpaperMac.app
```

검사 스크립트는 포맷·Clippy·테스트, `.app`/`.saver` 빌드와 서명, 실제 Metal offscreen 출력과 CPU reference 비교를 수행한다. 로그·JSON·PNG는 `verification/macos/`에 남는다. 세부 절차는 [MAC_VALIDATION.md](MAC_VALIDATION.md)를 따른다.

빌드만 하거나 universal 앱을 만들 수도 있다.

```bash
./scripts/build-macos.sh native

rustup target add aarch64-apple-darwin x86_64-apple-darwin
./scripts/build-macos.sh universal
```

생성물은 `dist/WallpaperMac.app`, `dist/WallpaperMac.saver`이다. 기본값은 ad-hoc 서명이며 signing identity는 `WPE_SIGN_IDENTITY`로 지정할 수 있다.

## 프로젝트 사용하기

앱에서 프로젝트 폴더를 선택하거나 경로를 직접 지정한다. 원본 프로젝트가 기본 assets를 참조한다면 보유한 설치본의 assets 경로도 제공한다.

```bash
open dist/WallpaperMac.app --args \
  --project "/path/to/project" \
  --assets "/path/to/wallpaper_engine/assets"
```

앱에는 직접 생성한 이미지·영상 test pattern·tone으로 만든 데모가 포함된다. 기본 설정과 편집용 복사본은 `~/Library/Application Support/WallpaperMac/`에 저장한다.

## 구성

| crate | 역할 |
|---|---|
| wpe-formats | PKG, TEX, MDL/MDMP, binary·ASCII FBX 읽기 |
| wpe-render | frame graph, GLSL→MSL, texture/depth/blend/HDR, text, CPU reference |
| wpe-sim | particle, bone/skin/morph/animation simulation |
| wpe-script | Boa 기반 SceneScript VM과 host API |
| wpe-core | 프로젝트·리소스·스크립트·simulation을 매 frame 결합 |
| wpe-editor | 문서·이미지·geometry 편집, undo/redo와 저장 |
| wpe-platform | 설정·library·playlist·rule·audio DSP·선택적 Steamworks |
| wpe-macos / wpe-screensaver | native 앱·Metal·재생·캡처·관리 UI·화면보호기 |
| wpe-cli | 검사, 데모 생성, CPU frame 출력과 graph 검증 |

## 개발 검사

Linux에서도 portable 경로와 CPU reference를 검사할 수 있다. C++ 컴파일러가 필요하다.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --no-fail-fast --locked

cargo run --locked --bin wpe -- demo out/demo
cargo run --locked --bin wpe -- frame out/demo/animated \
  --time 1.25 --output out/animated.png
```

0.2.1에서는 심볼릭 링크 임시 경로 비교와 Metal의 std140 배열 저장 간격을 수정했다. 생성된 MSL 버퍼 선언을 CPU C++로 컴파일해 실제 packed 값을 읽는 회귀 검사를 포함한다. CPU C++ ABI 검사는 Apple Metal 컴파일이나 GPU 실행을 대신하지 않는다.

원본 artwork·음악·engine binary·Steamworks SDK·시스템 font는 별도로 제공해야 한다. Cargo dependency의 라이선스는 `scripts/collect-licenses.py`가 앱 빌드 시 수집하며, 고정된 fallback 원문과 출처는 `packaging/dependency-licenses/`에 있다.
