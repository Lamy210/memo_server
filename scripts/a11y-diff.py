#!/usr/bin/env python3
"""Compare axe observations from the base and candidate UI captures."""

from __future__ import annotations

import re
import sys
from pathlib import Path

SUMMARY_RE = re.compile(
    r"^A11Y (?P<screen>[^:]+): violations=(?P<violations>\d+), "
    r"serious_or_critical=(?P<high>\d+)$"
)
DIAGNOSTIC_RE = re.compile(
    r"^A11Y-DIAGNOSTIC (?P<screen>[^:]+): (?P<rule>\S+) "
    r"impact=(?P<impact>serious|critical) nodes=(?P<nodes>\d+)\b"
)


def parse_log(path: Path) -> tuple[set[str], dict[tuple[str, str], int]]:
    screens: set[str] = set()
    findings: dict[tuple[str, str], int] = {}

    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()

        summary = SUMMARY_RE.match(line)
        if summary:
            screens.add(summary.group("screen"))
            continue

        diagnostic = DIAGNOSTIC_RE.match(line)
        if diagnostic:
            key = (diagnostic.group("screen"), diagnostic.group("rule"))
            findings[key] = int(diagnostic.group("nodes"))

    if not screens:
        raise ValueError(f"{path} does not contain axe summaries")

    return screens, findings


def main() -> int:
    if len(sys.argv) != 4:
        print(
            "usage: a11y-diff.py <base-log> <candidate-log> <summary-output>",
            file=sys.stderr,
        )
        return 2

    base_path = Path(sys.argv[1])
    candidate_path = Path(sys.argv[2])
    output_path = Path(sys.argv[3])

    try:
        base_screens, base_findings = parse_log(base_path)
        candidate_screens, candidate_findings = parse_log(candidate_path)
    except (OSError, ValueError) as error:
        print(f"Accessibility comparison error: {error}", file=sys.stderr)
        return 2

    if base_screens != candidate_screens:
        print(
            "Accessibility comparison error: base/candidate screen sets differ: "
            f"base={sorted(base_screens)} candidate={sorted(candidate_screens)}",
            file=sys.stderr,
        )
        return 2

    keys = sorted(set(base_findings) | set(candidate_findings))
    regressions: list[tuple[str, str, int, int]] = []

    lines = [
        "## Accessibility regression gate",
        "",
        "| Screen | Rule | Base nodes | PR nodes | Delta | Result |",
        "| --- | --- | ---: | ---: | ---: | --- |",
    ]

    if not keys:
        lines.append("| — | — | 0 | 0 | 0 | pass |")

    for screen, rule in keys:
        base_nodes = base_findings.get((screen, rule), 0)
        candidate_nodes = candidate_findings.get((screen, rule), 0)
        delta = candidate_nodes - base_nodes
        result = "pass"

        if delta > 0:
            result = "regression"
            regressions.append((screen, rule, base_nodes, candidate_nodes))

        lines.append(
            f"| `{screen}` | `{rule}` | {base_nodes} | {candidate_nodes} | "
            f"{delta:+d} | {result} |"
        )

    lines.extend(
        [
            "",
            "Existing serious/critical findings are treated as the baseline. "
            "A new rule or an increased affected-node count fails the PR; improvements pass.",
        ]
    )

    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(output_path.read_text(encoding="utf-8"), end="")

    if regressions:
        print("Accessibility regression detected.", file=sys.stderr)
        return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
