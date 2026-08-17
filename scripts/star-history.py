#!/usr/bin/env python3
"""Render the README's star-history chart from GitHub's stargazers API.

GitHub restricted who may read a repository's stargazers in mid-2026, limiting
it to the repo's admins and collaborators, which is why every hosted chart
service renders an error image: a service is nobody's collaborator. We can
still read our own, so the chart is built here from our own query and
committed, rather than fetched from a third party every time someone loads the
README.

That surface is in flux, so this fails loudly and keeps the last good chart: a
run that cannot complete the walk publishes nothing and the previously
committed SVG stays up.

Two outputs, both published to the `star-history` branch, which each run
replaces wholesale:

- `star-history.csv` — one `date,stars` row per day, cumulative. Every run
  re-derives the whole series, because the endpoint lists only accounts that
  star the repo *now*: an unstar retroactively lowers the count on the day that
  account first starred, so the curve moves slightly under its own past. The
  published CSV is therefore the current series rather than a running log, and
  it is what the chart can still be drawn from if GitHub restricts the endpoint
  further.
- `star-history.svg` — rendered from that series, transparent background so it
  reads on GitHub's light and dark themes without a `<picture>` element.

Requires `gh` authenticated with write access to the repo, which is what GitHub
accepts as proof of being a collaborator here — read-only credentials get 403.
No arguments: the repository and output paths are fixed, since one repo is
charted.
"""

import csv
import datetime as dt
import json
import subprocess
import sys
import time
from collections import Counter
from pathlib import Path

REPO = "max-sixty/worktrunk"
OWNER, NAME = REPO.split("/")
OUT_DIR = Path(__file__).resolve().parent.parent
CSV_PATH = OUT_DIR / "star-history.csv"
SVG_PATH = OUT_DIR / "star-history.svg"

# Chart geometry, in SVG user units.
WIDTH, HEIGHT = 800, 400
PAD_L, PAD_R, PAD_T, PAD_B = 60, 20, 30, 40

# Mid-tones legible against both a white and a near-black page.
ACCENT = "#3b82f6"
MUTED = "#8b949e"


# GraphQL rather than REST: GitHub restricted `GET /repos/{owner}/{repo}
# /stargazers`, and this field returns just the timestamps the chart needs
# instead of a full user object per stargazer. Ordering is left to the server —
# the caller buckets by date, so page order does not matter.
STARGAZER_PAGE = """
query($endCursor: String) {
  repository(owner: "%s", name: "%s") {
    stargazers(first: 100, after: $endCursor) {
      pageInfo { hasNextPage endCursor }
      edges { starredAt }
    }
  }
}
""" % (OWNER, NAME)

PAGE_ATTEMPTS = 5


def fetch_page(cursor: str | None) -> dict:
    """One page of stargazers, retried so a blip doesn't cost the whole walk.

    A full history is tens of sequential pages, and a GitHub API incident part
    way through leaves the run with nothing to show for the pages it did fetch.
    """
    args = ["gh", "api", "graphql", "-f", f"query={STARGAZER_PAGE}"]
    if cursor:
        args += ["-f", f"endCursor={cursor}"]
    for attempt in range(1, PAGE_ATTEMPTS + 1):
        result = subprocess.run(args, capture_output=True, text=True)
        if result.returncode == 0:
            return json.loads(result.stdout)["data"]["repository"]["stargazers"]
        if attempt == PAGE_ATTEMPTS:
            sys.exit(
                f"gh api graphql failed fetching stargazers for {REPO} after "
                f"{PAGE_ATTEMPTS} attempts: {result.stderr.strip()}"
            )
        time.sleep(2**attempt)
    raise AssertionError("unreachable")


def fetch_starred_at() -> list[str]:
    """Every current stargazer's `starred_at`.

    Walks every page or exits nonzero — a partial history would publish a chart
    that silently understates the repo.
    """
    timestamps, cursor = [], None
    while True:
        page = fetch_page(cursor)
        timestamps += [edge["starredAt"] for edge in page["edges"]]
        if not page["pageInfo"]["hasNextPage"]:
            return timestamps
        cursor = page["pageInfo"]["endCursor"]


def daily_cumulative(timestamps: list[str]) -> list[tuple[dt.date, int]]:
    """One (date, cumulative stars) point per day from the first star to today."""
    per_day = Counter(ts[:10] for ts in timestamps)
    day = dt.date.fromisoformat(min(per_day))
    today = dt.datetime.now(dt.UTC).date()
    series, total = [], 0
    while day <= today:
        total += per_day.get(day.isoformat(), 0)
        series.append((day, total))
        day += dt.timedelta(days=1)
    return series


# Label at most this many months, so the axis stays legible as the series grows
# past the ~12 months that fit the chart's width one-per-month.
MAX_MONTH_LABELS = 12


def month_ticks(series: list[tuple[dt.date, int]]) -> list[tuple[int, str]]:
    """Index and label for month starts, thinned to `MAX_MONTH_LABELS`.

    A label carries its year when the year changes from the previous label (and
    on the first one). Keying off the change rather than off January matters
    once thinning kicks in: a stride of 3 starting in November keeps
    Nov/Feb/May/Aug and would otherwise never land on a January, leaving a
    multi-year axis with repeated bare month names and no year anywhere.
    """
    months = [i for i, (day, _) in enumerate(series) if day.day == 1]
    stride = -(-len(months) // MAX_MONTH_LABELS) or 1
    ticks, shown_year = [], None
    for i in months[::stride]:
        day = series[i][0]
        ticks.append((i, day.strftime("%b %Y" if day.year != shown_year else "%b")))
        shown_year = day.year
    return ticks


def y_ticks(peak: int) -> list[int]:
    """Round gridline values from 0 to the first round number above `peak`.

    `step` starts at the largest power of ten at or below `peak`, so `peak /
    step` is under 10 and one doubling always brings it under the limit.
    """
    step = 10 ** (len(str(peak)) - 1)
    if peak / step > 7:
        step *= 2
    return list(range(0, peak + step, step))


def render(series: list[tuple[dt.date, int]]) -> str:
    peak = max(count for _, count in series)
    ticks = y_ticks(peak)
    top = ticks[-1]
    plot_w = WIDTH - PAD_L - PAD_R
    plot_h = HEIGHT - PAD_T - PAD_B

    def x_of(i: int) -> float:
        return PAD_L + plot_w * i / (len(series) - 1)

    def y_of(count: int) -> float:
        return PAD_T + plot_h * (1 - count / top)

    points = " ".join(f"{x_of(i):.1f},{y_of(c):.1f}" for i, (_, c) in enumerate(series))
    area = f"{PAD_L},{PAD_T + plot_h} {points} {PAD_L + plot_w},{PAD_T + plot_h}"

    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{HEIGHT}" '
        f'viewBox="0 0 {WIDTH} {HEIGHT}" role="img" '
        f'aria-label="Star history for {REPO}: {peak} stars">',
        '<g font-family="-apple-system, BlinkMacSystemFont, Segoe UI, Helvetica, Arial, sans-serif" '
        f'font-size="12" fill="{MUTED}">',
    ]

    for value in ticks:
        y = y_of(value)
        parts.append(
            f'<line x1="{PAD_L}" y1="{y:.1f}" x2="{PAD_L + plot_w}" y2="{y:.1f}" '
            f'stroke="{MUTED}" stroke-opacity="0.25"/>'
        )
        label = f"{value // 1000}k" if value >= 1000 else str(value)
        parts.append(
            f'<text x="{PAD_L - 8}" y="{y + 4:.1f}" text-anchor="end">{label}</text>'
        )

    for i, label in month_ticks(series):
        parts.append(
            f'<text x="{x_of(i):.1f}" y="{HEIGHT - PAD_B + 18}" '
            f'text-anchor="middle">{label}</text>'
        )

    parts.append(f'<polygon points="{area}" fill="{ACCENT}" fill-opacity="0.12"/>')
    parts.append(
        f'<polyline points="{points}" fill="none" stroke="{ACCENT}" '
        'stroke-width="2" stroke-linejoin="round"/>'
    )
    parts.append(
        f'<text x="{PAD_L}" y="{PAD_T - 12}" font-size="13" fill="{MUTED}">'
        f"{REPO} · {peak:,} stars</text>"
    )
    parts.append("</g></svg>")
    return "\n".join(parts) + "\n"


def main() -> None:
    series = daily_cumulative(fetch_starred_at())

    with CSV_PATH.open("w", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(["date", "stars"])
        writer.writerows((day.isoformat(), count) for day, count in series)

    SVG_PATH.write_text(render(series))

    json.dump(
        {"stars": series[-1][1], "days": len(series), "through": series[-1][0].isoformat()},
        sys.stdout,
    )
    print()


if __name__ == "__main__":
    main()
