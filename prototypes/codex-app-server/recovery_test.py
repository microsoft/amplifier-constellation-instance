"""Destructive only to its own test subprocess. Use a disposable image-enabled test chat."""

import argparse
import os
from pathlib import Path
from uuid import UUID

from adapter import Adapter


def main():
    p = argparse.ArgumentParser()
    p.add_argument("phase", choices=["interrupt", "before-receipt", "recover"])
    for name in ("binary", "profile", "directory", "cwd"):
        p.add_argument("--" + name, required=True, type=Path)
    p.add_argument("--session", required=True)
    p.add_argument("--operation", required=True, type=UUID)
    p.add_argument(
        "--prompt",
        required=True,
        help="Use the identical reference-edit request for both phases.",
    )
    args = p.parse_args()
    a = Adapter(args.binary, args.profile, args.directory, True, "gpt-6.2-sol")
    try:
        a.resume(args.session, str(args.cwd.resolve(strict=True)))
        if args.phase != "recover":

            def disconnect(receipt):
                print(receipt, flush=True)
                a.server.kill()
                a.server.wait()
                os._exit(75)

            if args.phase == "before-receipt":

                def before_save(sid, op, item):
                    if item.get("status") != "completed":
                        return
                    assert Path(item["savedPath"]).is_file()
                    assert not a.artifacts(sid, op)
                    disconnect("Cut before first adapter image receipt")

                a.save_image = before_save
            a.prompt(args.session, str(args.operation), args.prompt, disconnect)
            raise AssertionError("No image receipt to interrupt")
        rpc = a.rpc

        def inspect(method, params):
            if method == "turn/start":
                raise AssertionError("Recovery attempted to repeat inference")
            return rpc(method, params)

        a.rpc = inspect
        try:
            a.prompt(args.session, str(args.operation), args.prompt, print)
        except ValueError as exc:
            if "outcome uncertain" not in str(exc):
                raise
        (sha,) = a.artifacts(args.session, str(args.operation))
        original = a.original(args.session, sha)
        files = (args.profile / "generated_images" / args.session).glob("*.png")
        assert any(file.read_bytes() == original for file in files), (
            "Original bytes changed"
        )
        print("Recovered", sha, len(original), "bytes")
    finally:
        a.close()


if __name__ == "__main__":
    main()
