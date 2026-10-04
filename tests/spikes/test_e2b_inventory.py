"""Offline official-SDK interface fixtures; never contact a provider."""
import importlib.util
from pathlib import Path
from types import SimpleNamespace
import unittest

spec = importlib.util.spec_from_file_location("inventory", Path(__file__).parents[2] / "scripts/e2b-inventory.py")
worker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(worker)


class InventoryTests(unittest.TestCase):
    def request(self):
        return {"version": 1, "scope": {"semon_deployment": "hub-prod", "semon_session": "session-1", "semon_operation": "launch-1"}, "api_key": "synthetic-provider-secret"}

    def run_inventory(self, pages, request=None, clock=lambda: 0):
        calls = []
        class Paginator:
            @property
            def has_next(self):
                return bool(pages)
            def next_items(self, **opts):
                calls.append(opts)
                page = pages.pop(0)
                if isinstance(page, Exception):
                    raise page
                return page
        class Sandbox:
            @staticmethod
            def list(**opts):
                calls.append(opts)
                return Paginator()
        result = worker.inventory(request or self.request(), Sandbox, lambda **kw: kw, clock)
        return result, calls

    def item(self, state="running"):
        metadata = self.request()["scope"] | {"semon_version": "1", "semon_owner": "owner-1", "semon_workspace": "workspace-1", "semon_epoch": "1", "private": "must-not-relay"}
        return SimpleNamespace(sandbox_id="sandbox-1", state=SimpleNamespace(value=state), metadata=metadata)

    def test_all_pages_running_and_paused_without_vendor_metadata(self):
        result, calls = self.run_inventory([[self.item()], [self.item("paused")]])
        self.assertEqual(result["status"], "complete")
        self.assertEqual([r["state"] for r in result["resources"]], ["running", "paused"])
        self.assertNotIn("must-not-relay", str(result))
        self.assertNotIn("synthetic-provider-secret", str(result))
        self.assertEqual(calls[0]["query"], {"metadata": self.request()["scope"]})
        # No running-only filter: the SDK's documented default includes paused.
        self.assertEqual(calls[1:], [{"request_timeout": 5}, {"request_timeout": 5}])

    def test_partial_failure_never_returns_complete_absence_or_vendor_error(self):
        result, _ = self.run_inventory([[self.item()], RuntimeError("synthetic-secret-in-vendor-error")])
        self.assertEqual(result, {"version": 1, "status": "unavailable"})

    def test_page_resource_and_deadline_limits_are_incomplete(self):
        for pages in [[[ ] for _ in range(33)], [[self.item() for _ in range(1001)]]]:
            self.assertEqual(self.run_inventory(pages)[0]["status"], "incomplete")
        ticks = iter([0, 21])
        self.assertEqual(self.run_inventory([[]], clock=lambda: next(ticks))[0]["status"], "incomplete")

    def test_invalid_scope_or_credentials_never_call_sdk(self):
        for key, value in [("version", True), ("api_key", ""), ("scope", {"semon_session": "../other"})]:
            request = self.request() | {key: value}
            result, calls = self.run_inventory([], request)
            self.assertEqual(result["status"], "unavailable")
            self.assertEqual(calls, [])

    def test_query_mismatch_and_bad_metadata_cannot_establish_absence(self):
        for key, value in [("semon_session", "foreign"), ("semon_owner", "bad\nsecret")]:
            item = self.item()
            item.metadata[key] = value
            self.assertEqual(self.run_inventory([[item]])[0]["status"], "incomplete")

    def test_unknown_state_and_complete_empty_inventory_remain_distinct(self):
        self.assertEqual(self.run_inventory([[self.item("unrecognized")]])[0]["resources"][0]["state"], "unknown")
        self.assertEqual(self.run_inventory([[]])[0], {"version": 1, "status": "complete", "resources": []})


class ValidationTests(unittest.TestCase):
    def request(self):
        return {"version": 1, "method": "validate_credential", "scope": {"semon_deployment": "hub-prod", "semon_owner": "owner-1"}, "api_key": "synthetic-private-key"}

    def run_validation(self, failure=None, request=None, clock=lambda: 0):
        calls = []
        class AuthError(Exception):
            pass
        class Paginator:
            def next_items(self, **opts):
                calls.append(opts)
                if failure == "auth":
                    raise AuthError("synthetic-key-in-error")
                if isinstance(failure, Exception):
                    raise failure
                return [{"private": "must-not-relay"}]
        class Sandbox:
            @staticmethod
            def list(**opts):
                calls.append(opts)
                return Paginator()
        result = worker.validate_credential(request or self.request(), Sandbox, lambda **kw: kw, AuthError, clock)
        return result, calls

    def test_one_authenticated_page_has_no_resource_or_key_output(self):
        result, calls = self.run_validation()
        self.assertEqual(result, {"version": 1, "status": "credential_valid"})
        self.assertEqual(len(calls), 2)
        self.assertEqual(calls[0]["limit"], 1)
        self.assertEqual(calls[0]["query"], {"metadata": self.request()["scope"]})

    def test_auth_rejection_and_outage_are_distinct_and_sanitized(self):
        for failure, status in [("auth", "credential_rejected"), (RuntimeError("private-vendor-error"), "unavailable")]:
            self.assertEqual(self.run_validation(failure)[0], {"version": 1, "status": status})
        forbidden = RuntimeError("private-vendor-permission-error")
        forbidden.status_code = 403
        self.assertEqual(self.run_validation(forbidden)[0]["status"], "credential_rejected")

    def test_invalid_or_unsupported_requests_never_contact_sdk(self):
        for key, value in [("method", "create"), ("api_key", ""), ("scope", {"semon_owner": "owner-1"}), ("version", True)]:
            result, calls = self.run_validation(request=self.request() | {key: value})
            self.assertEqual(result["status"], "unavailable")
            self.assertEqual(calls, [])

    def test_response_after_deadline_is_not_validated(self):
        ticks = iter([0, 6])
        self.assertEqual(self.run_validation(clock=lambda: next(ticks))[0]["status"], "unavailable")


if __name__ == "__main__":
    unittest.main()
