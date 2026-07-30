#!/usr/bin/env python3
"""Convert Criterion estimates into JSONL rows for time-series benchmark tracking.

Walks `target/criterion/**/new/estimates.json` and prints one JSON line per
benchmark group with timestamp, commit SHA, group name, and key statistics.
"""

import argparse
import json
from datetime import datetime, timezone
from pathlib import Path

FIXTURE_MANIFEST = Path("benches/large-repository-fixture")


def parse_fixture_manifest(path: Path) -> dict[str, str | int]:
    values: dict[str, str] = {}
    for line_number, line in enumerate(path.read_text().splitlines(), start=1):
        try:
            key, value = line.split("=", 1)
        except ValueError as error:
            raise ValueError(f"{path}:{line_number}: expected key=value") from error
        if key in values:
            raise ValueError(f"{path}:{line_number}: duplicate {key}")
        if not value:
            raise ValueError(f"{path}:{line_number}: empty {key}")
        values[key] = value

    expected = {"schema", "corpus", "revision"}
    if values.keys() != expected:
        missing = expected - values.keys()
        unknown = values.keys() - expected
        raise ValueError(
            f"{path}: expected schema, corpus, revision; "
            f"missing={sorted(missing)}, unknown={sorted(unknown)}"
        )
    try:
        schema = int(values["schema"])
    except ValueError as error:
        raise ValueError(f"{path}: schema must be an integer") from error
    if schema != 1:
        raise ValueError(f"{path}: unsupported schema {schema}")
    revision = values["revision"]
    if len(revision) != 40 or any(
        character not in "0123456789abcdefABCDEF" for character in revision
    ):
        raise ValueError(
            f"{path}: revision must be a 40-character hexadecimal object ID"
        )

    return {
        "schema": schema,
        "corpus": values["corpus"],
        "revision": revision,
    }


def criterion_rows(
    root: Path,
    sha: str,
    timestamp: str,
    fixture: dict[str, str | int],
) -> list[dict[str, str | int | float]]:
    estimates = sorted(root.glob("**/new/estimates.json"))
    if not estimates:
        raise ValueError(f"No criterion estimates found under {root}")

    rows = []
    for estimate in estimates:
        bench = estimate.relative_to(root).parent.parent.as_posix()
        data = json.loads(estimate.read_text())
        rows.append(
            {
                "ts": timestamp,
                "sha": sha,
                "bench": bench,
                "mean_ns": data["mean"]["point_estimate"],
                "stddev_ns": data["std_dev"]["point_estimate"],
                "fixture_schema": fixture["schema"],
                "fixture_corpus": fixture["corpus"],
                "fixture_revision": fixture["revision"],
            }
        )
    return rows


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sha", required=True, help="commit SHA to record")
    parser.add_argument(
        "--root",
        type=Path,
        default=Path("target/criterion"),
        help="Criterion output root (default: target/criterion)",
    )
    args = parser.parse_args()

    ts = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    try:
        fixture = parse_fixture_manifest(FIXTURE_MANIFEST)
        rows = criterion_rows(args.root, args.sha, ts, fixture)
    except ValueError as error:
        raise SystemExit(str(error)) from error
    for row in rows:
        print(json.dumps(row))


if __name__ == "__main__":
    main()
