# Portable platform services

Actual configuration, inventory, playlist/preset/profile/rule decisions, recovered audio DSP, PCM decoding, external process configuration, media metadata/image processing, compressed-audio fallback, Workshop request contracts and optional Steamworks dynamic FFI live here. These services run independently of AppKit and Metal.

`ConfigStore` validates before an atomic owned-file save. Libraries are read-only; editor working copies and exported copies preserve source bytes and reject unsafe paths/symlinks. Missing legacy project type is inferred from the declared entry; an explicit unknown type remains unknown. App rules apply the recovered action masks, exemptions and last matching load/profile behavior. Runtime playlist state persists.

The audio transform implements the recovered size/prefix/complex-input, bin remap, channel reduction, gain/threshold/normalization and frame smoothing equations. Independent FFT/DFT and buffer tests pass; this scalar implementation is not certified bit-equivalent to the original SIMD backend or any Mac capture format. No audio device is opened by the DSP/decoder tests.

`steam::SteamProvider` loads an explicitly supplied official flat-API library. The pinned profile is UGC_v020, Utils_v010, optional User_v023, UserStats_v012 and Apps_v008. The provider checks the initialized AppID, owns callback/query releases and shuts down before unloading. Query/publication/subscription/download/delete DLC and schema-checked publication achievement calls are actual FFI, not public-HTTP substitutes. Real client credentials, SDK headers, Mac library and remote-service permission were unavailable here. Run `tools/verify-steam-abi.cpp` against your official SDK before enabling a matching profile. The Linux owned C mock checks ABI calls, callback lifetime, errors, AppID separation, query release and mutation dispatch; it does not prove compatibility with a particular SDK binary.

`export::FfmpegEncoder` passes bounded raw RGBA frames to a user-configured absolute encoder path without a shell. The queue applies backpressure, finish closes input and observes process exit, and cancel terminates the owned process group. Codec availability is an external capability; requesting WMV3 does not silently substitute WMV2. Native Metal/AVFoundation export lives in `wpe-macos`.

```sh
cargo test --locked -p wpe-platform
cargo run --locked -p wpe-platform --example audio_decode_check -- /path/to/audio.ogg
cargo run --locked -p wpe-platform --example export_check -- /absolute/path/to/ffmpeg /new/owned-check.mp4
```

The export example generates its own32×32 color frames without source wallpaper assets or audio. `export-probe.json` records the Linux H264 result. `vorbis-smoke.json` records a read-only decode of a locally installed Ogg file without bundling the source. Neither is a Mac GPU/media-device test.

Primary service APIs: [Steam UGC](https://partner.steamgames.com/doc/api/ISteamUGC), [Steam User](https://partner.steamgames.com/doc/api/ISteamUser), [Steam Apps](https://partner.steamgames.com/doc/api/ISteamApps), reviewed 2026-10-28. External identity, schema and remote permissions stay explicit.
