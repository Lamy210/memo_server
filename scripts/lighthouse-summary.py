#!/usr/bin/env python3
"""Render representative Lighthouse CI results as a Markdown summary."""

from __future__ import annotations

import json
import sys
from pathlib import Path
from urllib.parse import urlparse


def percent(value: object) -> str:
    if not isinstance(value, (int, float)):
        return "—"
    return f"{round(value * 100):d}"


def main() -> int:
    if len(sys.argv) != 3:
        print(
            "usage: lighthouse-summary.py <manifest.json> <summary-output>",
            file=sys.stderr,
        )
        return 2

    manifest_path = Path(sys.argv[1])
    output_path = Path(sys.argv[2])

    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        print(f"Failed to read Lighthouse manifest: {error}", file=sys.stderr)
        return 2

    representative = [entry for entry in manifest if entry.get("isRepresentativeRun")]
    if not representative:
        print("Lighthouse manifest has no representative runs.", file=sys.stderr)
        return 2

    lines = [
        "## Lighthouse baseline",
        "",
        "| Route | Performance | Accessibility | Best practices | SEO |",
        "| --- | ---: | ---: | ---: | ---: |",
    ]

    for entry in sorted(representative, key=lambda item: str(item.get("url", ""))):
        parsed = urlparse(str(entry.get("url", "")))
        route = parsed.path or "/"
        if parsed.query:
            route += f"?{parsed.query}"

        scores = entry.get("summary") or {}
        lines.append(
            f"| `{route}` | {percent(scores.get('performance'))} | "
            f"{percent(scores.get('accessibility'))} | "
            f"{percent(scores.get('best-practices'))} | "
            f"{percent(scores.get('seo'))} |"
        )

    lines.extend(
        [
            "",
            "Scores are representative (median) Lighthouse CI runs on a production preview.",
            "Category thresholds are warnings while the initial baseline is being established; collection or report failures still fail CI.",
        ]
    )

    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(output_path.read_text(encoding="utf-8"), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
