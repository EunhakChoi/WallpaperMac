#!/usr/bin/env python3
"""Create this task's temporary user slice with an exact shared 8GB limit."""
from pathlib import Path
import json
import os
import subprocess
import sys

if sys.platform != "linux":
    raise SystemExit("This runtime worker limit uses Linux systemd/cgroup v2.")
runtime = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")) / "systemd/user"
runtime.mkdir(parents=True, exist_ok=True)
unit = runtime / "wallpapermac-work.slice"
description = "Description=WallpaperMac bounded implementation work"
if unit.exists() and description not in unit.read_text():
    raise SystemExit("Refusing to replace a distinct existing runtime unit")
unit.write_text("[Unit]\n" + description + "\n\n[Slice]\nMemoryAccounting=yes\nMemoryHigh=6000000000\nMemoryMax=8000000000\nMemorySwapMax=0\nTasksMax=128\n")
subprocess.run(["systemctl", "--user", "daemon-reload"], check=True)
subprocess.run(["systemctl", "--user", "start", "wallpapermac-work.slice"], check=True)
subprocess.run(["systemctl", "--user", "set-property", "--runtime", "wallpapermac-work.slice",
    "MemoryMax=8000000000", "MemoryHigh=6000000000", "MemorySwapMax=0"], check=True)
group = subprocess.check_output(["systemctl", "--user", "show", "wallpapermac-work.slice", "--property=ControlGroup", "--value"], text=True).strip()
path = Path("/sys/fs/cgroup") / group.lstrip("/")
limit = int((path / "memory.max").read_text())
if limit != 8_000_000_000:
    raise SystemExit("Kernel did not apply the requested 8GB limit")
print(json.dumps({"cgroup": group, "memory_max_bytes": limit,
    "memory_high_bytes": int((path / "memory.high").read_text()),
    "swap_max_bytes": int((path / "memory.swap.max").read_text()),
    "persistent_system_configuration_modified": False}), flush=True)
