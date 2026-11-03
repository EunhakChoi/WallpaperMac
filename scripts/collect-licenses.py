#!/usr/bin/env python3
"""Collect actual pinned dependency license files for a distributable bundle."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument("--output", required=True, type=Path)
args = parser.parse_args()
metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--format-version", "1"], cwd=root))
args.output.mkdir(parents=True, exist_ok=True)
rows = []
fallback_root = root / "packaging/dependency-licenses"
fallback_manifest = fallback_root / "manifest.json"
fallbacks = {}
if fallback_manifest.is_file():
    fallbacks = {(row["name"], row["version"]): row
        for row in json.loads(fallback_manifest.read_text())["packages"]}
for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
    if package["source"] is None:
        continue
    source = Path(package["manifest_path"]).parent
    destination = args.output / (package["name"] + "-" + package["version"])
    destination.mkdir(exist_ok=True)
    candidates = set()
    if package.get("license_file"):
        candidates.add(source / package["license_file"])
    for file in source.iterdir():
        if file.name.lower().startswith(("license", "licence", "copying", "copyright", "notice", "authors")):
            if file.is_file():
                candidates.add(file)
            elif file.is_dir():
                candidates.update(p for p in file.rglob("*") if p.is_file())
    files = []
    for file in sorted(candidates):
        if not file.is_file():
            continue
        relative = file.relative_to(source)
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(file, target)
        files.append({"path": str(relative), "sha256": hashlib.sha256(file.read_bytes()).hexdigest()})
    if not files:
        fallback = fallbacks.get((package["name"], package["version"]))
        if fallback is None or fallback["license"] != package["license"]:
            raise SystemExit("Missing actual license texts for pinned dependency: " + package["name"] + "-" + package["version"])
        for record in fallback["files"]:
            file = root / record["path"]
            if not file.resolve().is_relative_to(fallback_root.resolve()):
                raise SystemExit("Invalid pinned license path")
            data = file.read_bytes()
            if len(data) != record["bytes"] or hashlib.sha256(data).hexdigest() != record["sha256"]:
                raise SystemExit("Pinned license text changed: " + str(file))
            relative = Path("upstream") / record["upstream_path"]
            if relative.is_absolute() or ".." in relative.parts:
                raise SystemExit("Invalid upstream notice name")
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
            files.append({"path": str(relative), "sha256": record["sha256"],
                "source_url": record["url"], "published_crate_git_commit": fallback["git_commit"]})
    rows.append({"name": package["name"], "version": package["version"], "license": package["license"], "repository": package["repository"], "source": package["source"], "files": files})
(args.output / "index.json").write_text(json.dumps({"packages": rows, "cargo_lock_sha256": hashlib.sha256((root / "Cargo.lock").read_bytes()).hexdigest()}, indent=2) + "\n")
(args.output / "README.txt").write_text("Pinned Rust dependencies and their actual upstream license files are indexed in index.json.\nThe source implementation and Cargo.lock are supplied with WallpaperMac.\nNo Wallpaper Engine artwork, runtime binaries, Valve Steamworks SDK or system fonts are bundled.\n")
print(json.dumps({"packages": len(rows), "output": str(args.output)}))
