#!/usr/bin/env python3
"""Verify the published archive checksum and extract only its expected files."""
import hashlib
import json
import os
from pathlib import Path
import re
import sys
import tarfile


def extract(archive, checksum, destination):
    expected = checksum.read_text().strip()
    match = re.fullmatch(r"([a-fA-F0-9]{64})\s+\*?" + re.escape(archive.name), expected)
    if not match:
        raise ValueError("Checksum must name exactly the requested release archive")
    digest = hashlib.sha256()
    with archive.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    if digest.hexdigest() != match.group(1).lower():
        raise ValueError("Release archive checksum mismatch")
    destination.mkdir(mode=0o700)
    with tarfile.open(archive, "r:gz") as bundle:
        members = bundle.getmembers()
        binary_name = "agent-instance.exe" if "windows" in archive.name else "agent-instance"
        if {member.name for member in members} != {binary_name, "build-info.json"} or len(members) != 2:
            raise ValueError("Unexpected files in release archive")
        for member in members:
            if not member.isfile() or not 0 < member.size < 512 * 1024 * 1024:
                raise ValueError("Invalid release archive member")
            descriptor = os.open(destination / member.name, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o700 if member.name == binary_name else 0o600)
            with os.fdopen(descriptor, "wb") as output, bundle.extractfile(member) as source:
                for chunk in iter(lambda: source.read(1024 * 1024), b""):
                    output.write(chunk)
    metadata = json.loads((destination / "build-info.json").read_text())
    if metadata.get("name") != "agent-instance" or not re.fullmatch(r"[a-fA-F0-9]{40,64}", str(metadata.get("commit", ""))):
        raise ValueError("Invalid release build metadata")
    if hashlib.sha256((destination / binary_name).read_bytes()).hexdigest() != metadata.get("binary_sha256"):
        raise ValueError("Executable does not match its build metadata")


if __name__ == "__main__":
    try:
        extract(*(Path(value) for value in sys.argv[1:]))
    except (ValueError, OSError, tarfile.TarError) as error:
        print("Release verification failed: " + str(error), file=sys.stderr)
        raise SystemExit(1)
