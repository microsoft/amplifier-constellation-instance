"""Exercise a real ACP endpoint from a non-game host. Sends two subscription turns."""

import argparse
import json
import subprocess
from contextlib import contextmanager
from pathlib import Path
from uuid import uuid4


def call(peer, number, method, params):
    peer.stdin.write(
        json.dumps({"jsonrpc": "2.0", "id": number, "method": method, "params": params})
        + "\n"
    )
    peer.stdin.flush()
    messages = []
    for line in peer.stdout:
        event = json.loads(line)
        if event.get("method") == "session/update":
            messages.append(
                event["params"]["update"].get("content", {}).get("text", "")
            )
        if event.get("id") == number:
            if "error" in event:
                raise RuntimeError(event["error"]["message"])
            return event["result"], messages
    raise RuntimeError("ACP endpoint closed")


@contextmanager
def client(command):
    with subprocess.Popen(command, stdin=-1, stdout=-1, text=True) as peer:
        try:
            call(
                peer,
                1,
                "initialize",
                {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "clientInfo": {"name": "print-study", "version": "0.1"},
                },
            )
            yield peer
        finally:
            peer.terminate()


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--cwd", required=True, type=Path)
    p.add_argument(
        "command", nargs=argparse.REMAINDER, help="ACP client command following --"
    )
    args = p.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    params = {"cwd": str(args.cwd.resolve(strict=True)), "mcpServers": []}

    def prompt(peer, sid, op, text):
        return call(
            peer,
            3,
            "session/prompt",
            {
                "sessionId": sid,
                "_meta": {"operationId": op},
                "prompt": [{"type": "text", "text": text}],
            },
        )

    with client(command) as peer:
        session, _ = call(peer, 2, "session/new", params)
        sid = session["sessionId"]
        result, _ = prompt(
            peer,
            sid,
            str(uuid4()),
            "Remember the studio label LARCH-47. Generate one blue-heron woodcut using the built-in image tool. Keep the source PNG.",
        )
        (original,) = result["_meta"]["artifacts"]
    op = str(uuid4())
    edit = "Use the original saved PNG as a reference: turn the blue heron vermilion. Make exactly one edit and repeat the earlier studio label."
    with client(command) as peer:
        call(peer, 2, "session/load", {**params, "sessionId": sid})
        prompt(peer, sid, op, edit)  # Model a host that loses this acknowledgment.
    with client(command) as peer:
        call(peer, 2, "session/load", {**params, "sessionId": sid})
        result, replay = prompt(peer, sid, op, edit)
        (revision,) = result["_meta"]["artifacts"]
        assert original != revision and "LARCH-47" in "".join(replay)
    print(
        json.dumps(
            {"session": sid, "original": original, "revision": revision}, indent=2
        )
    )


if __name__ == "__main__":
    main()
