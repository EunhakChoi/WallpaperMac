# wpe-formats

Actual bounded Rust readers for the pinned local Wallpaper Engine formats. This
crate does not execute original applications, scripts, shaders or animation poses,
and does not bundle original artwork.

- `pkg::Package` reads PKGV index layouts through version24. `Member` offsets are
  absolute file offsets. `get` matches exact spelling; `member_data(index)` lets
  the VFS apply lowercase lookup and last-duplicate priority. Empty members remain
  distinguishable from missing members.
- `jsonc::parse` accepts UTF-8/BOM, line/block comments and trailing commas while
  preserving comment-like characters and escaped quotes inside strings. Errors
  include original byte positions.
- `tex::Texture` reads TEXV4 legacy and TEXV5/TEXI1, TEXB1–4 and TEXS1–3, retaining storage/logical
  dimensions, image/mip arrays, sprite words, volume depth, video fields, flags
  and trailing bytes. Raw LZ4 is decoded as a block without a frame or a prepended
  length. Pixel decoding supports observed RGBA8, BC3, RG8, R8, PNG and JPEG,
  plus standard-tested BC1/DXT1(7), BC2/DXT3(6), BC7(12), RG16F(10) and R16F(11).
  RG8/R8 preserve native component placement as `[r,g,0,255]`/`[r,0,0,255]`;
  they do not synthesize grayscale or normal-map channels. Output is the entire
  selected storage mip, with top-down rows, no logical crop or premultiplication.
  A 2D decode request for volume/video, or an unimplemented codec, returns an
  explicit `Unsupported` error while parsed metadata remains available.
- `tex::Texture::decode_image` returns `DecodedImage::Rgba8` or
  `DecodedImage::Rgba32Float(FloatImage { width, height, rgba })`.
  `decode_rgba32f` expands finite little-endian IEEE binary16 to float32 exactly,
  retaining negative values, HDR range, signed zero and subnormals. Missing
  channels are zero and alpha is one. Raw floating TEX deliberately errors from
  `decode_rgba8`; there is no implicit clipping or tone map. NaN/Inf payloads and
  allocation overflow return typed errors; original stored bytes remain available.
- `tex::encode_rgba8` writes a single 2D RGBA8 image/mip without LZ4, for owned
  demos and roundtrip fixtures. It does not implement every original compiler
  option or the complete texture exporter.
- `mdl::Model` reads observed MDLV4/14/17/23, mesh buffers, 16-bit indices, optional
  mesh data, batches, MDLS4 skeleton/constraints, MDAT1 attachments, MDLE2 matrices
  and MDLA6 dense channels/events. Exact raw mesh/channel bytes remain accessible.
  Observed skin indices/weights are retained without normalization. Unknown
  sections are explicitly listed and retained, with `fully_typed=false` in the
  summary. Structural parsing does not claim pose/rendering equivalence or invent
  meanings for unnamed native words.
- `mdl::SectionData::Morph` parses `MDMP0001` per-mesh target ordinals, opaque
  IDs/names, base influence, declared/effective vertex counts and IEEE binary16
  position/normal/auxiliary vector/scalar channels. Optional bone/rule/range words
  and the exact raw section body remain available. Blob sizes, component strides
  and optional skeleton indices are checked. This local corpus contains no MDMP
  sample: owned fixtures test the schema recovered from static native reader and
  GPU-producer code. Auxiliary channel meanings and original GPU equivalence are
  not inferred from structural parsing. The CPU position/normal implementation
  and its limits are implemented in
  [the model simulator](../wpe-sim/src/model.rs).

`fbx::decode_to_editable` imports bounded ASCII/binary FBX7 into the real editable model profile. It triangulates concave polygons with checked control-point/layer mappings, transforms geometry/normals by the file axis/unit/pivot settings, preserves indexed hierarchy and normalized skin weights, and samples named skeletal clips into N+1 local matrices. Actual `AnimationCurve` → property/axis → model and layer/stack connections determine the animation. Constant, linear and stored user-tangent cubic curves (including monotonic weighted tangents) execute; unavailable auto/TCB/velocity policies fail explicitly. Input32MiB, expanded binary arrays32MiB, 25,000 nodes, 4,096 objects, 256 transform nodes and two million geometry/animation numeric values bound the importer. It supports one active100%-weight layer per stack, Normalize skins with at most four active influences, and the declared TRS/inheritance profile. FBX materials/textures and shape animation remain metadata; assign actual project materials in the editor. The chosen stored UV layer is explicitly identified in metadata. Name-based retarget refuses ambiguous bones.

Owned ASCII and compressed binary controls check the same real geometry, skin, moving animation keys and malformed inputs. Editor integration checks actual skinned-vertex motion, retained MDL bytes and undo/save behavior. See [the FBX regression tests](tests/fbx_import.rs). Autodesk SDK is not bundled or executed.

`Error` distinguishes parse, unsupported, allocation-limit and decode failures.
All binary reads use checked ranges and integer size calculations. Single TEX/
MDL inputs and decoded buffers use a 512MiB allocation policy; embedded image
decoders additionally limit dimensions to32768. These are implementation limits,
not original-engine format restrictions. Public `RgbaImage` fields can be checked
with `validate()` before serialization/rendering.

Run self-contained tests from the workspace:

```sh
cargo test --locked -p wpe-formats
```

The external original-writer test also compares triangle positions and OBJ UV
conversion, preserves exact source RGBA, and roundtrips those owned pixels through
the new writer:

```sh
WPE_CORPUS_ROOT=/path/to/investigations/wallpaper-engine cargo test --locked -p wpe-formats --test original_writers -- --ignored
```

The optional external-corpus validator reads the frozen investigation indexes and
their original loose/package byte ranges. It checks all member/texture/mesh
metadata, all raw LZ4 payloads and supported 2D mip decodes, JSONC documents, and
six controls from already executed original compiler outputs. It writes a JSON
report to stdout and never modifies its source corpus:

```sh
cargo run --locked -p wpe-formats --example validate_corpus -- /path/to/investigations/wallpaper-engine
```

Metadata/decode checks and exact owned RGBA controls do not establish original GPU
sampling, blend-state, color-space, animation or complete engine compatibility.
The CPU BC3 profile uses RGB565 bit replication and integer-truncated linear
interpolation; GPU rounding equivalence is unverified. The block/alpha structure
follows Microsoft's [BC format description](https://learn.microsoft.com/en-us/windows/uwp/graphics-concepts/block-compression).

The extended codec validation uses the [owned texture fixtures](validation/texture-extension-fixtures/)
uses owned normative bytes, not newly discovered original assets. Its independent
Pillow decoder comparisons cover 2,048 valid BC7 blocks across all eight modes,
exhausting each mode's partition, rotation and index-selector header combinations,
BC1/BC2 blocks and every width/height from 1 through 9. Python's binary16 conversion
checks all 63,488 finite half encoding against exact float32 bits. Unit tests
also reject all 2,048 nonfinite half encodings, check LZ4/truncation/mip bounds,
BC1 transparency, all 16 BC2 alpha nibbles and BC7 mode6 interpolation. BC7 uses the
pinned safe pure Rust [bcdec_rs 0.2.1](https://github.com/ScanMountGoat/image_dds),
whose exact version is pinned in Cargo.lock. [Microsoft's BC7 specification](https://learn.microsoft.com/en-us/windows/win32/direct3d11/bc7-format)
is the normative codec contract; the original engine's upload/color-space behavior
still needs original-file/GPU comparison for these unobserved renderer formats.

The saved native TEXV4 dispatch calls the same information/body readers with
version zero. Accordingly its header has six u32 words (plus volume depth when
flag 0x40), no average-color word or TEXI/TEXB markers, an implicit single image,
then a mip count and width/height/(depth)/byte-count/payload records without LZ4
fields. Native source paths/hashes are pinned by the extension report. The
installed 623-file corpus contains no TEXV4 or formats 6/7/10/11/12 sample;
legacy fixtures establish the recovered reader contract, not an original-writer
or visual oracle. Unknown trailing legacy data is retained rather than invented
as a sprite section. ETC1/ETC2 and other unimplemented codes remain explicit errors.
