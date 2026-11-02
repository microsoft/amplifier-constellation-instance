#!/usr/bin/env python3
"""Installation helpers only; the running agent does not depend on Python."""
import datetime
import ipaddress
import json
import os
from pathlib import Path
import re
import subprocess
import sys
from urllib.parse import urlsplit


def save_private(path, value):
    data = json.dumps(value, indent=2) + "\n"
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as stream:
        stream.write(data)


def unit_quote(value, *, command=False):
    if any(char in value for char in "\r\n\0"):
        raise ValueError("Service paths may not contain control characters")
    if not command:
        # WorkingDirectory= is a single path, not an ExecStart word list. Quotes
        # would become part of the path and make an absolute path appear relative.
        return value.replace("%", "%%")
    result = value.replace("\\", "\\\\").replace('"', '\\"').replace("%", "%%")
    return '"' + result + '"'


def read_endpoints(home):
    return json.loads((home / "endpoints.json").read_text())


def valid_url(value, *, welcome=False):
    if not isinstance(value, str):
        return False
    parsed = urlsplit(value)
    return parsed.scheme in (("http", "https") if welcome else ("https",)) and bool(parsed.hostname) and not parsed.username and not parsed.password and not parsed.query and not parsed.fragment and not any(ord(char) < 32 for char in value)


def main():
    command, *args = sys.argv[1:]
    if command == "preflight":
        home = Path(args[0]).resolve()
        ipaddress.ip_address(args[1])
        if str(home) != str(home).strip() or "\\" in str(home) or any(ord(char) < 32 for char in str(home)):
            raise ValueError("Instance paths cannot contain backslashes/control characters or end in whitespace")
        if home.exists() and (not home.is_dir() or any(home.iterdir())):
            raise ValueError("Choose a new or empty --home folder; existing files are never replaced")
    elif command == "package-field":
        text = Path(args[0]).read_text()
        package = re.search(r"(?ms)^\[package\]\s*(.*?)(?=^\[|\Z)", text)
        match = re.search(r'^' + re.escape(args[1]) + r'\s*=\s*"([^"]+)"', package.group(1), re.M) if package else None
        if not match:
            raise ValueError("Missing package field " + args[1])
        print(match.group(1))
    elif command == "rust-at-least":
        output = subprocess.check_output(["rustc", "--version"], text=True)
        current = re.search(r"rustc (\d+)\.(\d+)\.(\d+)", output)
        required = tuple(map(int, args[0].split(".")))
        if not current or tuple(map(int, current.groups())) < required:
            raise SystemExit(1)
    elif command == "metadata":
        home = Path(args[0])
        version, digest, source, repo, ref, commit, dirty = args[1:]
        metadata = {
            "schema_version": 1,
            "installed_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "version": version,
            "binary_sha256": digest,
            "source": source,
            "repository": repo,
            "requested_ref": ref or None,
            "commit": commit or None,
            "dirty_checkout": {"true": True, "false": False}.get(dirty),
            "platform": sys.platform,
            "architecture": os.uname().machine,
            "home": str(home),
        }
        save_private(home / "install.json", metadata)
        save_private(home / "releases" / (version + "-" + digest[:12]) / "release.json", metadata)
    elif command == "unit":
        home = Path(args[0])
        config = json.loads((home / "config.json").read_text())["config"]
        instance_id = config["instance_id"]
        if not re.fullmatch(r"[a-zA-Z0-9-]{1,80}", instance_id):
            raise ValueError("Invalid instance ID for service name")
        unit_name = "agent-instance-" + instance_id + ".service"
        unit = "\n".join([
            "[Unit]",
            "Description=Standalone agent instance " + instance_id,
            "After=network-online.target",
            "Wants=network-online.target",
            "",
            "[Service]",
            "Type=simple",
            "WorkingDirectory=" + unit_quote(str(home)),
            # ':' disables environment expansion, preserving literal '$' in paths.
            "ExecStart=:" + unit_quote(str(home / "bin/agent-instance"), command=True) + " serve --home " + unit_quote(str(home), command=True),
            "Restart=on-failure",
            "RestartSec=5",
            "TimeoutStopSec=30",
            "KillMode=control-group",
            "UMask=0077",
            "",
            "[Install]",
            "WantedBy=default.target",
            "",
        ])
        descriptor = os.open(home / "agent-instance.service", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "w") as stream:
            stream.write(unit)
        print(unit_name)
    elif command == "endpoints":
        home = Path(args[0])
        endpoints = read_endpoints(home)
        welcome = [endpoints.get("welcome_url")] + endpoints.get("welcome_urls", [])
        welcome = list(dict.fromkeys(value for value in welcome if valid_url(value, welcome=True)))
        if welcome:
            print("Start here for connection/certificate help (no sign-in on this page):")
            for url in welcome:
                print("  " + url)
        candidates = [endpoints.get("base_url")] + endpoints.get("urls", [])
        print("HTTPS sign-in/setup/dashboard URLs (reachability depends on the host network):")
        for url in dict.fromkeys(value for value in candidates if valid_url(value)):
            print("  " + url)
        print("Server certificate SHA-256: " + str(endpoints.get("certificate_sha256", "not reported")))
        print("Trust certificate SHA-256: " + str(endpoints.get("trust_certificate_sha256", endpoints.get("certificate_sha256", "not reported"))))
        print("Compare the trust certificate fingerprint before installing it on another device. Its private key stays on this host.")
        print("No credentials are included in these addresses.")
    elif command == "browser-url":
        endpoints = read_endpoints(Path(args[0]))
        preferred = endpoints.get("preferred_url")
        if endpoints.get("preferred_url_verified") is True and valid_url(preferred):
            print(preferred)
        elif valid_url(endpoints.get("welcome_url"), welcome=True):
            print(endpoints["welcome_url"])
        elif valid_url(endpoints.get("base_url")):
            print(endpoints["base_url"])
    else:
        raise ValueError("Unknown helper command: " + command)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, IndexError, json.JSONDecodeError) as error:
        print("Install error: " + str(error), file=sys.stderr)
        raise SystemExit(1)
