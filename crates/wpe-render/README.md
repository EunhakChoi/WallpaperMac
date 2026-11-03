# wpe-render

Portable render-frame contracts, actual GLSL-dialect → Naga IR → Metal Shading Language compilation, a CPU reference rasterizer, and shaped text. This crate depends on `wpe-formats`; it does not depend on the scene player or a native window system.

`frame.rs` is the shared contract. `GpuVertex` is exactly 64 bytes, aligned to 16 bytes: position, normal, UV and color are four `float4` values. Builtin shaders consume clip coordinates with Metal depth `[0,w]`; retained custom vertex shaders consume their original local attributes and require the caller's actual model/view/projection matrices. Extra vertex/instance attributes retain their exact input names and numerical signed/unsigned/float types. Instance streams include the prefix selected by `first_instance`.

Frames contain explicit textures, indexed triangle draws, and ordered render passes. A missing pass list draws all geometry to screen. Explicit passes are authoritative: they select screen or an offscreen target, clear, viewport/scissor, draws or an effect. Sampling an attachment while writing it is rejected; callers allocate a snapshot or ping-pong target. Depth persists until an explicit clear. Frame validation rejects malformed indices, inconsistent streams/mips, missing resources, overflow and nonfinite geometry.

| Implemented path | Behavior |
| --- | --- |
| Rasterizer | Homogeneous six-plane clipping, top-left triangle coverage, perspective interpolation, depth/cull/color masks, viewports, scissors and instances |
| Sampling | Byte/float 2D, volume trilinear, cube faces, depth comparison before linear filtering, mip LOD, per-draw sampler overrides, clamp/repeat/mirror/border |
| Targets | RGBA/BGRA8, RGBA16/32 float, RG16 float, R16/32 float, R8 and Depth32; float HDR preservation and half rounding; declared color target mip generation |
| Blend | Fixed-function profiles and all retained shader blend branches 0–32, including opacity exceptions and HSL modes |
| Effects | Ordered copy, color matrix, Gaussian blur, bloom, blend, displacement and 3D LUT, both CPU equations and actual generated MSL |
| Shaders | Conditional/function-macro preprocessing, scoped includes, active combo defaults, uniform annotation aliases/defaults, std140 packing, separate textures/samplers, actual Naga validation/MSL emission |
| Text | HarfBuzz shaping, UAX9 visual runs, UAX14 wrapping, ligatures/RTL clusters, letter spacing, row limit/reshaped ellipsis, fontdue coverage, outline/shadow and atlas origin metadata |

## Shader entry points

Build a `shader::SourceLibrary`, insert source names in their actual resource namespace, then call `ShaderCompiler::compile_pair(vertex, fragment, &defines)`. The two stages share effective combo defaults and varying locations. `reflect_metadata(name, stage, &defines)` resolves includes/preprocessing and returns active defaults, aliases and resource hints without creating Naga IR or MSL. `UniformValue::to_json()` produces ordinary numeric SceneScript values. `CompiledShader::pack_uniforms(&values)` packs each stage's own reflected layout, accepting the exact uniform name, annotation `material` alias or `label` alias in that order, followed by its default.

The compatibility prelude preserves the native Windows/top-down texture branch (`HLSL=1`) while retaining GLSL syntax utilities. `WPE_GLSL_COORDINATES=1` opts out. Matrix inverse helpers, explicit vector truncation/broadcast, mutable fragment varying copies and viewport-index lowering are implemented. Unknown expressions still have to pass Naga; a parse/validation failure is an error, not a substituted shader.

Runtime constants are the caller's responsibility: `TEXnFORMAT` must describe the uploaded texture semantics, `INSTANCECOUNT` the actual instance count, and `BLENDROWCOUNT` the uploaded puppet blend-map rows. The corpus runner's explicit representative counts are not native defaults. Short editor annotation vector defaults retain their raw metadata and zero-fill missing storage channels in this profile; explicit short user vectors fail. This choice is listed in `CompiledShader.profile_notes`.

Known engine transform uniforms accept the caller's canonical column-major 4×4 matrix when a retained stage declares a smaller matrix. Packing copies its top-left columns/rows into the actual reflected layout, including mat3 and mat4x3. This applies to an explicit list of engine matrix names; arbitrary material matrices still require their declared shape.

An inline audio function already exists in the retained audio shaders; there is no missing AudioProcessing requirement to invent. Naga distinguishes a std140 scalar array's storage stride from a function parameter array's natural stride. The adapter constructs a by-value array argument element by element while keeping the reflected uniform storage, original function and equations. This handles observed direct fixed-size array arguments, caps copies at 4096 elements, and preserves errors for writable uniform arguments, mismatched shapes or unsupported expressions. Shadowed names are left for normal scope/type checking. `validation/audio-corpus.json` checks all 13 retained inline-audio sources in 156 stereo/mask/cursor variants, with actual GLSL/Naga/MSL compilation and spectrum uniform packing.

The 0.2.1 Metal layout correction preserves padded array strides in the emitted storage declarations. Naga 27 otherwise emits both natural and std140 scalar arrays as ordinary C++ arrays, so uniform reflection and host packing can pass while a GPU reads padding or later members at the wrong offsets. `metal_layout.rs` adds padded elements with address-space-qualified subscripts and compile-time size checks, retaining the original IR and std140 host contract. Natural function-argument arrays stay unchanged. `tests/metal_uniform_layout.rs` compiles actual generated declarations as CPU C++ and reads real packed audio, skin-alpha, bloom and nested/vec2 buffers. It requires a C++ compiler and verifies storage ABI, not Metal compiler/device acceptance.

`shader::builtin_material` emits real MSL for supported CPU programs, including alpha-cut R32 shadow-depth output. `with_destination_blend` wraps the original fragment shader and samples a destination snapshot only on the original covered fragments. `depth_only_material` removes the color attachment while preserving discard/depth behavior. These helpers do not pretend to execute arbitrary custom shader code in the CPU reference.

Bindings are stage-specific: reflected uniforms use buffer 1; the native host reserves vertex buffer 30 for the canonical vertex and buffers 2–29 for additional streams. `ShaderBinding.resource_slot` is the logical Material input; `texture_slot` and densely allocated `sampler_slot` are physical Metal indices. For example logical texture24 can pair with physical sampler3. More than 16 actually used samplers in one stage errors explicitly. Naga entry-point usage excludes unused declarations from required GPU bindings, while metadata-only reflection preserves their active defaults. The host must honor the reflected dimension, array and comparison flags and actual pipeline attachment formats.

`TextureData::Float2D` carries top-down `Arc<Vec<f32>>` RGBA components. `Texture::float2d` validates finite values, component count, mips and allocation size and requires explicit R/RG/RGBA floating storage. CPU filtering and color effects retain signed/HDR values; R16/RG16/RGBA16 storage applies half rounding. Native upload uses the same explicit storage format. This is the direct path for the formats crate's RG16F/R16F decoder, without an RGBA8 conversion.

`effect::lower_separable` expands owned full-target Gaussian and bloom effects into horizontal/vertical kernels and a bloom composition pass. RGBA32F intermediate targets preserve HDR; integer reads avoid requiring float filtering for these targets. Dynamic kernel weights, radius, sizes and effect parameters use reflected uniforms. A bounded thread-local shader cache reuses the actual compiled stages across parameter changes. A partial viewport retains the direct two-dimensional fallback.

## Finite corpus result

`validation/corpus-v9-audio-arrays.json` exercises all 762 retained shader stages and all 1,021 material shader references, deduplicated to 446 declared combo/texture-format variants. It uses the actual owned shadow/cookie LightingV1 requirement from `wpe-core/src/player/lighting.rs`, preserved as `validation/lighting-requirement.glsl` for reproducibility. Individual stages using native runtime constants also have a separately labeled representative profile. See the JSON and validation summary for actual totals and per-resource diagnostics. The previous validated renderer is preserved in a deterministic archive under `validation/history/`, with its original 34-file manifest and archive hash. The later 51-file freeze is preserved separately before the audio-array correction.

The finite expected boundaries are six raw Windows HLSL stages, four geometry stages unsupported by Naga, and one workshop vertex source containing an unmatched `#endif`. The scene player supplies non-GS particle geometry. The source is preserved for malformed workshop content; it is not silently repaired. Successful MSL emission does not establish successful Apple Metal compilation, correct driver behavior or pixel equivalence with the original engine.

`validation/owned-reference.png` is an actual 640×360 CPU render: indexed textured/lit triangles, a 3D volume, shaped Latin/Arabic text, a floating render target and bloom. Geometry/pixels/text are owned test data. The external DejaVu Sans font is read only and is not bundled. No original artwork or original application executable is included in this crate.

## Verification

From the workspace:

```sh
cargo test --locked -p wpe-render --all-targets
WPE_TEST_FONT=/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf cargo test --locked -p wpe-render --test text_fonts -- --ignored
cargo clippy --locked -p wpe-render --all-targets -- -D warnings
cargo run --locked -p wpe-render --example compile_corpus -- /absolute/selected-text report.json /absolute/resources/textures.json crates/wpe-render/validation/lighting-requirement.glsl
cargo run --locked -p wpe-render --example cpu_smoke -- OUTPUT_DIRECTORY
```

The final `validation/render-validation.json` records actual checks, source hashes and the finite resource ledger. Earlier corpus attempts are retained as history.

On a physical Mac, run the shared native encoder and compare actual offscreen Metal readback against these CPU fixtures:

```sh
cargo run --locked -p wpe-macos --example metal_smoke -- OUTPUT_DIRECTORY [OPTIONAL_RETAINED_SHADER_ROOT]
```

The command creates `report.json`, CPU/GPU PNG pairs and amplified difference PNGs. It exercises indexed triangles, inter-pass depth, alpha/custom blending, generated mips, signed/HDR float sources, reflected uniform packing, two identity bones with independent skin alpha and by-value spectrum arrays, logical texture24/25 with dense physical samplers, and separable bloom. It waits for GPU completion with a bounded timeout and errors on a comparison failure. The optional retained source root additionally compiles a retained shader pair into a real Metal pipeline. These fixtures have opaque final alpha; generic `GpuReadback::poll` returns literal attachment RGBA, while `poll_straight(true)` only converts associated alpha when the caller explicitly knows that convention. Cross compilation on Linux does not count as this device execution.

The CPU reference deliberately errors on `GpuOnly` draws. Its cube filtering stays within a face; seamless edge filtering and exact hardware derivatives are not established. Color render-target mips use a 2×2 box profile; automatic depth-target mip generation is rejected. Array textures/storage buffers/geometry shaders are not represented as working features. Portable text uses outline glyphs; color/bitmap/SVG glyphs produce a capability issue and require the native CoreText raster path. Automatic font fallback and native text numerical parity are not asserted. All native GPU paths still require physical Mac validation.

The compiler is built on the primary APIs of [Naga 27](https://docs.rs/naga/27.0.0/naga/), [rustybuzz 0.20](https://docs.rs/rustybuzz/0.20.1/rustybuzz/), [fontdue 0.9](https://docs.rs/fontdue/0.9.3/fontdue/), [unicode-bidi](https://docs.rs/unicode-bidi/0.3.18/unicode_bidi/) and [unicode-linebreak](https://docs.rs/unicode-linebreak/0.1.5/unicode_linebreak/). The pinned workspace lock identifies the actual dependency set used by each build.
