"""Produce the optional Unix Codex adapter as a reproducible Python zip application."""

import argparse
import hashlib
import subprocess
import sys
import zipfile
from pathlib import Path


def package(destination):
    root = Path(__file__).resolve().parents[1]
    source = root / "prototypes" / "codex-app-server"
    destination.mkdir(exist_ok=True, parents=True)
    archive = destination / "codex-app-server-adapter.pyz"
    with zipfile.ZipFile(archive, "w") as bundle:
        for name in ("adapter.py", "host.py", "artifact.py", "README.md", "LICENSE"):
            path = (root if name == "LICENSE" else source) / name
            bundle.writestr(zipfile.ZipInfo(name), path.read_bytes())
        bundle.writestr(zipfile.ZipInfo("__main__.py"),
                        'import runpy\nrunpy.run_module("adapter", run_name="__main__")\n')
    subprocess.run([sys.executable, "-I", str(archive), "--help"], check=True, stdout=subprocess.DEVNULL)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_suffix(".pyz.sha256").write_text(f"{digest}  {archive.name}\n")
    return archive


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    print(package(parser.parse_args().output))
