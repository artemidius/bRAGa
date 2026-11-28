# SPDX-License-Identifier: Apache-2.0
"""Package the prebuilt ``braga_ffi`` cdylib into the wheel.

``cargo`` already produced the shared object, so there is nothing to compile here: the hook copies
``target/{release,debug}`` into ``braga/_native/`` and falls back to one ``cargo build --release -p
braga-ffi`` when neither profile has the artifact yet.
"""

import shutil
import subprocess
import sys
from pathlib import Path

from setuptools import setup
from setuptools.command.build_py import build_py as _build_py

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parent
NAME = {
    "darwin": "libbraga_ffi.dylib",
    "win32": "braga_ffi.dll",
}.get(sys.platform, "libbraga_ffi.so")


def _library() -> Path:
    """Returns the prebuilt cdylib, building it once if ``cargo`` has not produced one yet."""
    for profile in ("release", "debug"):
        candidate = REPO / "target" / profile / NAME
        if candidate.is_file():
            return candidate
    subprocess.run(
        ["cargo", "build", "--release", "-p", "braga-ffi"], cwd=REPO, check=True
    )
    return REPO / "target" / "release" / NAME


class build_py(_build_py):  # noqa: N801 - setuptools command class name
    """Copies the cdylib into the built package as ``braga/_native/`` data."""

    def run(self) -> None:
        super().run()
        source = _library()
        target = Path(self.build_lib) / "braga" / "_native"
        target.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target / source.name)


setup(cmdclass={"build_py": build_py})
