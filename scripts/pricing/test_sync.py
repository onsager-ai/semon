"""Offline parser tests using trimmed official pricing-page fixtures."""

from __future__ import annotations

import json
from pathlib import Path
import sys
import unittest


SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))
import sync  # noqa: E402


FIXTURES = SCRIPT_DIR / "fixtures"
PRICING = SCRIPT_DIR.parents[1] / "crates/semon-sessions/src/pricing.json"


def expected_synced_rows() -> dict[str, dict[str, object]]:
    current = json.loads(PRICING.read_text())
    return {
        model: {field: row[field] for field in (*sync.PRICE_FIELDS, "legacy", "source")}
        for model, row in current["models"].items()
        if row.get("sync", True)
    }


class PricingParserTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.anthropic_html = (FIXTURES / "anthropic.html").read_text()
        cls.openai_html = (FIXTURES / "openai.html").read_text()
        cls.parsed = sync.parse_anthropic(cls.anthropic_html)
        cls.parsed.update(sync.parse_openai(cls.openai_html))

    def test_fixtures_parse_to_the_checked_in_price_rows(self) -> None:
        self.assertEqual(self.parsed, expected_synced_rows())

    def test_current_json_has_all_price_fields_and_sources(self) -> None:
        current = sync.load_pricing(PRICING)
        models = current["models"]
        self.assertGreaterEqual(len(models), 1)
        for model, row in models.items():
            with self.subTest(model=model):
                self.assertTrue(all(field in row for field in sync.PRICE_FIELDS))
                self.assertTrue(row["source"].startswith("https://"))
                self.assertEqual(set(row["legacy"]), set(sync.LEGACY_FIELDS))

    def test_page_without_a_pricing_section_is_rejected(self) -> None:
        with self.assertRaises(sync.PricingParseError):
            sync.parse_anthropic("<!doctype html><html><body><p>Page layout changed</p></body></html>")

    def test_openai_fast_and_web_search_rates_are_in_the_parsed_shape(self) -> None:
        self.assertEqual(self.parsed["gpt-6-luna"]["fast"], {"multiplier": 2.0})
        self.assertEqual(self.parsed["gpt-6-luna"]["web_search_per_1k"], 10.0)
        self.assertIsNone(self.parsed["gpt-6-luna"]["long_context"])

    def test_gpt_61_preserves_long_context_and_rejects_missing_rates(self) -> None:
        tier = self.parsed["gpt-6.1-sol"]["long_context"]
        self.assertEqual(tier, {"threshold_tokens": 272_000, "input": 4.0, "output": 15.0,
                                "cache_read": 0.2, "cache_write_5m": 5.0, "cache_write_1h": None})
        broken = self.openai_html.replace("<td>$0.10</td><td>$2.50</td><td>$10.00</td><td>$4.00</td>",
                                          "<td>$0.10</td><td>$2.50</td><td>$10.00</td><td>-</td>", 1)
        with self.assertRaises(sync.PricingParseError):
            sync.parse_openai(broken)


if __name__ == "__main__":
    unittest.main()
