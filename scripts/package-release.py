#!/usr/bin/env python3
"""Package exactly the executable and non-secret build metadata for one target."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import re
import subprocess
import tarfile


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--commit", required=True)
    args = parser.parse_args()
    if not re.fullmatch(r"[a-z0-9_-]+", args.target):
        parser.error("Invalid target")
    if not re.fullmatch(r"[a-fA-F0-9]{40,64}", args.commit):
        parser.error("A full commit digest is required")
    version_output = subprocess.check_output([str(args.binary.resolve()), "--version"], text=True).strip()
    match = re.fullmatch(r"agent-instance (\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)", version_output)
    if not match:
        parser.error("Binary did not identify as agent-instance")
    metadata = {
        "name": "agent-instance",
        "version": match.group(1),
        "target": args.target,
        "commit": args.commit,
        "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
    }
    args.output.mkdir(parents=True, exist_ok=True)
    binary_name = "agent-instance.exe" if "windows" in args.target else "agent-instance"
    archive = args.output / ("agent-instance-" + args.target + ".tar.gz")
    encoded = (json.dumps(metadata, indent=2) + "\n").encode()
    with tarfile.open(archive, "w:gz") as bundle:
        binary_info = bundle.gettarinfo(str(args.binary), arcname=binary_name)
        binary_info.uid = binary_info.gid = 0
        binary_info.uname = binary_info.gname = ""
        binary_info.mode = 0o700
        binary_info.mtime = 0
        with args.binary.open("rb") as binary:
            bundle.addfile(binary_info, binary)
        information = tarfile.TarInfo("build-info.json")
        information.mode = 0o600
        information.size = len(encoded)
        bundle.addfile(information, io.BytesIO(encoded))
    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_suffix(archive.suffix + ".sha256").write_text(checksum + "  " + archive.name + "\n")
    print(archive)


if __name__ == "__main__":
    main()
