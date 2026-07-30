import importlib.util
from pathlib import Path

import pytest

SCRIPT = Path(__file__).with_name("criterion-to-jsonl.py")
SPEC = importlib.util.spec_from_file_location("criterion_to_jsonl", SCRIPT)
criterion_to_jsonl = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(criterion_to_jsonl)


def test_rows_include_the_fixture_identity(tmp_path: Path) -> None:
    manifest = tmp_path / "fixture"
    manifest.write_text(
        "schema=1\n"
        "corpus=example/project\n"
        "revision=0123456789abcdef0123456789abcdef01234567\n"
    )
    estimate = tmp_path / "criterion" / "group" / "case" / "new" / "estimates.json"
    estimate.parent.mkdir(parents=True)
    estimate.write_text(
        '{"mean":{"point_estimate":12.5},"std_dev":{"point_estimate":3.25}}'
    )

    fixture = criterion_to_jsonl.parse_fixture_manifest(manifest)
    assert criterion_to_jsonl.criterion_rows(
        tmp_path / "criterion", "abc123", "2026-07-30T00:00:00Z", fixture
    ) == [
        {
            "ts": "2026-07-30T00:00:00Z",
            "sha": "abc123",
            "bench": "group/case",
            "mean_ns": 12.5,
            "stddev_ns": 3.25,
            "fixture_schema": 1,
            "fixture_corpus": "example/project",
            "fixture_revision": "0123456789abcdef0123456789abcdef01234567",
        }
    ]


@pytest.mark.parametrize(
    "content, message",
    [
        (
            "schema=1\ncorpus=a/b\ncorpus=c/d\nrevision=0123456789abcdef0123456789abcdef01234567\n",
            "duplicate corpus",
        ),
        ("schema=1\ncorpus=a/b\nrevision=short\n", "40-character hexadecimal"),
        (
            "schema=2\ncorpus=a/b\nrevision=0123456789abcdef0123456789abcdef01234567\n",
            "unsupported schema 2",
        ),
    ],
)
def test_manifest_rejects_ambiguous_identities(
    tmp_path: Path, content: str, message: str
) -> None:
    manifest = tmp_path / "fixture"
    manifest.write_text(content)

    with pytest.raises(ValueError, match=message):
        criterion_to_jsonl.parse_fixture_manifest(manifest)
