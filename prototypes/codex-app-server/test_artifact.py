import tempfile
import unittest
from pathlib import Path
from uuid import uuid4

from adapter import read_generated_png


class SourceTests(unittest.TestCase):
    def test_regular_png_and_replaced_thread_directory(self):
        with tempfile.TemporaryDirectory() as tmp:
            profile = Path(tmp)
            sid = str(uuid4())
            folder = profile / "generated_images" / sid
            folder.mkdir(parents=True)
            png = folder / "asset.png"
            data = b"\x89PNG\r\n\x1a\noriginal"
            png.write_bytes(data)
            self.assertEqual(read_generated_png(profile, sid, png), data)
            moved = profile / "other"
            folder.rename(moved)
            folder.symlink_to(moved)
            with self.assertRaises(OSError):
                read_generated_png(profile, sid, png)
