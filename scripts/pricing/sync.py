#!/usr/bin/env python3
"""Read the official model pricing tables and update pricing.json safely."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
from html.parser import HTMLParser
import json
import math
from pathlib import Path
import re
import sys
from urllib.error import URLError
from urllib.request import Request, urlopen


ROOT = Path(__file__).resolve().parents[2]
PRICING_PATH = ROOT / "crates/semon-sessions/src/pricing.json"
ANTHROPIC_URL = "https://platform.claude.com/docs/en/about-claude/pricing"
OPENAI_URL = "https://developers.openai.com/api/docs/pricing"
PRICE_FIELDS = (
    "input",
    "output",
    "cache_read",
    "cache_write_5m",
    "cache_write_1h",
    "long_context",
    "fast",
    "web_search_per_1k",
)
LEGACY_FIELDS = ("input", "output", "cache_write", "cache_read")

# The pages show display names rather than API ids for Anthropic models. Keep
# known ids stable (including dated API ids) and derive ids for new model names
# from the linked model slug when the page publishes one.
ANTHROPIC_IDS = {
    "claude fable 5.1": "claude-fable-5-1",
    "claude opus 5.5": "claude-opus-5-5",
    "claude sonnet 5.5": "claude-sonnet-5-5",
    "claude sonnet 5": "claude-sonnet-5",
    "claude haiku 4.5": "claude-haiku-4-5-20251001",
    "claude mythos 5.1": "claude-mythos-5-1",
    "claude fable 5": "claude-fable-5",
    "claude mythos 5": "claude-mythos-5",
    "claude opus 5": "claude-opus-5",
    "claude opus 4.8": "claude-opus-4-8",
    "claude opus 4.7": "claude-opus-4-7",
    "claude opus 4.6": "claude-opus-4-6",
    "claude opus 4.5": "claude-opus-4-5-20251101",
    "claude opus 4.1": "claude-opus-4-1-20250805",
    "claude opus 4": "claude-opus-4-20250514",
    "claude sonnet 4.6": "claude-sonnet-4-6",
    "claude sonnet 4.5": "claude-sonnet-4-5-20250929",
    "claude sonnet 4": "claude-sonnet-4-20250514",
    "claude haiku 3.5": "claude-3-5-haiku-20241022",
    "claude 3.5 haiku": "claude-3-5-haiku-20241022",
}


class PricingParseError(ValueError):
    """The fetched page did not contain all of the pricing data we expect."""


class Table:
    def __init__(self, rows: list[list[str]], links: list[list[list[tuple[str, str]]]], headings: tuple[str, ...], pane: str | None):
        self.rows = rows
        self.links = links
        self.headings = headings
        self.pane = pane


class PricingHTMLParser(HTMLParser):
    """Collect table cells, heading context, links, and pricing switcher panes."""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.tables: list[Table] = []
        self.stack: list[tuple[str, dict[str, str | None]]] = []
        self.headings: list[str] = []
        self.heading_tag: str | None = None
        self.heading_depth = 0
        self.heading_text: list[str] = []
        self.table_rows: list[list[str]] | None = None
        self.table_links: list[list[list[tuple[str, str]]]] | None = None
        self.row: list[str] | None = None
        self.row_links: list[list[tuple[str, str]]] | None = None
        self.cell_text: list[str] | None = None
        self.cell_links: list[tuple[str, str]] | None = None
        self.link_depth = 0
        self.link_text: list[str] = []
        self.link_href = ""
        self.table_headings: tuple[str, ...] = ()
        self.table_pane: str | None = None

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        attributes = dict(attrs)
        if self.heading_tag is not None:
            self.heading_depth += 1
        if tag in ("h1", "h2", "h3", "h4", "h5", "h6"):
            self.heading_tag = tag
            self.heading_depth = 1
            self.heading_text = []
        if tag == "table":
            self.table_rows = []
            self.table_links = []
            self.table_headings = tuple(self.headings)
            self.table_pane = next(
                (
                    str(parent_attrs["data-value"])
                    for _, parent_attrs in reversed(self.stack)
                    if parent_attrs.get("data-content-switcher-pane") is not None
                    and parent_attrs.get("data-value") is not None
                ),
                None,
            )
        elif self.table_rows is not None and tag == "tr":
            self.row = []
            self.row_links = []
        elif self.table_rows is not None and tag in ("th", "td"):
            self.cell_text = []
            self.cell_links = []
        elif self.cell_text is not None and tag == "a":
            self.link_depth += 1
            self.link_text = []
            self.link_href = str(attributes.get("href") or "")
        self.stack.append((tag, attributes))

    def handle_data(self, data: str) -> None:
        if self.heading_tag is not None:
            self.heading_text.append(data)
        if self.cell_text is not None:
            self.cell_text.append(data)
        if self.link_depth:
            self.link_text.append(data)

    def handle_endtag(self, tag: str) -> None:
        if self.cell_text is not None and tag in ("th", "td"):
            assert self.row is not None and self.row_links is not None
            self.row.append(" ".join(" ".join(self.cell_text).split()))
            self.row_links.append(self.cell_links or [])
            self.cell_text = None
            self.cell_links = None
        elif self.table_rows is not None and tag == "tr" and self.row is not None:
            assert self.table_links is not None
            self.table_rows.append(self.row)
            self.table_links.append(self.row_links or [])
            self.row = None
            self.row_links = None
        elif self.table_rows is not None and tag == "table":
            assert self.table_links is not None
            self.tables.append(
                Table(self.table_rows, self.table_links, self.table_headings, self.table_pane)
            )
            self.table_rows = None
            self.table_links = None
        if self.link_depth and tag == "a":
            if self.cell_links is not None:
                label = " ".join(" ".join(self.link_text).split())
                self.cell_links.append((label, self.link_href))
            self.link_depth -= 1
        if self.heading_tag is not None:
            self.heading_depth -= 1
            if self.heading_depth == 0:
                title = " ".join(" ".join(self.heading_text).split())
                if title:
                    self.headings.append(title)
                self.heading_tag = None
        for index in range(len(self.stack) - 1, -1, -1):
            if self.stack[index][0] == tag:
                del self.stack[index:]
                break


class PageTextParser(HTMLParser):
    """Extract visible text while ignoring script and style contents."""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.skip_depth = 0
        self.parts: list[str] = []

    def handle_starttag(self, tag: str, _attrs: list[tuple[str, str | None]]) -> None:
        if tag in ("script", "style"):
            self.skip_depth += 1

    def handle_endtag(self, tag: str) -> None:
        if tag in ("script", "style") and self.skip_depth:
            self.skip_depth -= 1

    def handle_data(self, data: str) -> None:
        if not self.skip_depth and data.strip():
            self.parts.append(" ".join(data.split()))


def _page(html: str, page_name: str) -> tuple[list[Table], str]:
    try:
        parser = PricingHTMLParser()
        parser.feed(html)
        parser.close()
        text_parser = PageTextParser()
        text_parser.feed(html)
        text_parser.close()
    except Exception as error:
        raise PricingParseError(f"{page_name}: malformed HTML: {error}") from error
    return parser.tables, " ".join(text_parser.parts)


def _model_display(table: Table, row_index: int) -> str:
    if row_index < len(table.links) and table.links[row_index]:
        first_cell = table.links[row_index][0]
        if first_cell and first_cell[0][0]:
            return first_cell[0][0]
    return table.rows[row_index][0]


def _anthropic_id(display: str, link: str = "") -> str:
    clean = " ".join(display.split()).strip()
    key = clean.casefold()
    if key in ANTHROPIC_IDS:
        return ANTHROPIC_IDS[key]
    for known, model_id in sorted(ANTHROPIC_IDS.items(), key=lambda item: len(item[0]), reverse=True):
        if key.startswith(known + " "):
            return model_id
    slug = re.search(r"/models/([^/?#]+)/", link)
    if slug:
        return "claude-" + slug.group(1).strip("-").lower()
    # New rows have a display name followed by a sentence fragment. Model
    # names use a family/version prefix; this keeps the version and drops the
    # descriptive text when an official model URL is absent.
    match = re.match(r"^(Claude\s+(?:\d+(?:\.\d+)?\s+)?[A-Za-z][A-Za-z0-9.-]*(?:\s+\d+(?:\.\d+)?)?)", clean)
    if not match:
        raise PricingParseError(f"Anthropic: cannot identify model name {display!r}")
    slug_text = match.group(1).lower().replace(".", "-")
    return re.sub(r"[^a-z0-9]+", "-", slug_text).strip("-")


def _amount(value: str, *, optional: bool = False, context: str) -> float | None:
    normalized = value.strip().replace(",", "")
    if optional and normalized in ("", "-", "—", "n/a", "not available"):
        return None
    match = re.search(r"\$?\s*([0-9]+(?:\.[0-9]+)?)", normalized)
    if not match:
        if optional:
            return None
        raise PricingParseError(f"{context}: missing numeric price in {value!r}")
    price = float(match.group(1))
    if not math.isfinite(price):
        raise PricingParseError(f"{context}: non-finite price in {value!r}")
    return price


def _required_price(row: list[str], index: int, context: str) -> float:
    if index >= len(row):
        raise PricingParseError(f"{context}: row has only {len(row)} columns")
    value = _amount(row[index], context=context)
    assert value is not None
    return value


def _base_row(
    *,
    input_price: float,
    output_price: float,
    cache_read: float,
    cache_write_5m: float,
    cache_write_1h: float | None,
    fast: dict[str, float] | None,
    web_search_per_1k: float,
    source: str,
) -> dict[str, object]:
    return {
        "input": input_price,
        "output": output_price,
        "cache_read": cache_read,
        "cache_write_5m": cache_write_5m,
        "cache_write_1h": cache_write_1h,
        "long_context": None,
        "fast": fast,
        "web_search_per_1k": web_search_per_1k,
        "legacy": {
            "input": input_price,
            "output": output_price,
            "cache_write": cache_write_5m,
            "cache_read": cache_read,
        },
        "source": source,
    }


def _table_header(table: Table) -> list[str]:
    for row in table.rows:
        if row and row[0].strip().casefold() == "model":
            return [cell.strip().casefold() for cell in row]
    return []


def _anthropic_model_table(table: Table) -> bool:
    rows = [[cell.strip().casefold() for cell in row] for row in table.rows[:2]]
    return (
        len(rows) >= 2
        and rows[0][:3] == ["model", "base tokens", "prompt caching"]
        and rows[1][:6] == ["name", "input", "output", "5m writes", "1h writes", "hits and refreshes"]
    )


def _ratio(numerator: float, denominator: float, context: str) -> float:
    if denominator == 0:
        raise PricingParseError(f"{context}: cannot calculate a multiplier from a zero base price")
    value = numerator / denominator
    if not math.isfinite(value):
        raise PricingParseError(f"{context}: multiplier is not finite")
    return round(value, 12)


def parse_anthropic(html: str) -> dict[str, dict[str, object]]:
    """Parse Anthropic's standard and fast-mode pricing sections."""
    tables, page_text = _page(html, "Anthropic")
    model_tables = [table for table in tables if _anthropic_model_table(table)]
    if len(model_tables) != 1:
        raise PricingParseError(
            f"Anthropic: expected one model-pricing table, found {len(model_tables)}"
        )
    fast_tables = [
        table
        for table in tables
        if any(heading.startswith("Fast mode pricing") for heading in table.headings)
        and [cell.strip().casefold() for cell in table.rows[0]] == ["model", "input", "output"]
    ]
    if len(fast_tables) != 1:
        raise PricingParseError(f"Anthropic: expected one fast-mode pricing table, found {len(fast_tables)}")

    web = re.search(
        r"web search is available on the claude api for\s*\$?\s*([0-9]+(?:\.[0-9]+)?)\s*per\s*1,000 searches",
        page_text,
        re.IGNORECASE,
    )
    if not web:
        raise PricingParseError("Anthropic: web-search per-1,000 price was not found")
    web_search_per_1k = float(web.group(1))

    standard: dict[str, dict[str, object]] = {}
    standard_prices: dict[str, tuple[float, float]] = {}
    additional_seen = False
    primary_count = 0
    additional_count = 0
    for row_index, row in enumerate(model_tables[0].rows[2:], start=2):
        joined = " ".join(row).strip()
        if not joined:
            continue
        if len(row) == 1 and "additional models" in row[0].casefold():
            additional_seen = True
            continue
        if len(row) < 6:
            raise PricingParseError(f"Anthropic: incomplete model-pricing row {row!r}")
        display = _model_display(model_tables[0], row_index)
        link = ""
        if row_index < len(model_tables[0].links) and model_tables[0].links[row_index]:
            links = model_tables[0].links[row_index][0]
            if links:
                link = links[0][1]
        model_id = _anthropic_id(display, link)
        context = f"Anthropic model {model_id}"
        input_price = _required_price(row, 1, context)
        output_price = _required_price(row, 2, context)
        write_5m = _required_price(row, 3, context)
        write_1h = _required_price(row, 4, context)
        cache_read = _required_price(row, 5, context)
        if model_id in standard:
            raise PricingParseError(f"Anthropic: duplicate model id {model_id!r}")
        if additional_seen:
            additional_count += 1
        else:
            primary_count += 1
        standard[model_id] = _base_row(
            input_price=input_price,
            output_price=output_price,
            cache_read=cache_read,
            cache_write_5m=write_5m,
            cache_write_1h=write_1h,
            fast=None,
            web_search_per_1k=web_search_per_1k,
            source=ANTHROPIC_URL,
        )
        standard_prices[model_id] = (input_price, output_price)
    if not primary_count or not additional_count:
        raise PricingParseError("Anthropic: model table did not include both primary and additional model rows")

    fast_prices: dict[str, tuple[float, float]] = {}
    fast_table = fast_tables[0]
    for row_index, row in enumerate(fast_table.rows[1:], start=1):
        if not row or not row[0].strip():
            continue
        # Compound rows list multiple model names in one first cell. Use its
        # complete text instead of the first model link's label.
        display_parts = [part.strip() for part in re.split(r"\s*/\s*", row[0])]
        context = f"Anthropic fast-mode row {row[0]!r}"
        fast_input = _required_price(row, 1, context)
        fast_output = _required_price(row, 2, context)
        for display in display_parts:
            model_id = _anthropic_id(display)
            fast_prices[model_id] = (fast_input, fast_output)

    for model_id, (fast_input, fast_output) in fast_prices.items():
        if model_id not in standard:
            raise PricingParseError(f"Anthropic: fast-mode price has no standard model row for {model_id!r}")
        base_input, base_output = standard_prices[model_id]
        input_multiplier = _ratio(fast_input, base_input, f"Anthropic fast mode {model_id}")
        output_multiplier = _ratio(fast_output, base_output, f"Anthropic fast mode {model_id}")
        if input_multiplier != output_multiplier:
            raise PricingParseError(f"Anthropic: fast-mode input and output multipliers disagree for {model_id}")
        standard[model_id]["fast"] = {"multiplier": input_multiplier}

    standard_speed = re.search(
        r"claude opus 4\.6\s*\(requests run at standard speed and are billed at standard rates\)",
        page_text,
        re.IGNORECASE,
    )
    if standard_speed:
        model_id = "claude-opus-4-6"
        if model_id not in standard:
            raise PricingParseError("Anthropic: Opus 4.6 fast-mode note has no standard price row")
        standard[model_id]["fast"] = {"multiplier": 1.0}
    return standard


def _openai_pricing_table(table: Table) -> bool:
    header = _table_header(table)
    return (
        len(header) >= 9
        and header[:9]
        == ["model", "input", "cached input", "cache writes", "output", "input", "cached input", "cache writes", "output"]
    )


def _openai_rows(table: Table, *, page_tier: str) -> dict[str, tuple[float, float, float, float]]:
    output: dict[str, tuple[float, float, float, float]] = {}
    header_index = next(
        (index for index, row in enumerate(table.rows) if row and row[0].strip().casefold() == "model"),
        None,
    )
    if header_index is None:
        raise PricingParseError(f"OpenAI {page_tier}: model header row is missing")
    for row in table.rows[header_index + 1 :]:
        if not row or not row[0].strip():
            continue
        model_id = row[0].strip().lower()
        context = f"OpenAI {page_tier} model {model_id}"
        if len(row) < 5:
            raise PricingParseError(f"{context}: incomplete short-context row {row!r}")
        input_price = _required_price(row, 1, context)
        cache_read = _required_price(row, 2, context)
        cache_write = _required_price(row, 3, context)
        output_price = _required_price(row, 4, context)
        if model_id in output:
            raise PricingParseError(f"OpenAI {page_tier}: duplicate model id {model_id!r}")
        output[model_id] = (input_price, output_price, cache_read, cache_write)
    if not output:
        raise PricingParseError(f"OpenAI {page_tier}: no model rows were parsed")
    return output


def parse_openai(html: str) -> dict[str, dict[str, object]]:
    """Parse standard and fast model prices plus the web-search rate."""
    tables, _page_text_value = _page(html, "OpenAI")
    standard_tables = [table for table in tables if table.pane == "standard" and _openai_pricing_table(table)]
    fast_tables = [table for table in tables if table.pane == "fast" and _openai_pricing_table(table)]
    standalone_tables = [table for table in tables if table.pane is None and _openai_pricing_table(table)]
    cyber_tables = [table for table in standalone_tables if any(h.startswith("Cyber models") for h in table.headings)]
    if len(standard_tables) != 1:
        raise PricingParseError(f"OpenAI: expected one standard flagship table, found {len(standard_tables)}")
    if len(fast_tables) != 1:
        raise PricingParseError(f"OpenAI: expected one fast-mode flagship table, found {len(fast_tables)}")
    if len(cyber_tables) != 1:
        raise PricingParseError(f"OpenAI: expected one Cyber models standard table, found {len(cyber_tables)}")

    standard = _openai_rows(standard_tables[0], page_tier="standard")
    standalone_rows: dict[str, tuple[float, float, float, float]] = {}
    for table in standalone_tables:
        rows = _openai_rows(table, page_tier="standard " + (table.headings[-1] if table.headings else "models"))
        overlap = standalone_rows.keys() & rows.keys()
        if overlap:
            raise PricingParseError(f"OpenAI: duplicate standard model ids: {', '.join(sorted(overlap))}")
        standalone_rows.update(rows)
    overlap = standard.keys() & standalone_rows.keys()
    if overlap:
        raise PricingParseError(f"OpenAI: duplicate standard model ids: {', '.join(sorted(overlap))}")
    standard.update(standalone_rows)
    fast = _openai_rows(fast_tables[0], page_tier="fast")
    fast_without_standard = fast.keys() - standard.keys()
    if fast_without_standard:
        raise PricingParseError(
            "OpenAI: fast-mode models have no standard rows: "
            + ", ".join(sorted(fast_without_standard))
        )

    search_tables = [
        table
        for table in tables
        if "Tools" in table.headings
        and any(cell.strip().casefold() == "pricing" for row in table.rows[:1] for cell in row)
    ]
    if len(search_tables) != 1:
        raise PricingParseError(f"OpenAI: expected one tools pricing table, found {len(search_tables)}")
    web_search_per_1k = None
    for row in search_tables[0].rows:
        if len(row) >= 3 and row[0].strip().casefold() == "web search" and "all models" in row[1].casefold():
            web_search_per_1k = _amount(row[2], context="OpenAI web search per 1,000 calls")
            break
    if web_search_per_1k is None:
        raise PricingParseError("OpenAI: web-search per-1,000 price was not found")

    models: dict[str, dict[str, object]] = {}
    for model_id, (input_price, output_price, cache_read, cache_write) in standard.items():
        fast_multiplier = None
        if model_id in fast:
            fast_input, fast_output, _, _ = fast[model_id]
            input_multiplier = _ratio(fast_input, input_price, f"OpenAI fast mode {model_id}")
            output_multiplier = _ratio(fast_output, output_price, f"OpenAI fast mode {model_id}")
            if input_multiplier != output_multiplier:
                raise PricingParseError(f"OpenAI: fast-mode input and output multipliers disagree for {model_id}")
            fast_multiplier = {"multiplier": input_multiplier}
        models[model_id] = _base_row(
            input_price=input_price,
            output_price=output_price,
            cache_read=cache_read,
            cache_write_5m=cache_write,
            cache_write_1h=None,
            fast=fast_multiplier,
            web_search_per_1k=web_search_per_1k,
            source=OPENAI_URL,
        )
        if model_id == "gpt-6.1-sol":
            # The threshold is documented separately from the pricing table:
            # https://developers.openai.com/api/docs/models/gpt-6.1-sol
            row = next(row for row in standard_tables[0].rows if row and row[0] == model_id)
            context = f"OpenAI standard long context {model_id}"
            models[model_id]["long_context"] = {
                "threshold_tokens": 272_000,
                "input": _required_price(row, 5, context),
                "output": _required_price(row, 8, context),
                "cache_read": _required_price(row, 6, context),
                "cache_write_5m": _required_price(row, 7, context),
                "cache_write_1h": None,
            }
            if fast_multiplier is not None:
                fast_row = next(row for row in fast_tables[0].rows if row and row[0] == model_id)
                for index in range(5, 9):
                    multiplier = _ratio(_required_price(fast_row, index, context), _required_price(row, index, context), context)
                    if multiplier != fast_multiplier["multiplier"]:
                        raise PricingParseError(f"OpenAI: long-context fast multiplier disagrees for {model_id}")
    return models


def _render_table(table: Table, *, anthropic_models: bool = False) -> str:
    """Keep only table cell text and model links needed by the parser."""
    rows: list[str] = []
    for row_index, row in enumerate(table.rows):
        cells = row
        if anthropic_models and row_index >= 2 and row:
            cells = [_model_display(table, row_index), *row[1:]]
        rendered = "".join(f"<td>{html_escape(cell)}</td>" for cell in cells)
        rows.append(f"<tr>{rendered}</tr>")
    return "<table><tbody>" + "".join(rows) + "</tbody></table>"


def html_escape(value: str) -> str:
    # Kept local to avoid importing a browser-oriented or third-party parser.
    return value.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def _trimmed_fixture(provider: str, html: str) -> str:
    tables, page_text = _page(html, provider)
    if provider == "Anthropic":
        model_tables = [table for table in tables if _anthropic_model_table(table)]
        fast_tables = [
            table
            for table in tables
            if any(heading.startswith("Fast mode pricing") for heading in table.headings)
            and [cell.strip().casefold() for cell in table.rows[0]] == ["model", "input", "output"]
        ]
        if len(model_tables) != 1 or len(fast_tables) != 1:
            raise PricingParseError("Anthropic: cannot refresh fixtures because a pricing section is missing")
        web = re.search(
            r"(Web search is available on the Claude API for\s*\$?\s*[0-9]+(?:\.[0-9]+)?\s*per\s*1,000 searches[^.]*\.)",
            page_text,
            re.IGNORECASE,
        )
        standard_speed = re.search(
            r"(Fast mode is not available on Claude Opus 4\.7.*?Claude Opus 4\.6 \(requests run at standard speed and are billed at standard rates\)\.)",
            page_text,
            re.IGNORECASE,
        )
        if not web or not standard_speed:
            raise PricingParseError("Anthropic: cannot refresh fixtures because a tool or fast-mode rate note is missing")
        return "\n".join(
            (
                "<!doctype html><html><body>",
                "<h2>Model pricing</h2>",
                _render_table(model_tables[0], anthropic_models=True),
                "<h2>Fast mode pricing</h2>",
                _render_table(fast_tables[0]),
                f"<p>{html_escape(standard_speed.group(1))}</p>",
                f"<p>{html_escape(web.group(1))}</p>",
                "</body></html>",
                "",
            )
        )

    standard_tables = [table for table in tables if table.pane == "standard" and _openai_pricing_table(table)]
    fast_tables = [table for table in tables if table.pane == "fast" and _openai_pricing_table(table)]
    standalone_tables = [table for table in tables if table.pane is None and _openai_pricing_table(table)]
    cyber_tables = [table for table in standalone_tables if any(h.startswith("Cyber models") for h in table.headings)]
    search_tables = [
        table
        for table in tables
        if "Tools" in table.headings
        and any(cell.strip().casefold() == "pricing" for row in table.rows[:1] for cell in row)
    ]
    if any(len(selected) != 1 for selected in (standard_tables, fast_tables, cyber_tables, search_tables)):
        raise PricingParseError("OpenAI: cannot refresh fixtures because a pricing section is missing")
    return "\n".join(
        (
            "<!doctype html><html><body>",
            "<h2>Flagship models</h2>",
            '<div data-content-switcher-pane="true" data-value="standard">',
            _render_table(standard_tables[0]),
            "</div>",
            "<h2>Flagship models</h2>",
            '<div data-content-switcher-pane="true" data-value="fast">',
            _render_table(fast_tables[0]),
            "</div>",
            *[
                line
                for table in standalone_tables
                for line in (
                    f"<h2>{table.headings[-1] if table.headings else 'Models'}</h2>",
                    _render_table(table),
                )
            ],
            "<h2>Tools</h2>",
            _render_table(search_tables[0]),
            "</body></html>",
            "",
        )
    )


def load_pricing(path: Path = PRICING_PATH) -> dict[str, object]:
    try:
        data = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise PricingParseError(f"cannot read current pricing table {path}: {error}") from error
    if not isinstance(data, dict) or not isinstance(data.get("models"), dict):
        raise PricingParseError("current pricing table must contain an as_of date and models object")
    models = data["models"]
    if not data.get("as_of") or not models:
        raise PricingParseError("current pricing table has no as_of date or model rows")
    for model, row in models.items():
        _validate_row(model, row)
    return data


def _validate_row(model: str, row: object) -> None:
    if not isinstance(row, dict):
        raise PricingParseError(f"model {model!r} is not a JSON object")
    missing = [field for field in PRICE_FIELDS if field not in row]
    if missing:
        raise PricingParseError(f"model {model!r} is missing price fields: {', '.join(missing)}")
    if not isinstance(row.get("source"), str) or not row["source"].strip():
        raise PricingParseError(f"model {model!r} has no source URL")
    numeric_fields = ("input", "output", "cache_read", "cache_write_5m", "cache_write_1h", "web_search_per_1k")
    if any(row[field] is not None and not isinstance(row[field], (int, float)) for field in numeric_fields):
        raise PricingParseError(f"model {model!r} has a non-numeric price field")
    legacy = row.get("legacy")
    if not isinstance(legacy, dict) or any(field not in legacy for field in LEGACY_FIELDS):
        raise PricingParseError(f"model {model!r} is missing its legacy compatibility prices")
    if any(not isinstance(legacy[field], (int, float)) for field in LEGACY_FIELDS):
        raise PricingParseError(f"model {model!r} has a non-numeric legacy price")
    if row.get("fast") is not None:
        if not isinstance(row["fast"], dict) or not isinstance(row["fast"].get("multiplier"), (int, float)):
            raise PricingParseError(f"model {model!r} has an invalid fast-mode multiplier")
    if row.get("long_context") is not None:
        long = row["long_context"]
        long_fields = {
            "threshold_tokens",
            "input",
            "output",
            "cache_read",
            "cache_write_5m",
            "cache_write_1h",
        }
        if not isinstance(long, dict) or not long_fields.issubset(long):
            raise PricingParseError(f"model {model!r} has incomplete long-context prices")


def _row_prices(row: dict[str, object]) -> tuple[object, ...]:
    return tuple(row[field] for field in PRICE_FIELDS) + (row["legacy"],)


def _find_missing_section_models(current: dict[str, object], fetched: dict[str, dict[str, object]]) -> None:
    # The parsers require all source sections currently represented in the
    # table. Check coverage as a final guard against an empty or wrong source.
    current_models = current["models"]
    assert isinstance(current_models, dict)
    for source in (ANTHROPIC_URL, OPENAI_URL):
        old = {
            model
            for model, row in current_models.items()
            if row.get("source") == source and row.get("sync", True)
        }
        new = {model for model, row in fetched.items() if row["source"] == source}
        if old and not new:
            raise PricingParseError(f"{source}: no current model rows parsed; refusing to remove {len(old)} models")


def build_prices(anthropic_html: str, openai_html: str, current: dict[str, object]) -> dict[str, dict[str, object]]:
    fetched = parse_anthropic(anthropic_html)
    openai = parse_openai(openai_html)
    overlap = fetched.keys() & openai.keys()
    if overlap:
        raise PricingParseError(f"model ids appeared on both source pages: {', '.join(sorted(overlap))}")
    fetched.update(openai)

    # Preserve the two legacy OpenAI rows that the official page does not
    # price. They stay visible in the compatibility table, but the price
    # calculation deliberately has no rates for them.
    current_models = current["models"]
    assert isinstance(current_models, dict)
    for model, row in current_models.items():
        if row.get("sync") is False:
            fetched[model] = row

    _find_missing_section_models(current, fetched)
    for model, row in fetched.items():
        _validate_row(model, row)
    return dict(sorted(fetched.items()))


def fetch(url: str) -> str:
    request = Request(url, headers={"User-Agent": "semon-pricing-sync/1.0"})
    try:
        with urlopen(request, timeout=30) as response:
            body = response.read()
    except (OSError, URLError) as error:
        raise PricingParseError(f"could not fetch {url}: {error}") from error
    try:
        return body.decode("utf-8")
    except UnicodeDecodeError as error:
        raise PricingParseError(f"{url}: response was not valid UTF-8") from error


def _changes(old: dict[str, object], new_models: dict[str, dict[str, object]]) -> tuple[list[str], list[str], list[tuple[str, str, object, object]]]:
    old_models = old["models"]
    assert isinstance(old_models, dict)
    added = sorted(set(new_models) - set(old_models))
    removed = sorted(set(old_models) - set(new_models))
    changed: list[tuple[str, str, object, object]] = []
    for model in sorted(set(old_models) & set(new_models)):
        before = old_models[model]
        after = new_models[model]
        for field in PRICE_FIELDS:
            if before[field] != after[field]:
                changed.append((model, field, before[field], after[field]))
        for field in LEGACY_FIELDS:
            if before["legacy"][field] != after["legacy"][field]:
                changed.append((model, f"legacy.{field}", before["legacy"][field], after["legacy"][field]))
    return added, removed, changed


def _markdown_diff(
    old: dict[str, object],
    new_models: dict[str, dict[str, object]],
    added: list[str],
    removed: list[str],
    changed: list[tuple[str, str, object, object]],
) -> str:
    date = datetime.now(timezone.utc).date().isoformat()
    lines = [f"Pricing differences detected on {date}.", ""]
    if changed:
        lines.extend(("| Model | Field | Old | New |", "| --- | --- | ---: | ---: |"))
        for model, field, before, after in changed:
            lines.append(f"| `{model}` | `{field}` | `{_display(before)}` | `{_display(after)}` |")
        lines.append("")
    if added:
        lines.extend(("Models added:", *[f"- `{model}`" for model in added], ""))
    if removed:
        lines.extend(("Models removed:", *[f"- `{model}`" for model in removed], ""))
    lines.extend((f"Sources: [Anthropic]({ANTHROPIC_URL}), [OpenAI]({OPENAI_URL}).", ""))
    return "\n".join(lines)


def _display(value: object) -> str:
    if value is None:
        return "null"
    if isinstance(value, (dict, list)):
        return json.dumps(value, sort_keys=True, separators=(",", ":"))
    return str(value)


def synchronize(*, path: Path = PRICING_PATH, dry_run: bool = False) -> tuple[bool, str]:
    current = load_pricing(path)
    anthropic_html = fetch(ANTHROPIC_URL)
    openai_html = fetch(OPENAI_URL)
    new_models = build_prices(anthropic_html, openai_html, current)
    added, removed, changed = _changes(current, new_models)
    has_price_change = bool(added or removed or changed)
    if not has_price_change:
        return False, "No pricing changes."

    report = _markdown_diff(current, new_models, added, removed, changed)
    output = {
        "as_of": datetime.now(timezone.utc).date().isoformat(),
        "models": new_models,
    }
    if not dry_run:
        anthropic_fixture = _trimmed_fixture("Anthropic", anthropic_html)
        openai_fixture = _trimmed_fixture("OpenAI", openai_html)
        path.write_text(json.dumps(output, indent=2, ensure_ascii=False) + "\n")
        fixture_dir = ROOT / "scripts/pricing/fixtures"
        (fixture_dir / "anthropic.html").write_text(anthropic_fixture)
        (fixture_dir / "openai.html").write_text(openai_fixture)
    return True, report


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dry-run", action="store_true", help="report changes without writing pricing.json")
    args = parser.parse_args(argv)
    try:
        _changed, report = synchronize(dry_run=args.dry_run)
    except PricingParseError as error:
        print(f"Pricing sync parse failure: {error}", file=sys.stderr)
        return 2
    except OSError as error:
        print(f"Pricing sync write failure: {error}", file=sys.stderr)
        return 1
    print(report)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
