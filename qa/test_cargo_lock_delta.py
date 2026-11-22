"""Exercise dependency identity deltas against real TOML lock shapes."""
import importlib.util
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("delta", Path(__file__).with_name("cargo_lock_delta.py"))
delta = importlib.util.module_from_spec(spec)
spec.loader.exec_module(delta)


class DeltaTests(unittest.TestCase):
    def test_multiversion_and_sources_are_not_collapsed_by_name(self):
        old = [{"name": "x", "version": "1", "source": "registry+a", "checksum": "old"}, {"name": "x", "version": "2", "source": "registry+a"}]
        new = [{"name": "x", "version": "2", "source": "registry+a"}, {"name": "x", "version": "1", "source": "git+b"}]
        result = delta.compare(old, new)
        self.assertEqual(len(result["added"]), 1)
        self.assertEqual(len(result["removed"]), 1)
        self.assertEqual(result["changed"][0]["name"], "x")
        self.assertEqual(result["changed"][0]["old"][0]["checksum"], "old")
        self.assertEqual(len(result["changed"][0]["new"]), 2)

    def test_checksum_only_change_is_retained(self):
        result = delta.compare([{"name": "a", "version": "1", "checksum": "a"}], [{"name": "a", "version": "1", "checksum": "b"}])
        self.assertEqual(result["added"], [])
        self.assertEqual(result["removed"], [])
        self.assertEqual(len(result["changed"]), 1)

    def test_identical_graph_is_empty(self):
        packages = [{"name": "workspace", "version": "0.1"}]
        self.assertEqual(delta.compare(packages, packages), {"added": [], "removed": [], "changed": []})


if __name__ == "__main__":
    unittest.main()
