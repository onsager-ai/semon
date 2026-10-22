"""Retired executable graph and independently captured native expectations (#469)."""
import hashlib
import json
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "crates/semon-sessions/src"


class ModelRetirement(unittest.TestCase):
    def test_only_native_producer_targets_and_no_model_transport_adapters(self):
        model = (SRC / "model.rs").read_text()
        targets = re.search(r"enum BuildTarget\s*\{([^}]+)\}", model).group(1)
        self.assertEqual(re.findall(r"\b[A-Z]\w*\b", targets), ["Query", "Catalog", "Sources"])
        for path in SRC.rglob("*.rs"):
            with self.subTest(source=path.relative_to(SRC)):
                text = path.read_text()
                self.assertNotRegex(text, r"\b(?:BuildTarget|BuildOutput)::Model\b")
                self.assertNotRegex(text, r"\b(?:COMPAT_BUILDS|AFTER_MODEL_DERIVATION|MODEL_API)\b")
                self.assertNotRegex(text, r"\bmodel::(?:reference_json|build|build_sources)\s*\(")
        self.assertNotRegex(model, r"\bstruct\s+(?:Built|Rest|Machine)\b")
        self.assertNotRegex(model, r"\bfn\s+(?:build|build_sources|reference_json|texts_by_turn)\s*\(")
        renderer = (SRC / "tx.rs").read_text()
        self.assertNotRegex(renderer, r"\bfn\s+(?:page|page_limited|page_from|signals|errors|errors_limited|full_slot|full_yielded|full|text_json|read_record)\s*\(")
        self.assertNotIn("ERRORS_MAX", renderer)

    def test_retired_json_cache_import_is_removed(self):
        self.assertFalse((SRC / "events/store/json_migration.rs").exists())
        events = (SRC / "events.rs").read_text()
        store = (SRC / "events/store.rs").read_text()
        self.assertNotIn("legacy_path", events)
        for symbol in (
            "json_migration",
            "import_legacy",
            "warn_import",
            "BEFORE_IMPORT",
            "AFTER_CLAIM",
            "IMPORTS",
            "set_aside_legacy",
        ):
            with self.subTest(symbol=symbol):
                self.assertNotIn(symbol, store)

    def test_runtime_fences_have_no_upgrade_anchors(self):
        for path in ("events/store.rs", "source_change.rs", "native_evidence.rs", "native_evidence/runtime.rs"):
            text = (SRC / path).read_text()
            for symbol in ("preserve_legacy_runtime", "legacy_runtime_generation", "legacy_before", "catalog_legacy_runtime"):
                with self.subTest(source=path, symbol=symbol):
                    self.assertNotIn(symbol, text)

    def test_independent_reference_bytes_and_capture_provenance(self):
        expected = {
            "query/contract-reference.json": "19c05ca78463bdc64dc8f54696994bdda108078d036cca80c5c648b7016cbe7c",
            # Independent #466 pricing-only refresh, before this cleanup:
            # 634b5312f9359dae1a9ab1c6979ae4878539e5bc. Native semantic fields
            # still match the original 5f346070 Model capture; see the design doc.
            "model/golden/answers.json": "dfaea17d3c32d5b8052957dc8854ff6115ff4ec8388868a13f4cd8577143312a",
            "model/golden/lineage.json": "b6fcebdab482f037d13c1897131cbf58deb3f0300e8e696d946355c533b5616c",
            "model/golden/relays.json": "8b48fd581ec5c254b759d9a1fce23440fef9cccf99e68ded1f33b30bdf0bcf6e",
            "model/golden/spawns.json": "69037bf44b4654bcbf68818c3c26bcc95f1982eb695190c6177bf273f3ce0972",
            "model/golden/turns.json": "ae0d3a4a529400fff361d21674b01a81ffd039e110a4a3038489988c7cb83781",
        }
        for path, digest in expected.items():
            with self.subTest(reference=path):
                self.assertEqual(hashlib.sha256((SRC / path).read_bytes()).hexdigest(), digest)
        query = json.loads((SRC / "query/contract-reference.json").read_text())
        self.assertEqual(query["source_revision"], "7ce1ac4d4f34af9143aa292b631a07fd797890b1")
        self.assertEqual(len(query["responses"]), 134)


if __name__ == "__main__":
    unittest.main()
