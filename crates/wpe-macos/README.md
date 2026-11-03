# Native macOS application

The `wallpaper-macos` application implements the AppKit host for the portable scene, scripting, simulation and editor crates. Its native APIs typecheck for Apple Silicon and Intel. This development host is Linux/WSL: Apple SDK linking, real MSL compilation, GPU execution, TCC dialogs and desktop behavior have **not** been run here.

On a Mac, from the workspace root:

```sh
./scripts/build-macos.sh native
./scripts/verify-macos.sh
cargo run --locked -p wpe-macos --example metal_smoke -- /tmp/wallpaper-metal-check
cargo run --locked -p wpe-macos --bin wallpaper-macos -- --project /path/to/project --assets /path/to/assets
cargo run --locked -p wpe-macos --bin wallpaper-macos -- --project /path/to/project --desktop --screen 0
```

The packager currently sets macOS 14.2 as its deployment target. `universal` builds both architectures. The application, CLI and separate `WallpaperMac.saver` are packaged by the root scripts. SDK binaries, proprietary artwork and original executable code are not bundled. No-argument launch opens Settings; the menu bar item provides Settings, Open, Pause/Resume and Quit. `--screen` is a zero-based initial screen index; saved assignments use public display UUIDs.

## Native providers

| Area | Actual implementation |
|---|---|
| Scene | `ScenePlayer` fixed/runtime frames, SceneScript host commands, particles/models, font and video providers; indexed Metal meshes, exact named vertex/instance streams, per-stage reflected uniforms and MSL, per-pass viewport/scissor/depth state, color/float/3D/cube/depth textures and mips. |
| Effects | Shared render graph, real offscreen textures, comparison samplers, separable built-in Gaussian/Bloom and destination-read blend snapshots. Physical sampler slots are independent of logical texture slots such as 24/25. Shader compile errors propagate. MSL compilation uses precise/safe options and disables fast math. |
| HDR | Authored `general.hdr` requests RGBA16Float presentation with extended-linear sRGB and EDR on a screen reporting potential EDR headroom. Otherwise the display uses the explicitly reported BGRA8 SDR clamp profile. Floating-point intermediate textures preserve HDR values. Source colorimetry and physical display luminance are not validated. |
| Unicode | TextKit/CoreText attributed-text rasterization, system font cascade and color emoji, project-local fonts, outlines/shadow/background, `systemfont_` family resolution and style-keyed raster caching. Native RGBA is unpremultiplied before the core straight-alpha texture contract. |
| Video | Real AVQueuePlayer/AVPlayerLooper/AVPlayerLayer. Scene video textures use AVPlayerItemVideoOutput and BGRA readback. Playback, seek, rate, loop, mute, volume and metadata feed the scene provider. AVFoundation determines installed codecs. |
| Sound | AVAudioPlayer with native resource data; Symphonia compressed-audio → PCM16 WAV fallback (including Ogg Vorbis), actual loop/single playback, editor mute, volume, pause/stop/seek and state feedback. |
| Web | Project-local HTML in WKWebView; independently written property/general/audio/media callbacks, pause-preserving timer/rAF/CSS bridge and HTML/WebAudio output volume. The wallpaper WebView has no management command handler. Custom CEF shared textures/plugins are not substituted with fake handles. |
| External applications | Explicit user-configured native executable/.app or Wine/CrossOver command with literal arguments, owned process group and ScreenCaptureKit window frames. Pause and teardown control owned processes/capture. Mute/zero volume pauses the group; public per-process volume control and Windows desktop IPC/input injection are unavailable. |
| Audio response | ScreenCaptureKit system/microphone/application-filtered PCM → independent recovered DSP → stereo64/32/16 and Web128. Permission/error status is observable. App audio rules use actual filtered PCM activity, including their separate hold windows; up to16 filtered application streams. |
| Media | Opt-in running Music/Spotify AppleEvents polling or owned AV media. No private MediaRemote API. Metadata/state/timeline come from actual sources; Spotify artwork is bounded/decoded into PNG and palette. Music artwork and a universal all-app Now Playing source are not supplied by this profile. |
| Desktop | Public desktop window level, click-through and Spaces collection flags, per-display windows, display notifications, foreground/fullscreen/audio/battery/sleep rules, playlist/preset/profile persistence and explicit input gating above the desktop. Other-app clicks do not produce background shortcut clicks. |
| Pause | Paused scene slots skip VM/simulation/render submission. Explicit resize/editor camera changes permit one dirty `render_current` frame. AV, capture and owned processes synchronize pause; idle policy monitoring continues at4Hz. Paused dirty frames preserve the last audio bands. Stop releases resources and resume reloads. |
| Editor | Owned project copies; safe PKG unpacking, transaction/undo/autosave, native file panels/Trash/reveal, live document/resource-memory overlays, editable-model sidecar previews, particle/effect/script previews, temporary camera/input and property tests, image/model/sound/font import and local image tools provided by `wpe-editor`. |
| Export | Actual shared Metal offscreen render/readback, time-indexed AVAssetReader embedded video, temporal accumulation, AVAssetWriter H264 or PNG sequence; configured FFmpeg handles its available other codecs. Bounded backpressure, progress, cancellation and non-overwriting final publication. This export profile writes opaque RGB without an audio track. |
| Services | Public Workshop metadata/Steam URI; optional supplied official Steamworks flat API for user/public queries, filters, subscriptions, download/install progress, create/update/delete, DLC inventory/install/uninstall. User authentication, AppID/Workshop permission and SDK ABI are required. |
| Login / saver | Public SMAppService registration/status. Real ScreenSaver.framework plug-in reuses the native surface. See `../wpe-screensaver/README.md`. |

## Settings and editor entry points

The independently written management HTML has Library, Properties, Playback/displays, Audio/media, Playlists/rules, Editor, Steam Workshop, Application runtimes and Diagnostics pages. File panels configure library/shared-assets roots and external tools. Configuration, scene storage and working copies live under `~/Library/Application Support/WallpaperMac`; original installations and source project files are preserved.

The editor has file import, object selection/property editing, undo/redo/save/copy/Trash, live preview, export controls and advanced command JSON. The advanced command route reaches the same real editor service. Native host requests always produce completion events with a real result or error; the UI displays failed completions. Preview resource overlays include unsaved bytes and deletion tombstones. Packed working copies carry `.wallpaper-mac-unpacked`, so saved loose scene edits take precedence on reopen. The original PKG is retained.

Model import accepts OBJ, MDL and the bounded binary/ASCII FBX profile. The Model preview page opens a real editable sidecar; its FBX picker appends sampled animation retargeted by unique bone name while preserving the existing rest pose, geometry and source MDL. Unsupported FBX transforms or animation data return an error. Imports retain actual source bytes and participate in undo/save. Publication achievements use the four identifiers recovered from retained native source, require authenticated AppID431960 and verify actual SDK schema membership before SetAchievement/StoreStats. Store completion is reported only after its callback. Original AI/neural image equivalence is not claimed: local image tools expose their actual procedural profile. Puppet depth import/export currently uses an exact editable-vertex JSON roundtrip. Effect preview requires an actual preview scene/image, rather than inventing an effect's missing target. Windows wallpaper application compatibility depends on the configured external runtime.

## Verification and Mac acceptance

```sh
cargo test --locked -p wpe-platform -p wpe-macos -p wpe-screensaver
cargo clippy --locked -p wpe-macos -p wpe-platform -p wpe-screensaver --target aarch64-apple-darwin --all-targets --no-deps -- -D warnings
cargo clippy --locked -p wpe-macos -p wpe-platform -p wpe-screensaver --target x86_64-apple-darwin --all-targets --no-deps -- -D warnings
node crates/wpe-macos/tests/web_bridge.cjs
```

`validation.json` records exact completed local checks. `host-services.json` is a routing/capability ledger, not an original-product equivalence percentage. The Linux synthetic FFmpeg check proves portable encoder framing only. The actual Vorbis decode check proves decoded PCM only; no output device was opened. Node tests execute the actual bridge source under a modeled browser/native clock, not WebKit.

On the Mac, run the Metal smoke example first: it compiles real MSL, submits the real shared encoder, reads back and writes CPU/GPU/diff PNGs plus `report.json`. The optional retained-shader-root argument compiles available supplied shaders. Then verify real UTF-8/Korean/emoji raster output, retained scenes and changing effects, video codecs/seeks, sound/permission errors, application capture and cleanup, and export progress/cancel/output duration. Compare source and working-copy hashes after editing.

Test two screens with different scale factors; unplug/replug and reorder them; move through Spaces, fullscreen apps, Stage Manager, sleep/wake and locked screens; verify that foreground clicks do not trigger wallpaper shortcuts. Check pause CPU/GPU/energy usage and audio privacy state, persisted profiles/playlists/storage, service permission denials and Steam SDK-header ABI probe results. Screensaver preview/fullscreen has a separate lifecycle and TCC validation. These are outstanding physical acceptance tests, not local test passes.

Primary API references: [Metal HDR](https://developer.apple.com/documentation/metal/processing-hdr-images-with-metal), [ScreenCaptureKit](https://developer.apple.com/documentation/screencapturekit), [AVAssetReader](https://developer.apple.com/documentation/avfoundation/avassetreader), [SMAppService](https://developer.apple.com/documentation/servicemanagement/smappservice), [Steam UGC](https://partner.steamgames.com/doc/api/ISteamUGC), [Steam Apps](https://partner.steamgames.com/doc/api/ISteamApps). Reviewed 2026-10-28; native source references the actual pinned Rust framework bindings.
