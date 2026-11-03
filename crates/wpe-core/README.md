# wpe-core

Portable project/resource loading and the actual per-frame scene orchestrator. `ScenePlayer` joins the independent formats, render, simulation and SceneScript services; neither CLI nor native hosts need a second scene implementation. The original Windows engine is never loaded.

`load_project(path, assets)` accepts a folder/project.json, scene JSON/PKG, or a directly selected image convenience profile. Metadata-selected `scene.json` automatically mounts an adjacent `scene.pkg`, including packages without a loose JSON. An explicitly unpacked editor copy carries `.wallpaper-mac-unpacked` so its saved loose resources remain authoritative. Web/video/application metadata describes the native provider; loading does not launch it. An absent project type is inferred from its entry, while an explicit unknown type remains unknown.

The VFS implements positive-size PKG entries, direct paths and front-priority directory mounts, including case-folded lookup, last duplicate and empty-entry behavior. `mount_memory_front` provides authoritative unsaved byte overrides and deletion tombstones. `ResourceOrigin` reports the successful source. Resource enumeration uses package indexes and does not decode every asset.

`ScenePlayer::new` initializes lazy assets, materials, script modules, particles and models. `advance(delta, input)` runs actual VM hooks/host commands, clocks, pose/particle simulation and render graph construction. Frames contain indexed geometry, textures, explicitly ordered targets/passes, actual compiled shader stages, uniforms and compatibility diagnostics. The host supplies stereo audio, media, pointer/button and screen state; native sound/video requests use explicit commands and feedback. Bindings and zero-binding animation clocks share one runtime.

Implemented scene paths include parent transforms, perspective/orthographic cameras and editor camera overrides, fullscreen and dependency images, materials/effects and command-driven ping-pong targets, light/shadow/cookie uniforms, planar reflection producers, sprite/volume/embedded video resources, renderer-specific particles, typed model pose/skin/morph and edited model data, procedural geometry, portable/native text integration and actual triangle hit testing. Scene resources follow descriptor → material → `materials/<full dotted logical name>.tex`; a logical name ending in `.jpg` still probes its native `.jpg.tex` first.

`render_current(input)` builds an explicitly requested dirty frame while paused. It does not advance VM hooks/timers, animation, particles or models. Camera edits and render-only pointer input do not consume the last VM input edge. Stateful effect targets retain their previous simulation state. Runtime audio reducers average left/right before their pairwise-max reduction. Hidden image dependencies render offscreen without becoming clickable visible layers.

Known numerical policies and explicit capabilities are documented in [COMPATIBILITY.md](../../COMPATIBILITY.md). Camera paths currently interpolate retained transforms sequentially, shadow/reflection have declared independent profiles, and unknown model constraint tables remain diagnostics rather than invented values. Frame validation and shader uniform packing establish structural consistency, not original pixels or a physical Metal driver result.

The older `load_scene`/`RenderPlan` and `render_software` remain a separately described static-image preview adapter for existing callers. Its deliberate unsupported warnings do not describe `ScenePlayer` support. Its shared legacy `Vertex` ABI stays 16 bytes; the dynamic renderer uses the separate 64-byte `GpuVertex` contract.

```sh
cargo test --locked -p wpe-core
cargo clippy --locked -p wpe-core --all-targets -- -D warnings
python3 crates/wpe-core/tools/validate.py
```

Owned tests cover project/resource priority, packed scenes, actual dynamic pixels and audio shader data, preview pause clocks, hierarchy/hit testing, edited model pixels, reflection and effect passes. The optional retained shader test requires the externally supplied shader root. Tests do not bundle original assets or execute original engine code. Native link/run/readback remains physical Mac validation.
