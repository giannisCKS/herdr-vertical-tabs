#!/usr/bin/env python3
"""Report local installation drift without changing files or running updates."""

import hashlib
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent


def compare(label: str, source: Path, installed: Path) -> bool:
    try:
        expected = hashlib.sha256(source.read_bytes()).hexdigest()
        actual = hashlib.sha256(installed.read_bytes()).hexdigest()
    except OSError as error:
        print(f"ERROR {label}: {error}")
        return False
    matches = expected == actual
    print(f"{'OK' if matches else 'DRIFT'} {label}: {installed}")
    print(f"  source={expected} installed={actual}")
    return matches


def main() -> int:
    home = Path.home()
    results = [
        compare("Herdr binary", ROOT / "target/release/herdr", home / ".local/bin/herdr"),
        compare(
            "OpenCode hook",
            ROOT / "src/integration/assets/opencode/herdr-tui-session.js",
            home / ".config/opencode/herdr-tui-session.js",
        ),
    ]
    print("Byte comparison only: does not prove build freshness or runtime compatibility.")
    return 0 if all(results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
