#!/usr/bin/env python3
"""Exercise the completion contract over trusted HTTPS on a disposable instance.

The source and product are explicit fixtures. Separate subprocess checks inspect
actual files; separate scoped credentials submit review reports. No model, real
account password, production instance, or external conversation is used.
"""
import argparse
import datetime
import functools
import hashlib
import http.server
import json
import os
from pathlib import Path
import platform
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")
    path.chmod(0o600)


class Instance:
    def __init__(self, binary, home):
        self.binary, self.home = binary, home
        self.process = None
        self.calls = []
        subprocess.run([str(binary), "--home", str(home), "init", "--name",
                        "Disposable completion validation", "--bind", "127.0.0.1", "--port", "0"],
                       check=True, capture_output=True, timeout=30)
        self.credentials = {"administrator": (home / "secrets/control-token").read_text().strip()}
        self.principals = {}
        self.log = (home / "validation.log").open("w")
        self.start()

    def start(self):
        (self.home / "endpoints.json").unlink(missing_ok=True)
        self.process = subprocess.Popen([str(self.binary), "--home", str(self.home), "serve"],
                                        stdout=self.log, stderr=self.log)
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError("Disposable instance exited before it became ready")
            if (self.home / "endpoints.json").exists():
                self.url = json.loads((self.home / "endpoints.json").read_text())["base_url"]
                self.tls = ssl.create_default_context(cafile=str(self.home / "tls/root-ca.crt"))
                return
            time.sleep(.05)
        raise TimeoutError("Disposable instance did not start")

    def stop(self):
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)

    def api(self, method, path, body=None, actor="requester", expected=200):
        headers = {"Content-Type": "application/json"}
        if actor is not None:
            headers["Authorization"] = "Bearer " + self.credentials[actor]
        request = urllib.request.Request(self.url + "/api/v1/" + path, method=method,
            data=None if body is None else json.dumps(body).encode(), headers=headers)
        try:
            with urllib.request.urlopen(request, context=self.tls, timeout=20) as response:
                status, value = response.status, json.load(response)
        except urllib.error.HTTPError as error:
            status, value = error.code, json.load(error)
        # Credentials only remain in memory. Token creation receipts keep IDs/scopes.
        retained = {key: item for key, item in value.items() if key != "token"} if isinstance(value, dict) else value
        self.calls.append({"method": method, "path": path, "actor": actor,
                           "request": body, "status": status, "response": retained})
        if status != expected:
            raise AssertionError(f"{method} {path}: expected {expected}, received {status}: {retained}")
        return value

    def issue(self, name):
        grant = self.api("POST", "tokens", {"name": name, "scopes": ["work", "work:read"],
                         "expires_in_seconds": 600}, "administrator")
        self.credentials[name] = grant["token"]
        self.principals[name] = "token:" + grant["id"]

    def command(self, work, action, actor="requester", expected=200):
        return self.api("POST", "work/" + work["id"] + "/commands",
                        {"expected_revision": work["revision"], "command": action}, actor, expected)


class QuietFiles(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *_):
        pass


def checks(report_dir, source, artifact, version):
    """Two independently launched deterministic checks, not model testimony."""
    programs = {
        "bytes": """import hashlib,json,pathlib,sys
source=json.loads(pathlib.Path(sys.argv[1]).read_text())
artifact=pathlib.Path(sys.argv[2]); data=artifact.read_bytes()
assert data.decode().splitlines()[0] == '# ' + source['title']
assert data.endswith(b'\\n')
print(json.dumps({'passed':True,'checks':2,'source_sha256':hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest(),'artifact_sha256':hashlib.sha256(data).hexdigest()}))
""",
        "structure": """import hashlib,json,pathlib,sys
source=json.loads(pathlib.Path(sys.argv[1]).read_text())
artifact=pathlib.Path(sys.argv[2]); text=artifact.read_text()
assert text.partition('\\n')[0].removeprefix('# ') == source['title']
assert len(text.splitlines()) >= 3 and text.endswith('\\n')
print(json.dumps({'passed':True,'checks':2,'source_sha256':hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest(),'artifact_sha256':hashlib.sha256(artifact.read_bytes()).hexdigest()}))
"""}
    receipts = []
    for name, program in programs.items():
        script = report_dir / ("check-" + name + ".py")
        script.write_text(program)
        before = {"source": digest(source), "artifact": digest(artifact), "checker": digest(script)}
        result = subprocess.run([sys.executable, str(script), str(source), str(artifact)],
                                capture_output=True, text=True, timeout=15)
        after = {"source": digest(source), "artifact": digest(artifact), "checker": digest(script)}
        assert result.returncode == 0, result.stderr
        assert before == after, "Checker or input changed during verification"
        output = json.loads(result.stdout)
        assert output["passed"] and output["artifact_sha256"] == before["artifact"]
        receipt = {"schema": "agent-instance.executed-check-fixture/1", "version": version,
                   "check": name, "executor": "separate Python subprocess", "exit_code": result.returncode,
                   "inputs_unchanged": True, "sha256": before, "output": output}
        path = report_dir / f"executed-check-{version}-{name}.json"
        save(path, receipt)
        receipts.append(path)
    return receipts


def review(work, evidence_ids, outcome="pass"):
    contract = work["completion_contract"]
    return {"type": "review_artifacts", "brief_sha256": contract["brief"]["sha256"],
            "artifact_set_sha256": contract["artifact_set"]["sha256"], "outcome": outcome,
            "criterion_indices": [0, 1], "evidence_ids": evidence_ids,
            "note": "Authenticated fixture review of the recorded executed checks; not a human approval claim."}


def close(work):
    contract = work["completion_contract"]
    return {"type": "close", "brief_sha256": contract["brief"]["sha256"],
            "artifact_set_sha256": contract["artifact_set"]["sha256"],
            "note": "The fixture requester accepts these exact delivered bytes for the frozen scope."}


def exercise(instance, report_dir, source_url):
    instance.api("GET", "work", actor=None, expected=401)
    for name in ["requester", "builder", "reviewer-one", "reviewer-two"]:
        instance.issue(name)
    assert len(set(instance.credentials.values())) == 5
    source_path = report_dir / "source.json"
    with urllib.request.urlopen(source_url, timeout=10) as response:
        assert response.read() == source_path.read_bytes()
    source = json.loads(source_path.read_text())
    work = instance.api("POST", "work", {"title": "Completion contract integration fixture",
        "goal": "Deliver the small Markdown artifact requested in the linked source.",
        "source": {"provider_id": "disposable-validation", "conversation_id": "request-room",
                   "message_id": "request-1", "url": source_url}})
    work = instance.command(work, {"type": "freeze_brief", "acceptance_criteria": source["criteria"],
        "reviewer_actors": [instance.principals["reviewer-one"], instance.principals["reviewer-two"]]})
    original_brief = work["completion_contract"]["brief"]
    work = instance.command(work, {"type": "accept"}, "builder")
    work = instance.command(work, {"type": "start"}, "builder")
    version_sets = []
    for version, text in [("1", "# Local release notes\n\nFirst checked fixture.\n"),
                          ("2", "# Local release notes\n\nRevised checked fixture.\n")]:
        artifact = report_dir / f"release-notes-v{version}.md"
        artifact.write_text(text)
        prior_close = close(work) if version_sets else None
        work = instance.command(work, {"type": "artifact_version", "artifact": "release-notes",
            "label": "Release notes version " + version, "uri": artifact.as_uri(), "version": version,
            "sha256": digest(artifact)}, "builder")
        version_sets.append(work["completion_contract"]["artifact_set"]["sha256"])
        if prior_close:
            instance.command(work, prior_close, expected=409)
            instance.command(work, close(work), expected=400)
            assert work["verification"] is None
        receipts = checks(report_dir, source_path, artifact, version)
        evidence_ids = []
        for receipt in receipts:
            work = instance.command(work, {"type": "evidence", "kind": "test",
                "label": receipt.stem, "uri": receipt.as_uri()}, "builder")
            evidence_ids.append(work["evidence"][-1]["id"])
        if work["state"] != "review":
            work = instance.command(work, {"type": "review"}, "builder")
        if version == "1":
            instance.command(work, review(work, evidence_ids), "builder", 403)
            instance.command(work, {"type": "complete"}, expected=400)
        work = instance.command(work, review(work, evidence_ids), "reviewer-one")
        instance.command(work, close(work), expected=400)
        if version == "2":
            work = instance.command(work, review(work, evidence_ids, "fail"), "reviewer-two")
            instance.command(work, close(work), expected=400)
        work = instance.command(work, review(work, evidence_ids), "reviewer-two")
        assert work["verification"]["outcome"] == "pass"
        assert work["verification"]["independently_verified"] is False
    assert version_sets[0] != version_sets[1]
    assert work["completion_contract"]["brief"] == original_brief
    assert all(item["content_verified_by_store"] is False for item in work["evidence"] if item["kind"] == "artifact")
    instance.command(work, close(work), "reviewer-one", 403)
    instance.command(work, {"type": "complete"}, expected=400)
    work = instance.command(work, close(work))
    assert work["state"] == "completed"
    assert work["completion_contract"]["closure"]["recorded_by"] == instance.principals["requester"]
    assert work["completion_contract"]["closure"]["human_identity_inferred"] is False
    instance.stop()
    instance.start()
    stored = instance.api("GET", "work/" + work["id"])
    assert stored == work
    save(report_dir / "work-completed.json", stored)
    return {"completed": True, "restart_persistence": True, "tls_certificate_verified": True,
        "distinct_scoped_principals": instance.principals, "source_retrieved": True,
        "frozen_brief_preserved": True, "artifact_versions_checked": 2,
        "executed_checks": {"subprocess_runs": 4, "assertions": 8, "input_hashes_stable": True},
        "rejected": ["unauthenticated access", "unnamed builder review", "missing peer review",
                     "failed peer review", "stale artifact digest", "non-requester closure", "legacy complete bypass"],
        "authenticated_review_reports": 5, "store_executed_checks": False,
        "human_participation_or_acceptance_tested": False, "model_called": False,
        "external_conversation_sent": False, "service_registered": False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--report-dir", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    report_dir = args.report_dir.resolve()
    report_dir.mkdir(mode=0o700, parents=True, exist_ok=False)
    source = {"title": "Local release notes", "criteria": [
        "The first line is the Markdown heading '# Local release notes'.", "The file ends with a newline."],
        "scope": "Deterministic disposable integration fixture; not a production delivery."}
    save(report_dir / "source.json", source)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(QuietFiles, directory=str(report_dir)))
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    instance = None
    report = {"schema": "agent-instance.completion-live/1", "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "platform": platform.platform(), "binary_sha256": digest(binary),
              "binary_version": subprocess.check_output([str(binary), "--version"], text=True).strip(),
              "validation_program_sha256": digest(Path(__file__)), "passed": False}
    try:
        with tempfile.TemporaryDirectory(prefix="agent-completion-live-") as temp:
            try:
                instance = Instance(binary, Path(temp) / "instance")
                report["checks"] = exercise(instance, report_dir, f"http://127.0.0.1:{server.server_port}/source.json")
                report["passed"] = True
            finally:
                if instance:
                    instance.stop()
                    instance.log.close()
                    save(report_dir / "api-receipts.json", instance.calls)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        report["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        save(report_dir / "result.json", report)
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
