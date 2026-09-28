import base64
import contextlib
import io
import os
import tempfile
import unittest
from pathlib import Path

from core import cli, image

# 1x1 transparent PNG
PNG_1PX = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=")


@unittest.skipUnless(os.name == "nt", "uses Windows PowerShell System.Drawing")
class ImageTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.tmp = Path(self.dir.name)

    def tearDown(self):
        self.dir.cleanup()

    def big_png(self) -> Path:
        # enlarge the 1px PNG to 3000x1500 with the same System.Drawing path the module uses
        source = self.tmp / "one.png"
        source.write_bytes(PNG_1PX)
        return image.resize(source, self.tmp / "big.png", width=3000, height=1500)

    def test_shrinks_the_long_edge_and_keeps_aspect(self):
        out = image.downscale(self.big_png(), max_edge=1024, cache_dir=self.tmp / "cache")
        self.assertEqual(image.size(out), (1024, 512))

    def test_small_images_are_returned_unchanged(self):
        source = self.tmp / "one.png"
        source.write_bytes(PNG_1PX)
        self.assertEqual(image.downscale(source, max_edge=1024, cache_dir=self.tmp / "cache"), source)

    def test_cli_prints_the_path_to_read(self):
        big = self.big_png()
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = cli.main(["image", str(big), "--max-edge", "800", "--cache-dir", str(self.tmp / "c")])
        self.assertEqual(code, 0)
        self.assertEqual(image.size(Path(out.getvalue().strip())), (800, 400))

    def test_non_images_are_rejected(self):
        text = self.tmp / "a.txt"
        text.write_text("x", encoding="utf-8")
        with self.assertRaises(ValueError):
            image.downscale(text, max_edge=1024, cache_dir=self.tmp / "cache")


if __name__ == "__main__":
    unittest.main()
