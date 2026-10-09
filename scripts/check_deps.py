#!/usr/bin/env python3
"""Enforce the crate dependency direction from ARCHITECTURE.md §A / ADR-016.

Each crate may depend only on crates in strictly lower layers (or listed explicitly).
Run: python3 scripts/check_deps.py   (exit 1 on violation)
"""
import json, subprocess, sys

ALLOWED = {
    "veskra-core": set(),
    "veskra-color": {"veskra-core"},
    "veskra-raster": {"veskra-core"},
    "veskra-input": {"veskra-core"},
    "veskra-vector": {"veskra-core"},
    "veskra-testkit": {"veskra-core"},
    "veskra-brush": {"veskra-core", "veskra-raster", "veskra-vector", "veskra-color"},
    "veskra-doc": {"veskra-core", "veskra-raster", "veskra-vector"},
    "veskra-compose": {"veskra-core", "veskra-raster", "veskra-doc", "veskra-color"},
    "veskra-format": {"veskra-core", "veskra-raster", "veskra-doc", "veskra-color", "veskra-vector", "veskra-compose"},
    "veskra-io": {"veskra-core", "veskra-raster", "veskra-doc", "veskra-compose", "veskra-color", "veskra-vector"},
    "veskra-engine": {"veskra-core", "veskra-raster", "veskra-doc", "veskra-color", "veskra-compose", "veskra-brush",
                      "veskra-vector", "veskra-input", "veskra-format", "veskra-io"},
    "veskra-ffi": {"veskra-core", "veskra-engine", "veskra-input"},
    "veskra-cli": {"veskra-core", "veskra-engine", "veskra-io", "veskra-color", "veskra-format", "veskra-compose",
                   "veskra-doc", "veskra-raster", "veskra-testkit", "veskra-input"},
    "veskra-qa": {"veskra-core", "veskra-engine", "veskra-input", "veskra-format", "veskra-io", "veskra-color",
                  "veskra-compose", "veskra-doc", "veskra-testkit"},
}
# Crates that must never depend on JNI / Android-specific crates.
FORBIDDEN_EXTERNAL = {"jni", "ndk", "ndk-sys", "android_logger", "android-activity"}
JNI_ALLOWED = {"veskra-ffi"}

meta = json.loads(subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--no-deps"]))
bad = []
for pkg in meta["packages"]:
    name = pkg["name"]
    if name not in ALLOWED:
        bad.append(f"{name}: not registered in scripts/check_deps.py")
        continue
    for dep in pkg["dependencies"]:
        d = dep["name"]
        if d.startswith("veskra-") and dep["kind"] in (None, "normal") and d not in ALLOWED[name]:
            bad.append(f"{name} -> {d} violates layering")
        if d in FORBIDDEN_EXTERNAL and name not in JNI_ALLOWED:
            bad.append(f"{name} -> {d}: platform crate outside veskra-ffi")
if bad:
    print("\n".join(bad))
    sys.exit(1)
print(f"dependency layering OK ({len(meta['packages'])} crates)")
