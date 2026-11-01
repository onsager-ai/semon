from __future__ import annotations

import base64
import hashlib
import json
import re
import tempfile
import unittest
from html.parser import HTMLParser
from pathlib import Path
from unittest.mock import patch

import cloud_viewer


class _Scripts(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.scripts: list[tuple[dict[str, str | None], str]] = []
        self._attrs: dict[str, str | None] | None = None
        self._parts: list[str] = []
        self.meta_csp = ""

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        values = dict(attrs)
        if tag == "script":
            self._attrs = values
            self._parts = []
        if tag == "meta" and values.get("http-equiv") == "Content-Security-Policy":
            self.meta_csp = values.get("content") or ""

    def handle_data(self, data: str) -> None:
        if self._attrs is not None:
            self._parts.append(data)

    def handle_endtag(self, tag: str) -> None:
        if tag == "script" and self._attrs is not None:
            self.scripts.append((self._attrs, "".join(self._parts)))
            self._attrs = None
            self._parts = []


class CloudViewerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.input_dir = self.root / "input"
        self.input_dir.mkdir()
        self.output = self.root / "private" / "cloud-viewer" / "index.html"
        for name, content in {
            "index.html": (Path(cloud_viewer.EXPERIMENT_ROOT / "viewer" / "index.html").read_text(encoding="utf-8")),
            "app.js": "window.viewerAppLoaded = true;\n",
            "style.css": "body { color: #123; }\n",
            "review.json": '{"text":"</script><script>window.pwned=true</script>","symbols":"<>&"}',
            "results.json": '{"result":{"value":7}}',
        }.items():
            (self.input_dir / name).write_text(content, encoding="utf-8")

    def tearDown(self) -> None:
        self.temp.cleanup()

    def _bundle(self) -> str:
        with patch.object(cloud_viewer, "thermal_checkpoint", return_value=40_000):
            cloud_viewer.bundle(self.input_dir, self.output)
        return self.output.read_text(encoding="utf-8")

    def test_json_data_round_trips_without_closing_script_injection(self) -> None:
        document = self._bundle()
        parser = _Scripts()
        parser.feed(document)
        data = {attrs["id"]: json.loads(body) for attrs, body in parser.scripts if attrs.get("type") == "application/json"}
        self.assertEqual(data["review-data"], {"text": "</script><script>window.pwned=true</script>", "symbols": "<>&"})
        self.assertEqual(data["results-data"], {"result": {"value": 7}})
        self.assertNotIn("</script><script>window.pwned=true", document)
        self.assertIn("Account-private blind review.", document)
        self.assertIn("Download JSONL for a durable copy; answers do not sync to a server.", document)

    def test_inline_script_hashes_match_csp(self) -> None:
        document = self._bundle()
        parser = _Scripts()
        parser.feed(document)
        script_hashes = {
            "'sha256-" + base64.b64encode(hashlib.sha256(body.encode("utf-8")).digest()).decode("ascii") + "'"
            for attrs, body in parser.scripts
            if attrs.get("type") != "application/json"
        }
        csp_script_src = re.search(r"(?:^|; )script-src ([^;]+)", parser.meta_csp)
        self.assertIsNotNone(csp_script_src)
        self.assertEqual(set(csp_script_src.group(1).split()), script_hashes)
        self.assertIn("connect-src 'none'", parser.meta_csp)

    def test_cli_rejects_output_outside_dedicated_private_file(self) -> None:
        with patch.object(cloud_viewer, "thermal_checkpoint", return_value=40_000):
            self.assertEqual(cloud_viewer.main(["--output", str(self.root / "elsewhere.html")]), 2)
        self.assertFalse((self.root / "elsewhere.html").exists())

    def test_output_permissions_are_private(self) -> None:
        self._bundle()
        self.assertEqual(self.output.stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.output.parent.stat().st_mode & 0o777, 0o700)


if __name__ == "__main__":
    unittest.main()
