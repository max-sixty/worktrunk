#!/usr/bin/env python3
"""Convert Criterion estimates into JSONL rows for time-series benchmark tracking.

Walks `target/criterion/**/new/estimates.json` and prints one JSON line per
benchmark group with timestamp, commit SHA, group name, and key statistics.
"""

import argparse
import json
from datetime import datetime, timezone
from pathlib import Path


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
    estimates = sorted(args.root.glob("**/new/estimates.json"))
    if not estimates:
        raise SystemExit(f"No criterion estimates found under {args.root}")
    for est in estimates:
        bench = est.relative_to(args.root).parent.parent.as_posix()
        data = json.loads(est.read_text())
        row = {
            "ts": ts,
            "sha": args.sha,
            "bench": bench,
            "mean_ns": data["mean"]["point_estimate"],
            "stddev_ns": data["std_dev"]["point_estimate"],
        }
        print(json.dumps(row))


if __name__ == "__main__":
    main()
