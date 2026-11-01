#!/usr/bin/env python3
"""Bundle the private evaluation viewer into one account-private HTML file."""

from __future__ import annotations

import argparse
import base64
import hashlib
import os
import stat
import sys
from pathlib import Path

from real_pipeline import PipelineError, thermal_checkpoint


EXPERIMENT_ROOT = Path(__file__).resolve().parent
MAX_FILE_BYTES = 10_000_000
INPUT_FILES = ("index.html", "app.js", "style.css", "review.json", "results.json")
DEFAULT_INPUT = EXPERIMENT_ROOT / "private" / "viewer"
AGGREGATE_INPUT = EXPERIMENT_ROOT / "private" / "viewer-aggregate"
DEFAULT_OUTPUT = EXPERIMENT_ROOT / "private" / "cloud-viewer" / "index.html"


class BundleError(ValueError):
    """A safe, concise error for invalid bundle inputs or outputs."""


def _read_input(directory: Path, name: str) -> bytes:
    path = directory / name
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError:
        raise BundleError(f"input_file_invalid:{name}") from None
    try:
        info = os.fstat(descriptor)
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_size >= MAX_FILE_BYTES:
            raise BundleError(f"input_file_invalid:{name}")
        with os.fdopen(descriptor, "rb") as stream:
            descriptor = -1
            data = stream.read(MAX_FILE_BYTES)
        if len(data) >= MAX_FILE_BYTES:
            raise BundleError(f"input_file_too_large:{name}")
        return data
    finally:
        if descriptor >= 0:
            os.close(descriptor)


def _read_inputs(input_dir: Path) -> dict[str, str]:
    try:
        info = input_dir.lstat()
    except OSError:
        raise BundleError("input_directory_invalid") from None
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        raise BundleError("input_directory_invalid")
    files: dict[str, str] = {}
    for name in INPUT_FILES:
        try:
            files[name] = _read_input(input_dir, name).decode("utf-8")
        except UnicodeDecodeError:
            raise BundleError(f"input_file_not_utf8:{name}") from None
    return files


def _script_hash(source: str) -> str:
    digest = hashlib.sha256(source.encode("utf-8")).digest()
    return "'sha256-" + base64.b64encode(digest).decode("ascii") + "'"


def _safe_json_text(source: str) -> str:
    # Script elements are parsed as raw text, so escape HTML-sensitive codepoints.
    return source.replace("&", r"\u0026").replace("<", r"\u003c").replace(">", r"\u003e")


def _assemble(files: dict[str, str]) -> str:
    html = files["index.html"]
    style_tag = "<link rel=\"stylesheet\" href=\"./style.css\">"
    app_tag = "<script src=\"./app.js\" defer></script>"
    if html.count(style_tag) != 1 or html.count(app_tag) != 1:
        raise BundleError("viewer_asset_reference_invalid")
    html = html.replace(style_tag, "<style>\n" + files["style.css"] + "\n</style>", 1)
    html = html.replace(app_tag, "", 1)

    cloud_copy = (
        '<p class="intro-copy">Account-private blind review. Your review draft stays in this browser only. '
        "Download JSONL for a durable copy; answers do not sync to a server.</p>"
    )
    source_copy = (
        '<p class="intro-copy">A local, blind spot-check followed by a clear view of classifier quality, '
        "operating cost and limits.</p>"
    )
    if html.count(source_copy) != 1:
        raise BundleError("viewer_intro_invalid")
    html = html.replace(source_copy, cloud_copy, 1)

    bootstrap = """(() => {
  "use strict";
  const originalFetch = window.fetch.bind(window);
  const embedded = {
    "./review.json": "review-data",
    "./results.json": "results-data",
  };
  window.fetch = (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input?.url;
    const elementId = embedded[url];
    if (elementId) {
      const body = document.getElementById(elementId).textContent;
      return Promise.resolve(new Response(body, { headers: { "Content-Type": "application/json" } }));
    }
    return originalFetch(input, init);
  };
})();"""
    app_script = files["app.js"]
    bootstrap_content = "\n" + bootstrap + "\n"
    app_content = "\n" + app_script + "\n"
    script_src = " ".join((_script_hash(bootstrap_content), _script_hash(app_content)))
    csp = (
        "default-src 'none'; script-src " + script_src + "; style-src 'unsafe-inline'; "
        "connect-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'"
    )
    csp_tag = '<meta http-equiv="Content-Security-Policy" content="' + csp + '">'
    marker = '<meta http-equiv="Content-Security-Policy" content="default-src \'self\'; script-src \'self\'; style-src \'self\' \'unsafe-inline\'; connect-src \'self\'; font-src \'self\'; img-src \'self\' data:; object-src \'none\'; base-uri \'none\'; form-action \'none\'">'
    if html.count(marker) != 1:
        raise BundleError("viewer_csp_invalid")
    html = html.replace(marker, csp_tag, 1)

    data_tags = (
        '<script type="application/json" id="review-data">'
        + _safe_json_text(files["review.json"])
        + "</script>\n"
        + '<script type="application/json" id="results-data">'
        + _safe_json_text(files["results.json"])
        + "</script>\n"
    )
    inline_scripts = (
        "<script>" + bootstrap_content + "</script>\n"
        + "<script>" + app_content + "</script>\n"
    )
    if html.count("</body>") != 1:
        raise BundleError("viewer_body_invalid")
    html = html.replace("</body>", data_tags + inline_scripts + "</body>", 1)
    return html


def _write_output(path: Path, data: bytes) -> None:
    absolute = Path(os.path.abspath(path))
    current = Path(absolute.anchor)
    for part in absolute.parts[1:-1]:
        current = current / part
        try:
            info = current.lstat()
        except FileNotFoundError:
            continue
        except OSError:
            raise BundleError("output_path_invalid") from None
        if stat.S_ISLNK(info.st_mode):
            raise BundleError("output_path_symlink")
    try:
        absolute.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        parent_info = absolute.parent.lstat()
        if stat.S_ISLNK(parent_info.st_mode) or not stat.S_ISDIR(parent_info.st_mode):
            raise BundleError("output_directory_invalid")
        os.chmod(absolute.parent, 0o700)
    except OSError:
        raise BundleError("output_directory_unwritable") from None

    flags = os.O_WRONLY | os.O_CREAT | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(absolute, flags, 0o600)
    except OSError:
        raise BundleError("output_file_unwritable") from None
    try:
        info = os.fstat(descriptor)
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
            raise BundleError("output_file_invalid")
        os.fchmod(descriptor, 0o600)
        os.ftruncate(descriptor, 0)
        view = memoryview(data)
        while view:
            written = os.write(descriptor, view)
            view = view[written:]
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def bundle(input_dir: Path, output: Path) -> Path:
    """Inline the five viewer files and write a single private HTML artifact."""
    thermal_checkpoint()
    try:
        files = _read_inputs(Path(input_dir))
        document = _assemble(files)
        output_bytes = document.encode("utf-8")
        if len(output_bytes) >= MAX_FILE_BYTES:
            raise BundleError("bundle_too_large")
        result = Path(output)
        _write_output(result, output_bytes)
        return result
    finally:
        thermal_checkpoint()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--aggregate-only", action="store_true", help="build from public aggregates only")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT, help="private output path")
    args = parser.parse_args(argv)

    try:
        if args.output.resolve() != DEFAULT_OUTPUT.resolve():
            raise BundleError("output_must_be_dedicated_private_cloud_viewer")
        input_dir = DEFAULT_INPUT
        if args.aggregate_only:
            from local_viewer import ViewerError, build

            try:
                build(AGGREGATE_INPUT, aggregate_only=True)
            except (ViewerError, PipelineError, OSError) as exc:
                raise BundleError(f"aggregate_viewer_build_failed:{exc}") from None
            input_dir = AGGREGATE_INPUT
        bundle(input_dir, args.output)
    except (BundleError, PipelineError, OSError) as exc:
        print(f"cloud viewer bundle failed: {exc}", file=sys.stderr)
        return 2
    print(f"cloud viewer generated at {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
