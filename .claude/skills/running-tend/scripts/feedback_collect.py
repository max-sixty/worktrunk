#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# ///
"""Collect public mentions of Worktrunk for the weekly feedback digest.

Finds the open issue labeled `feedback-digest`, reads the state marker the
newest digest comment ends with, and queries each source from where that
source left off up to now. Writes to `$TMPDIR/feedback-digest/`:

- `items.jsonl`: one record per mention, with `source`, `kind`, `url`,
  `created_at`, `author`, `where`, `title`, `excerpt`, and `thread` for a
  comment on an older thread.
- `summary.json`: the tracking issue number (null before the first digest),
  each source's window, status and item count, and `marker`, the line the new
  digest ends with.
- `previous_digests.md`: the earlier digests' text, for checking whether a URL
  was already reported.

A source whose request fails keeps its old start in the marker, so the next
run covers its gap; a source that answers with zero results advances.
"""

from __future__ import annotations

import html
import json
import os
import re
import subprocess
import sys
import time
import urllib.parse
import urllib.request
from collections.abc import Callable
from dataclasses import dataclass
from datetime import UTC, datetime, timedelta
from pathlib import Path

QUERY = "worktrunk"
LABEL = "feedback-digest"
MARKER_RE = re.compile(r"<!-- feedback-digest-state (\{.*?\}) -->")
# With no earlier digest, every source starts this far back.
FIRST_WINDOW = timedelta(days=7)
# GitHub search serves at most 1000 results per query; a window holding more is
# split in half until each half fits.
SEARCH_CAP = 1000
# GitHub allows 30 search requests a minute.
SEARCH_INTERVAL = 2.1
USER_AGENT = "worktrunk-feedback-digest"
MAINTAINER_ASSOCIATIONS = {"OWNER", "MEMBER", "COLLABORATOR"}


@dataclass(frozen=True)
class Context:
    repo: str
    owner: str
    me: str

    def excluded(self, user: dict | None) -> bool:
        """Bots, and this project's owner and bot, whose words aren't outside feedback."""
        if not user:
            return True
        login = user.get("login") or ""
        return (
            user.get("type") == "Bot"
            or login.endswith("[bot]")
            or login in {self.owner, self.me}
        )


def gh(*args: str) -> str:
    for attempt in range(4):
        proc = subprocess.run(["gh", *args], capture_output=True, text=True)
        if proc.returncode == 0:
            return proc.stdout
        if "rate limit" in proc.stderr.lower() and attempt < 3:
            time.sleep(60)
            continue
        raise RuntimeError(f"gh {' '.join(args[:3])}: {proc.stderr.strip()[:300]}")
    raise AssertionError("unreachable")


_last_search = 0.0


def search(
    endpoint: str, query: str, qualifier: str, since: datetime, until: datetime
) -> list[dict]:
    """All results of a GitHub search over `qualifier:since..until`."""
    global _last_search
    q = f"{query} {qualifier}:{iso(since)}..{iso(until)}"
    items: list[dict] = []
    page = 1
    while True:
        time.sleep(max(0.0, _last_search + SEARCH_INTERVAL - time.monotonic()))
        _last_search = time.monotonic()
        data = json.loads(
            gh(
                "api",
                "-X",
                "GET",
                f"search/{endpoint}",
                "-H",
                "Accept: application/vnd.github.text-match+json",
                "-f",
                f"q={q}",
                "-f",
                "per_page=100",
                "-f",
                f"page={page}",
            )
        )
        total = data["total_count"]
        if total > SEARCH_CAP:
            if until - since < timedelta(minutes=2):
                raise RuntimeError(f"{total} results in under two minutes for {q!r}")
            # Both halves include the second at `mid`.
            mid = since + (until - since) / 2
            halves = search(endpoint, query, qualifier, since, mid) + search(
                endpoint, query, qualifier, mid, until
            )
            return list({i["url"]: i for i in halves}.values())
        if data["incomplete_results"]:
            raise RuntimeError(f"search/{endpoint} timed out on {q!r}")
        items += data["items"]
        if len(items) >= min(total, SEARCH_CAP) or not data["items"]:
            return items
        page += 1


def http_json(url: str):
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(req, timeout=30) as resp:
        return json.load(resp)


def iso(dt: datetime) -> str:
    return dt.astimezone(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def parse_time(value: str) -> datetime:
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def in_window(value: str, since: datetime, until: datetime) -> bool:
    return since <= parse_time(value) < until


def excerpt(text: str | None, width: int = 400) -> str:
    """The text around the first mention of QUERY, whitespace collapsed."""
    text = re.sub(r"\s+", " ", text or "").strip()
    i = text.lower().find(QUERY)
    start = max(0, i - width // 2) if i >= 0 else 0
    clip = text[start : start + width]
    return ("…" if start else "") + clip + ("…" if start + width < len(text) else "")


def strip_html(text: str | None) -> str:
    return html.unescape(re.sub(r"<[^>]+>", " ", text or ""))


def text_match_excerpt(item: dict) -> str:
    fragments = [m["fragment"] for m in item.get("text_matches", [])]
    return excerpt(" … ".join(fragments) if fragments else item.get("body"))


def repo_of(api_url: str) -> str:
    return "/".join(api_url.split("/")[-2:])


def issue_record(source: str, item: dict) -> dict:
    return {
        "source": source,
        "kind": "pr" if "pull_request" in item else "issue",
        "url": item["html_url"],
        "created_at": item["created_at"],
        "author": item["user"]["login"],
        "where": repo_of(item["repository_url"]),
        "title": item["title"],
        "excerpt": text_match_excerpt(item),
    }


def tracker(ctx: Context, since: datetime, until: datetime) -> list[dict]:
    """New issues in this repo from people outside the project."""
    items = search("issues", f"repo:{ctx.repo} is:issue", "created", since, until)
    return [
        issue_record("tracker", i)
        for i in items
        if not ctx.excluded(i["user"])
        and i["author_association"] not in MAINTAINER_ASSOCIATIONS
    ]


def github_issues(ctx: Context, since: datetime, until: datetime) -> list[dict]:
    query = f"{QUERY} -user:{ctx.owner} is:issue"
    items = search("issues", query, "created", since, until)
    return [issue_record("github-issues", i) for i in items if not ctx.excluded(i["user"])]


def github_prs(ctx: Context, since: datetime, until: datetime) -> list[dict]:
    query = f"{QUERY} -user:{ctx.owner} is:pr"
    items = search("issues", query, "created", since, until)
    return [issue_record("github-prs", i) for i in items if not ctx.excluded(i["user"])]


def github_commits(ctx: Context, since: datetime, until: datetime) -> list[dict]:
    query = f"{QUERY} -user:{ctx.owner}"
    records: dict[str, dict] = {}
    for c in search("commits", query, "committer-date", since, until):
        if c["repository"]["fork"] or c["sha"] in records:
            continue
        if c["author"] and ctx.excluded(c["author"]):
            continue
        records[c["sha"]] = {
            "source": "github-commits",
            "kind": "commit",
            "url": c["html_url"],
            "created_at": c["commit"]["committer"]["date"],
            "author": (c["author"] or {}).get("login") or c["commit"]["author"]["name"],
            "where": c["repository"]["full_name"],
            "title": c["commit"]["message"].split("\n", 1)[0],
            "excerpt": excerpt(c["commit"]["message"]),
        }
    return list(records.values())


def github_comments(ctx: Context, since: datetime, until: datetime) -> list[dict]:
    """New conversation comments mentioning Worktrunk, on any thread.

    Inline PR review comments aren't read: finding them takes a request per PR.
    """
    query = f"{QUERY} -user:{ctx.owner}"
    # A thread's `updated_at` moves with every later activity, so the search
    # runs to now: bounding it at `until` would drop a thread updated again
    # since then, and with it a comment posted inside the window.
    threads = search("issues", query, "updated", since, datetime.now(UTC) + timedelta(hours=1))
    records = []
    for thread in threads:
        if not thread["comments"]:
            continue
        repo = repo_of(thread["repository_url"])
        lines = gh(
            "api",
            "--paginate",
            f"repos/{repo}/issues/{thread['number']}/comments?since={iso(since)}&per_page=100",
            "--jq",
            ".[] | tojson",
        )
        for line in lines.splitlines():
            c = json.loads(line)
            if (
                in_window(c["created_at"], since, until)
                and QUERY in (c["body"] or "").lower()
                and not ctx.excluded(c["user"])
            ):
                records.append(
                    {
                        "source": "github-comments",
                        "kind": "comment",
                        "url": c["html_url"],
                        "created_at": c["created_at"],
                        "author": c["user"]["login"],
                        "where": repo,
                        "title": thread["title"],
                        "excerpt": excerpt(c["body"]),
                        "thread": thread["html_url"],
                    }
                )
    return records


def hacker_news(ctx: Context, since: datetime, until: datetime) -> list[dict]:
    params = urllib.parse.urlencode(
        {
            "query": QUERY,
            # Without this, Algolia also matches near-spellings such as
            # "WorkTrackers".
            "typoTolerance": "false",
            "numericFilters": f"created_at_i>={int(since.timestamp())},"
            f"created_at_i<{int(until.timestamp())}",
            "hitsPerPage": 1000,
        }
    )
    data = http_json(f"https://hn.algolia.com/api/v1/search_by_date?{params}")
    if data["nbHits"] > len(data["hits"]):
        raise RuntimeError(f"{data['nbHits']} hits, more than one request returns")
    records = []
    for hit in data["hits"]:
        is_comment = "comment" in hit["_tags"]
        text = strip_html(hit.get("comment_text") or hit.get("story_text"))
        records.append(
            {
                "source": "hacker-news",
                "kind": "comment" if is_comment else "story",
                "url": f"https://news.ycombinator.com/item?id={hit['objectID']}",
                "created_at": hit["created_at"],
                "author": hit["author"],
                "where": "Hacker News",
                "title": hit.get("story_title") or hit.get("title") or "",
                "excerpt": excerpt(text) if text else hit.get("url") or "",
                **(
                    {"thread": f"https://news.ycombinator.com/item?id={hit['story_id']}"}
                    if is_comment
                    else {}
                ),
            }
        )
    return records


def zenn(ctx: Context, since: datetime, until: datetime) -> list[dict]:
    records = []
    page = 1
    while page:
        params = urllib.parse.urlencode(
            {"q": QUERY, "source": "articles", "order": "latest", "page": page}
        )
        data = http_json(f"https://zenn.dev/api/search?{params}")
        for a in data["articles"]:
            if in_window(a["published_at"], since, until):
                records.append(
                    {
                        "source": "zenn",
                        "kind": "article",
                        "url": f"https://zenn.dev{a['path']}",
                        "created_at": a["published_at"],
                        "author": a["user"]["username"],
                        "where": "Zenn",
                        "title": a["title"],
                        "excerpt": f"{a['liked_count']} likes",
                    }
                )
        oldest = min((parse_time(a["published_at"]) for a in data["articles"]), default=since)
        page = data["next_page"] if oldest >= since else None
    return records


def qiita(ctx: Context, since: datetime, until: datetime) -> list[dict]:
    query = f"{QUERY} created:>={since.date().isoformat()}"
    records = []
    page = 1
    while True:
        params = urllib.parse.urlencode({"query": query, "per_page": 100, "page": page})
        articles = http_json(f"https://qiita.com/api/v2/items?{params}")
        records += [
            {
                "source": "qiita",
                "kind": "article",
                "url": a["url"],
                "created_at": a["created_at"],
                "author": a["user"]["id"],
                "where": "Qiita",
                "title": a["title"],
                "excerpt": excerpt(a["body"]),
            }
            for a in articles
            if in_window(a["created_at"], since, until)
        ]
        if len(articles) < 100:
            return records
        page += 1


SOURCES: dict[str, Callable[[Context, datetime, datetime], list[dict]]] = {
    "tracker": tracker,
    "github-issues": github_issues,
    "github-prs": github_prs,
    "github-commits": github_commits,
    "github-comments": github_comments,
    "hacker-news": hacker_news,
    "zenn": zenn,
    "qiita": qiita,
}


def main() -> None:
    out = Path(os.environ.get("TMPDIR", "/tmp")) / "feedback-digest"
    out.mkdir(parents=True, exist_ok=True)

    repo = json.loads(gh("repo", "view", "--json", "nameWithOwner"))["nameWithOwner"]
    ctx = Context(repo=repo, owner=repo.split("/")[0], me=json.loads(gh("api", "user"))["login"])
    issues = json.loads(
        gh("issue", "list", "--repo", repo, "--label", LABEL, "--state", "open", "--json", "number")
    )
    issue = issues[0]["number"] if issues else None

    digests: list[str] = []
    if issue is not None:
        lines = gh(
            "api",
            "--paginate",
            f"repos/{repo}/issues/{issue}/comments?per_page=100",
            "--jq",
            f'.[] | select(.user.login == "{ctx.me}") | .body | tojson',
        )
        digests = [b for b in map(json.loads, lines.splitlines()) if MARKER_RE.search(b)]
    state = json.loads(MARKER_RE.findall(digests[-1])[-1]) if digests else {}
    (out / "previous_digests.md").write_text("\n\n---\n\n".join(digests))

    now = datetime.now(UTC).replace(microsecond=0)
    items: list[dict] = []
    sources = {}
    next_state = {}
    for name, collect in SOURCES.items():
        since = parse_time(state[name]) if name in state else now - FIRST_WINDOW
        window = {"since": iso(since), "until": iso(now)}
        try:
            found = collect(ctx, since, now)
        except Exception as e:
            sources[name] = {**window, "ok": False, "error": f"{type(e).__name__}: {e}"}
            next_state[name] = iso(since)
            continue
        items += found
        sources[name] = {**window, "ok": True, "items": len(found)}
        next_state[name] = iso(now)

    with (out / "items.jsonl").open("w") as f:
        for record in items:
            f.write(json.dumps(record, ensure_ascii=False) + "\n")
    summary = {
        "issue": issue,
        "previous_digests": len(digests),
        "sources": sources,
        "marker": "<!-- feedback-digest-state "
        + json.dumps(next_state, sort_keys=True, separators=(",", ":"))
        + " -->",
    }
    (out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(f"Wrote {len(items)} items to {out}/items.jsonl")
    print(json.dumps(summary, indent=2))
    if not any(s["ok"] for s in sources.values()):
        sys.exit(1)


if __name__ == "__main__":
    main()
