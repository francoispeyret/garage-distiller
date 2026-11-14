"""Offline guardrail tests; never calls GitHub or creates a production key."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import configure_updates as config


class SetupTests(unittest.TestCase):
    def run_setup(self, has_secret: bool, has_variable: bool):
        calls = []
        remote = {"secret": has_secret, "variable": has_variable}
        public = "ab" * 32

        def github(*args, stdin=None):
            calls.append((args, stdin))
            kind, command = args[:2]
            if command == "list":
                name = config.SECRET if kind == "secret" else config.VARIABLE
                return json.dumps([{"name": name}] if remote[kind] else [])
            if command == "get":
                return public
            if command == "set":
                self.assertFalse(remote[kind], "An existing value must never be overwritten")
                if kind == "secret":
                    self.assertIsNotNone(stdin)
                    self.assertNotIn(stdin, args)
                remote[kind] = True
                return ""
            raise AssertionError(args)

        def generate(args, **kwargs):
            Path(args[2]).write_bytes(b"test-only" * 8)
            Path(args[3]).write_bytes(bytes.fromhex(public))

        with tempfile.TemporaryDirectory() as temp:
            public_file = Path(temp) / "public.txt"
            with patch.object(config, "PUBLIC_FILE", public_file), patch.object(config, "github", github), patch.object(config.shutil, "which", return_value="zipsign"), patch.object(config.subprocess, "run", generate):
                if has_secret != has_variable:
                    with self.assertRaisesRegex(RuntimeError, "Incomplete"):
                        config.configure("zipsign")
                    self.assertFalse(public_file.exists())
                else:
                    config.configure("zipsign")
                    self.assertEqual(public_file.read_text().strip(), public)
        return [args for args, _ in calls if args[1] == "set"]

    def test_fresh_configuration_sets_both_values_once(self):
        writes = self.run_setup(False, False)
        self.assertEqual([w[0] for w in writes], ["secret", "variable"])

    def test_existing_configuration_is_preserved(self):
        self.assertEqual(self.run_setup(True, True), [])

    def test_partial_configuration_is_never_overwritten(self):
        self.assertEqual(self.run_setup(True, False), [])
        self.assertEqual(self.run_setup(False, True), [])


if __name__ == "__main__":
    unittest.main()
