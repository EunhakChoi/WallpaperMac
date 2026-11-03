# 구현 범위와 확인된 경계

WallpaperMac 0.2.1은 설치본에서 회수한 형식·호출·상태 전이 계약을 바탕으로 작성한 독립 구현이다. 정적 분석, 새 구현의 테스트, 보존된 원본 compiler 출력 비교, 실제 원본 runtime 비교를 구분한다. 현재 원본 엔진과의 완전한 동작·픽셀 동등성은 확립하지 않았다.

| 영역 | 연결된 실제 구현 | 확인이 남은 부분 또는 차이 |
|---|---|---|
| 리소스 | PKG 우선순위·대소문자·중복/빈 항목, JSONC, 메모리 편집 overlay와 삭제, packed/loose 프로젝트 | Unix 경로 처리와 Windows locale/Unicode 파일시스템의 완전한 일치 |
| 텍스처 | TEX v4/v5, RGBA/R/RG, BC1/2/3/7, half float, mip/volume/sprite/video 경로 | ETC 등 명시적으로 미지원인 codec; 원본 GPU의 반올림·색공간 |
| SceneScript | Boa의 실제 ES module 실행, hook/입력/timer/storage/속성·소재·동적 layer와 host 명령 | 원본 V8과 모든 JavaScript 모서리 동작의 일치; 원본 watchdog/heap 정책 |
| scene | hierarchy/camera, 숨은 의존 layer, 효과 target와 명령, audio uniforms, shader ABI packing, 입자·모델·텍스트 연결 | 원본의 모든 dependency 순서와 수치 정책의 동일성 |
| shader/render | GLSL 전처리·조합·reflection·Naga→MSL, indexed geometry, depth/blend/HDR/mip/cube, 논리 texture24/25와 실제 sampler 분리 | raw Windows HLSL 6개와 geometry stage 4개는 직접 번역하지 않는다. 입자는 CPU에서 geometry를 만든다. 원본의 잘못된 unmatched `#endif` source 1개는 오류로 보존한다. |
| 빛·반사 | 실제 shadow/cookie atlas와 PCF, mirrored geometry를 그리는 planar reflection | shadow 해상도/필터·기본 반사면·교차 clipping은 명시된 독립 수치 정책이며 원본 동등성이 아니다. |
| 입자 | 결정적 RNG/noise, emitter/initializer/operator, renderer별 geometry, 자식·death event·collision | 모든 원본 SSE 반올림, 충돌 세부 정책과 실제 GPU 출력 |
| 모델 | MDL skeleton/clip, skin·morph·bone alpha/order·root motion, scalar blend-map·보조 텍스처 합성, editable geometry/animation preview | 이름이 회수되지 않은 native constraint table·일부 보조 track 의미; 독립 IK/spring solver와 원본 solver의 수치 차이 |
| 텍스트 | portable shaping/RTL/줄바꿈 및 native CoreText/TextKit, 시스템 font cascade·색 emoji | font 파일·OS별 glyph 차이, 원본 text layout과 실제 Mac 출력 |
| 편집 | 복사본의 실제 JSON/byte 변경, undo/redo·저장, paint·geometry/weight/topology·모델 sidecar, image 도구 | 원본 MDL binary writer는 제공하지 않는다. 새 UI와 동일 editor 명령 service를 제공한다. |
| image 도구 | gradient normal map, luminance depth, 색 분포/min-cut 전경 분리, mask·atlas의 실제 픽셀 출력 | semantic AI depth/분리 모델은 회수되지 않았다. 독립 알고리즘 결과를 원본 neural 모델의 결과로 표시하지 않는다. |
| video/audio/web | AVFoundation 실제 재생/영상 texture, AVAudioPlayer와 compressed-audio decode fallback, WKWebView bridge | OS codec·출력 장치·WebKit과 CEF의 차이, CEF 전용 shared texture/plugin |
| export | 같은 Metal encoder의 offscreen readback, 시간 지정 embedded video, PNG sequence/H.264·설정한 FFmpeg | 현재 export는 불투명 RGB이며 audio track을 포함하지 않는다. 외부 codec의 실제 가용성 |
| macOS | AppKit desktop windows, 다중 화면·규칙·pause·입력, ScreenCaptureKit, login item, 실제 화면보호기 plugin | Apple SDK link, 실제 Metal compile/readback, Spaces/Stage Manager·권한·sleep·장치는 실제 Mac 검증 대상 |
| media | 선택한 running Music/Spotify 또는 자체 AV media의 실제 metadata | 비공개 범용 Now Playing API 및 Music artwork는 제공하지 않는다. |
| application | 명시적으로 설정한 Mac 실행파일/.app/Wine/CrossOver, 소유 process group과 창 capture | Windows wallpaper IPC/input injection; 다른 process의 음량 API. mute는 소유 group을 pause한다. |
| Steam | 선택한 공식 SDK flat API의 query/subscription/download/publish/DLC, schema와 callback을 확인하는 업적 처리 | SDK header ABI probe·실제 계정/AppID/remote permission이 필요하다. Linux mock은 live Steam 성공의 증거가 아니다. |

CPU reference는 지원하는 builtin 프로그램의 실제 픽셀을 그린다. 임의의 번역 shader를 CPU에서 대체 실행하지 않으며 `GpuOnly`는 명시적 오류다. CLI `simulate`는 실제 VM/simulation/graph와 uniform packing을 검사하고, 임의 shader의 실행은 Mac Metal 경로가 담당한다. `render`/`inspect.scene`는 보존된 초기 정적 preview다.

검증 요약은 [verification/report.json](verification/report.json)에 있다. CPU 검사와 Darwin 타입 검사는 실제 Mac GPU 실행과 구분한다. 실제 Mac 절차는 [MAC_VALIDATION.md](MAC_VALIDATION.md)에 있다.

전체 metadata226개 중 Scene216개는 새 구현의3-frame 그래프 검사를 마쳤다. 그래프가 정상 구성된 것과 모든 layer가 원본처럼 보이는 것은 다르다. 진단9개 프로젝트에는 native system-font가 필요한3개, 교체 이미지 입력을 요구하는 미완성 template2개, 원본의 잘못된 shader가 포함된1개, 선언된 독립 shadow/reflection profile5개가 있으며 일부가 겹친다. 원본 파일이나 shader 문법을 바꾸어 성공으로 집계하지 않았다. Windows `sheep.exe`가 있는 type 누락 프로젝트를 포함해 web/video/application10개는 이 Scene 검사와 구분한다.
