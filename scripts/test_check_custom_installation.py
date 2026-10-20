import contextlib
import importlib.util
import io
from pathlib import Path
import tempfile
import unittest


spec = importlib.util.spec_from_file_location(
    "custom_installation", Path(__file__).with_name("check-custom-installation.py")
)
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)


class CustomInstallationTests(unittest.TestCase):
    def test_match_drift_and_missing_are_read_only(self):
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(io.StringIO()):
            source = Path(directory) / "source"
            installed = Path(directory) / "installed"
            source.write_bytes(b"source")
            installed.write_bytes(b"source")
            self.assertTrue(checker.compare("test", source, installed))
            installed.write_bytes(b"different")
            self.assertFalse(checker.compare("test", source, installed))
            self.assertEqual(installed.read_bytes(), b"different")
            self.assertFalse(checker.compare("test", source, Path(directory) / "missing"))
            self.assertEqual(source.read_bytes(), b"source")


if __name__ == "__main__":
    unittest.main()
