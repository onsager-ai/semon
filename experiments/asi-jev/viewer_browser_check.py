#!/usr/bin/env python3
"""CI browser checks using public aggregates and explicitly synthetic events only."""

from __future__ import annotations

import functools
import json
import shutil
import tempfile
import threading
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from unittest.mock import patch

from playwright.sync_api import sync_playwright

import local_viewer


class QuietHandler(SimpleHTTPRequestHandler):
    def log_message(self, *_args: object) -> None:
        pass


def main() -> None:
    with tempfile.TemporaryDirectory() as temporary:
        output = Path(temporary) / "site"
        # CI has no host Tctl hardware. Only this test mocks the measurement;
        # the shipped generator always uses the real thermal checkpoint.
        with patch.object(local_viewer, "thermal_checkpoint", return_value=35_000):
            local_viewer.build(output, aggregate_only=True)
        aggregate = output / "aggregate"
        aggregate.mkdir()
        for name in ("index.html", "app.js", "style.css", "review.json", "results.json"):
            shutil.copyfile(output / name, aggregate / name)
        results = json.loads((output / "results.json").read_text())
        reference = dict(activity="tool_action", failure_category="none",
                         retry_loop="none", anomaly=False, phase="active")
        malicious = '<script>window.untrustedMarkupRan=true</script> SYNTHETIC ONLY'
        review = {
            "schema": 1, "aggregate_only": False, "packet_id": "browser-synthetic",
            "events": [dict(id=f"synthetic-{i}", harness="codex", context=malicious,
                            reference=reference) for i in range(50)],
        }
        predictions = {c["id"]: [{"repetition": 1, "labels": dict(reference, phase="blocked")}]
                       for c in results["conditions"]}
        results["sessions"] = [{"key": "Synthetic session", "harness": "codex", "events": [
            dict(id=f"synthetic-{i}", order=i, context=malicious, reference=reference,
                 causal_reference=reference, predictions=predictions) for i in range(2)
        ]}]
        results["confusion"] = {c["id"]: {"activity": {
            "labels": ["failure", "tool_action", "<abstain>"],
            "row_labels": ["tool_action", "failure"],
            "matrix": [[1 / 3, 2 / 3, 0], [0, 0, 0]],
        }} for c in results["conditions"]}
        (output / "review.json").write_text(json.dumps(review))
        (output / "results.json").write_text(json.dumps(results))
        handler = functools.partial(QuietHandler, directory=str(output))
        server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        origin = f"http://127.0.0.1:{server.server_port}"
        try:
            with sync_playwright() as playwright:
                browser = playwright.chromium.launch()
                aggregate_page = browser.new_page()
                aggregate_requests: list[str] = []
                aggregate_page.on("request", lambda request: aggregate_requests.append(request.url))
                aggregate_page.goto(origin + "/aggregate/")
                aggregate_page.locator("#aggregate-review").wait_for(state="visible")
                assert not any("results.json" in url for url in aggregate_requests)
                assert aggregate_page.locator("#aggregate-reveal").is_disabled()
                aggregate_page.locator("#aggregate-reveal-ack").check()
                aggregate_page.locator("#aggregate-reveal").click()
                aggregate_page.locator("#results-app").wait_for(state="visible")
                assert any("results.json" in url for url in aggregate_requests)
                aggregate_page.close()
                page = browser.new_page(viewport={"width": 1365, "height": 1000})
                requests: list[str] = []
                errors: list[str] = []
                page.on("request", lambda request: requests.append(request.url))
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.goto(origin)
                page.locator("#review-panel").wait_for(state="visible")
                assert page.locator("#spot-context").inner_text() == malicious
                assert page.evaluate("window.untrustedMarkupRan") is None
                assert not any("results.json" in url for url in requests)
                assert not page.locator("#results-app").is_visible()
                assert page.locator("#reveal-results").is_disabled()
                for index in range(50):
                    page.locator("#decision-options").get_by_text("Agree", exact=True).click()
                    if index < 49:
                        assert page.locator("#reveal-results").is_disabled()
                        page.locator("#next-event").click()
                page.locator("#spot-note").fill("Synthetic browser check only")
                with page.expect_download() as download_info:
                    page.locator("#download-review").click()
                saved = Path(temporary) / "answers.jsonl"
                download_info.value.save_as(saved)
                answers = [json.loads(line) for line in saved.read_text().splitlines()]
                assert len(answers) == 50
                assert all(row["decision"] == "agree" for row in answers)
                assert all(row["packet_id"] == "browser-synthetic" for row in answers)
                assert all(row["review_stage"] == "blind" for row in answers)
                assert all("predictions" not in row for row in answers)
                page.locator("#saved-file-confirm").check()
                page.locator("#reveal-ack").check()
                assert not any("results.json" in url for url in requests)
                page.locator("#reveal-results").click()
                page.locator("#results-app").wait_for(state="visible")
                assert any("results.json" in url for url in requests)
                assert page.locator("#results-nav button").count() == 7
                for button in page.locator("#results-nav button").all():
                    button.click()
                    assert page.locator(".result-view:visible").inner_text().strip()
                page.locator('#results-nav [data-view="classes"]').click()
                assert page.locator(".heatmap-row .is-label").all_text_contents() == [
                    "Reference", "tool action", "failure",
                ]
                assert "0.33" in page.locator(".heatmap-grid").inner_text()
                page.locator('#results-nav [data-view="failures"]').click()
                page.locator("#failure-condition").select_option("jev-prefix")
                page.locator("#failure-dimension").select_option("phase")
                assert page.locator(".failure-row").count() == 2
                page.locator(".failure-open").first.click()
                assert page.locator("#event-dialog").is_visible()
                assert malicious in page.locator("#dialog-body").inner_text()
                page.locator("#dialog-close").click()
                page.locator('#results-nav [data-view="methodology"]').click()
                for row in page.locator("#view-methodology tbody tr").all():
                    assert all(value not in ("Unavailable", "—")
                               for value in row.locator("td").all_text_contents()[-2:])
                # A small screen must not force the entire page horizontally.
                page.set_viewport_size({"width": 390, "height": 844})
                assert page.evaluate("document.documentElement.scrollWidth <= innerWidth + 2")
                assert all(url.startswith(origin + "/") for url in requests), requests
                assert not errors, errors
                browser.close()
        finally:
            server.shutdown()
            server.server_close()
    print("Viewer browser checks passed: blind loading, local export, safe text, seven views, mobile, no external requests.")


if __name__ == "__main__":
    main()
