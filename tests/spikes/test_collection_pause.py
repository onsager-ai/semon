"""Default delivery entry guards; retained imported engine oracles stay separate."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[2] / "scripts"


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), SCRIPTS / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class CollectionPause(unittest.TestCase):
    def test_workers_refuse_stale_and_malformed_delivery_before_sdk_admission(self):
        for name, engine in [("e2b-runtime", "execute"), ("e2b-managed", "managed")]:
            module = load(name)
            for value in [{"method": "create"}, {"method": "bootstrap"},
                          {"method": "dispatch"}, {"method": "qualify_profile"},
                          *({"method": "rpc", "payload": {"op": op}} for op in
                            ["send", "answer", "interrupt", "reconnect", "renew", "end"]),
                          None, 17, "malformed-json"]:
                raw = value.encode() if isinstance(value, str) else json.dumps(value).encode()
                output = io.StringIO()
                stdin = io.TextIOWrapper(io.BytesIO(raw))
                with self.subTest(worker=name, request=value), \
                        patch.object(module.sys, "stdin", stdin), \
                        patch.object(module.importlib.metadata, "version") as sdk, \
                        patch.object(module, engine) as delivery, \
                        contextlib.redirect_stdout(output):
                    module.main()
                self.assertEqual(json.loads(output.getvalue())["error"], "capability_unavailable")
                sdk.assert_not_called()
                delivery.assert_not_called()

    def test_worker_receipt_inspection_keeps_existing_reconciliation(self):
        for name, engine, request in [
                ("e2b-runtime", "execute", {"method": "inspect"}),
                ("e2b-runtime", "execute", {"method": "inspect_bootstrap"}),
                ("e2b-managed", "managed", {"method": "rpc", "payload": {"op": "snapshot"}})]:
            module = load(name)
            output = io.StringIO()
            stdin = io.TextIOWrapper(io.BytesIO(json.dumps(request).encode()))
            # Stop at SDK admission: no provider, credential, network or command.
            with self.subTest(worker=name, request=request), \
                    patch.object(module.sys, "stdin", stdin), \
                    patch.object(module.importlib.metadata, "version", side_effect=RuntimeError("offline")) as sdk, \
                    patch.object(module, engine) as delivery, \
                    contextlib.redirect_stdout(output):
                module.main()
            sdk.assert_called_once_with("e2b")
            delivery.assert_not_called()
            self.assertNotEqual(json.loads(output.getvalue()).get("error"), "capability_unavailable")

    def test_guest_launch_refuses_without_invoking_retained_engine(self):
        module = load("runtime-guest")
        for arguments in [["launch"], [], ["resume"]]:
            output = io.StringIO()
            with patch.object(module, "launch") as launch, contextlib.redirect_stdout(output):
                self.assertEqual(module.main(arguments), 1)
            launch.assert_not_called()
            self.assertEqual(json.loads(output.getvalue())["error"], "capability_unavailable")


if __name__ == "__main__":
    unittest.main()
