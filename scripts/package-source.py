#!/usr/bin/env python3
"""Build a deterministic, hash-indexed source handoff without original assets."""
import argparse
import hashlib
import json
from pathlib import Path
import stat
import re
import zipfile

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument("--output", type=Path, default=ROOT / "dist")
args = parser.parse_args()
version = re.search(r'^version\s*=\s*"([^"]+)"', (ROOT / "Cargo.toml").read_text(), re.MULTILINE).group(1)
prefix = "WallpaperMac-" + version
args.output.mkdir(parents=True, exist_ok=True)
paths = set()
for path in ROOT.iterdir():
    if path.is_file() and (path.suffix in {".md", ".toml"} or path.name in {"Cargo.lock", ".gitignore"}):
        paths.add(path)
for directory in [".cargo", "crates", "packaging", "scripts"]:
    for path in (ROOT / directory).rglob("*"):
        if not path.is_file() or "__pycache__" in path.parts:
            continue
        if "validation" in path.parts and "texture-extension-fixtures" not in path.parts:
            continue
        if path.suffix in {".log", ".zip", ".gz", ".exe", ".dll", ".pyc"}:
            continue
        paths.add(path)
# Summaries retain the distinction between new-code tests and original evidence.
summaries = [
    "verification/report.json", "verification/scene-corpus-final.json",
    "verification/publish-achievement-contract.json", "implementation-start.json",
    "verification/memory-policy.json", "verification/puppet-channel-stream-contract.json",
    "verification/license-collection-check.json",
    "verification/mac-feedback-20261103/resolution.json",
    "verification/mac-feedback-20261103/user-report.md",
    "verification/mac-feedback-20261103/local-checks.json",
    "verification/mac-feedback-20261103/counterfactual-cpu-model.json",
    "verification/mac-feedback-20261103/baseline-layout-mismatches.json",
    "crates/wpe-core/validation/results.json",
    "crates/wpe-render/validation/render-validation.json",
    "crates/wpe-render/validation/metal-smoke-readiness.json",
    "crates/wpe-formats/validation/texture-final-validation.json",
    "crates/wpe-formats/validation/texture-extension-oracles.json",
    "crates/wpe-formats/validation/fbx-validation.json",
    "crates/wpe-script/validation/results.json",
    "crates/wpe-script/validation/reference-execution-capture.json",
    "crates/wpe-script/validation/model-recovery/report.md",
    "crates/wpe-editor/validation/results.json",
    "crates/wpe-macos/validation.json",
]
paths.update(ROOT / name for name in summaries if (ROOT / name).is_file())
paths.update((ROOT / "verification").glob("*.py"))
entries = []
for path in sorted(paths):
    if path.is_symlink():
        raise SystemExit("Source handoff refuses symlinks: " + str(path))
    data = path.read_bytes()
    entries.append((str(path.relative_to(ROOT)), data, bool(path.stat().st_mode & stat.S_IXUSR)))
rows = [{"path": name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()} for name, data, _ in entries]
manifest = {
    "kind": "independent-implementation-source-handoff", "version": version,
    "files": rows, "file_count": len(rows),
    "original_wallpaper_artwork_runtime_sdk_or_system_fonts_included": False,
    "dependencies": "Pinned Cargo.lock; Cargo downloads dependencies on the first build.",
    "physical_mac_validation_performed": False,
}
manifest_data = (json.dumps(manifest, indent=2) + "\n").encode()
archive = args.output / (prefix + "-source.zip")
with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as output:
    for name, data, executable in entries + [("source-manifest.json", manifest_data, False)]:
        entry = zipfile.ZipInfo(prefix + "/" + name, date_time=(2026, 10, 28, 0, 0, 0))
        entry.create_system = 3
        entry.external_attr = (stat.S_IFREG | (0o755 if executable else 0o644)) << 16
        entry.compress_type = zipfile.ZIP_DEFLATED
        output.writestr(entry, data)
with zipfile.ZipFile(archive) as check:
    if check.testzip() is not None:
        raise SystemExit("Archive CRC check failed")
    for row in rows:
        data = check.read(prefix + "/" + row["path"])
        if len(data) != row["bytes"] or hashlib.sha256(data).hexdigest() != row["sha256"]:
            raise SystemExit("Archive identity check failed: " + row["path"])
descriptor = {
    "path": str(archive.relative_to(ROOT)) if archive.is_relative_to(ROOT) else str(archive),
    "bytes": archive.stat().st_size, "sha256": hashlib.sha256(archive.read_bytes()).hexdigest(),
    "manifest_sha256": hashlib.sha256(manifest_data).hexdigest(), "source_files": len(rows),
    "archive_crc_and_every_member_hash_passed": True,
    "macOS_binaries_included": False,
}
(ROOT / "verification/source-package.json").write_text(json.dumps(descriptor, indent=2) + "\n")
print(json.dumps(descriptor), flush=True)
