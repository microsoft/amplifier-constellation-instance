"""Local owner controls for recovering outputs from the feasibility adapter."""

import argparse
import io
import os
from pathlib import Path

from adapter import Adapter


def main():
    os.umask(0o077)
    p = argparse.ArgumentParser()
    for arg in ("binary", "profile", "directory"):
        p.add_argument("--" + arg, type=Path, required=True)
    p.add_argument("--workspace-write", action="store_true")
    p.add_argument("--model", default="gpt-6.2-sol")
    p.add_argument("--session", required=True)
    p.add_argument("--sha256", required=True)
    cmd = p.add_mutually_exclusive_group(required=True)
    cmd.add_argument("--export-to", type=Path)
    cmd.add_argument("--preview-to", type=Path)
    cmd.add_argument("--accept-interrupted-operation", metavar="OPERATION_ID")
    args = p.parse_args()
    a = Adapter(
        args.binary,
        args.profile,
        args.directory,
        True,
        args.model,
        workspace_write=args.workspace_write,
    )
    try:
        if args.export_to:
            data = a.original(args.session, args.sha256)
            with args.export_to.open("xb") as f:
                f.write(data)
        elif args.preview_to:
            from PIL import Image

            data = a.original(args.session, args.sha256)
            with Image.open(io.BytesIO(data)) as im:
                if im.width * im.height > 32_000_000:
                    raise ValueError("Image too large to preview")
                im.thumbnail((512, 512))
                with args.preview_to.open("xb") as target:
                    im.convert("RGBA").save(target, format="PNG")
        else:
            a.accept_effect(
                args.session, args.accept_interrupted_operation, args.sha256
            )
    finally:
        a.close()


if __name__ == "__main__":
    main()
