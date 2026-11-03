#!/usr/bin/env python3
"""Build and exercise the supplied implementation on the user's real Mac.

Does not install the saver, assign a desktop wallpaper, publish content, launch
an external application, or enable capture/automation permissions.
"""
import argparse
import datetime
import hashlib
import json
import platform
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument("mode", nargs="?", choices=["native", "universal"], default="native")
args = parser.parse_args()
if sys.platform != "darwin":
    raise SystemExit("Run this script on macOS with Rust, Python3 and the Apple SDK installed.")
stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
output = ROOT / "verification/macos" / stamp
output.mkdir(parents=True)
records = []


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(name, command, timeout, required=True):
    print(f"[{name}] {' '.join(map(str,command))}", flush=True)
    started = time.monotonic()
    stdout = output / (name + ".stdout.txt")
    stderr = output / (name + ".stderr.txt")
    with stdout.open("wb") as out, stderr.open("wb") as err:
        try:
            process = subprocess.run(command, cwd=ROOT, stdout=out, stderr=err, timeout=timeout,
                env=dict(os.environ, CARGO_BUILD_JOBS="1", RUST_TEST_THREADS="1"))
            status = process.returncode
        except subprocess.TimeoutExpired:
            status = None
    record = dict(name=name, command=list(map(str, command)), exit_code=status, required=required,
        passed=status == 0, elapsed_seconds=time.monotonic() - started,
        stdout=str(stdout.relative_to(ROOT)), stderr=str(stderr.relative_to(ROOT)),
        stdout_sha256=sha(stdout), stderr_sha256=sha(stderr))
    records.append(record)
    print(f"[{name}] {'PASS' if status == 0 else 'FAILED'}; logs: {stderr}", flush=True)
    if status != 0:
        print(stderr.read_text(errors="replace")[-6000:], file=sys.stderr)
    return status == 0


run("host", ["sw_vers"], 10)
run("rust", ["rustc", "--version", "--verbose"], 10)
run("apple-sdk", ["xcrun", "--show-sdk-path"], 15)
run("format", ["cargo", "fmt", "--all", "--", "--check"], 60)
run("clippy", ["cargo", "clippy", "--workspace", "--all-targets", "--keep-going", "--locked", "--", "-D", "warnings"], 600)
run("tests", ["cargo", "test", "--workspace", "--no-fail-fast", "--locked"], 600)
if shutil.which("node"):
    run("web-bridge", ["node", "crates/wpe-macos/tests/web_bridge.cjs"], 30)
else:
    records.append(dict(name="web-bridge", required=False, passed=False, status="not-run", reason="Node.js is not installed; this does not block the native app"))
built = run("bundles", ["bash", "scripts/build-macos.sh", args.mode], 2400)
smoke = run("metal-smoke", ["cargo", "run", "--locked", "-p", "wpe-macos", "--example", "metal_smoke", "--", str(output / "metal")], 300)
if built:
    for name in ["WallpaperMac.app", "WallpaperMac.saver"]:
        run(name + "-signature", ["codesign", "--verify", "--strict", str(ROOT / "dist" / name)], 30)
report = dict(created_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),
    host=dict(system=platform.system(), release=platform.release(), architecture=platform.machine()),
    cargo_lock_sha256=sha(ROOT / "Cargo.lock"), checks=records,
    passed=all(r["passed"] for r in records if r["required"]),
    native_metal_smoke_command_passed=smoke,
    manual_tests_required=["App launch and settings", "desktop/Spaces/Stage Manager/multiple displays", "video/audio output", "ScreenCaptureKit permissions and capture", "ScreenSaver.framework preview/lifetime", "Music/Spotify Automation", "Steamworks SDK ABI and live account", "external application window capture"],
    original_engine_equivalence_established=False)
(output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps({"passed": report["passed"], "report": str(output / "report.json")}), flush=True)
raise SystemExit(0 if report["passed"] else 1)
