"""Repackage release archives as PyPI wheels.

Each wheel carries the `wt` and `git-wt` binaries from one release archive,
byte for byte, so PyPI ships the same (signed, on Windows) binaries as GitHub
Releases. The wheels contain no Python code: installers place the binaries in
the environment's scripts directory, as maturin's `bindings = "bin"` does.

Usage: build-wheels.py --version 0.82.0 --out-dir wheels ARCHIVE...
"""

import argparse
import base64
import hashlib
import io
import re
import sys
import tarfile
import zipfile
from pathlib import Path

NAME = "worktrunk"
BINARIES = ("wt", "git-wt")

# Release target → wheel platform tag. The Linux binaries are static musl
# builds with no dynamic dependencies, so they qualify for both the glibc
# (manylinux) and musl (musllinux) tags. The macOS floors are Rust's default
# deployment targets.
PLATFORM_TAGS = {
    "x86_64-unknown-linux-musl": "manylinux_2_17_x86_64.manylinux2014_x86_64.musllinux_1_1_x86_64",
    "aarch64-unknown-linux-musl": "manylinux_2_17_aarch64.manylinux2014_aarch64.musllinux_1_1_aarch64",
    "x86_64-apple-darwin": "macosx_10_12_x86_64",
    "aarch64-apple-darwin": "macosx_11_0_arm64",
    "x86_64-pc-windows-msvc": "win_amd64",
}

ARCHIVE_RE = re.compile(r"^worktrunk-(?P<target>.+?)\.(?:tar\.xz|zip)$")

# PyPI renders the long description without a base URL, so relative links in
# the README (the logo) would break. Point them at the release tag instead.
RELATIVE_LINK_RE = re.compile(r'\b(src|href)="(?![a-z][a-z0-9+.-]*:|#|/)')

EXPERIMENTAL_NOTE = (
    "> **Experimental:** the PyPI package is new; GitHub Releases, Homebrew and "
    "cargo are the main install channels."
)

# Fixed timestamp so rebuilding the same archives yields identical wheels.
ZIP_DATE = (1980, 1, 1, 0, 0, 0)


def read_archive(path: Path) -> dict[str, bytes]:
    """Map each regular file's basename to its contents."""
    files = {}
    if path.name.endswith(".zip"):
        with zipfile.ZipFile(path) as zf:
            for info in zf.infolist():
                if not info.is_dir():
                    files[Path(info.filename).name] = zf.read(info)
    else:
        with tarfile.open(path, "r:xz") as tf:
            for member in tf.getmembers():
                if member.isfile():
                    files[Path(member.name).name] = tf.extractfile(member).read()
    return files


def record_hash(data: bytes) -> str:
    digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=")
    return f"sha256={digest.decode()}"


def metadata(version: str, readme: str, license_files: list[str]) -> str:
    headers = [
        "Metadata-Version: 2.4",
        f"Name: {NAME}",
        f"Version: {version}",
        "Summary: A CLI for Git worktree management, designed for parallel AI agent workflows",
        "License-Expression: MIT OR Apache-2.0",
        *(f"License-File: {name}" for name in license_files),
        "Project-URL: Homepage, https://worktrunk.dev",
        "Project-URL: Repository, https://github.com/max-sixty/worktrunk",
        "Project-URL: Changelog, https://github.com/max-sixty/worktrunk/blob/main/CHANGELOG.md",
        "Description-Content-Type: text/markdown",
    ]
    raw_base = f"https://raw.githubusercontent.com/max-sixty/worktrunk/v{version}/"
    readme = RELATIVE_LINK_RE.sub(lambda m: f'{m[1]}="{raw_base}', readme)
    return "\n".join(headers) + "\n\n" + EXPERIMENTAL_NOTE + "\n\n" + readme


def build_wheel(archive: Path, version: str, out_dir: Path) -> Path:
    match = ARCHIVE_RE.match(archive.name)
    if not match or match["target"] not in PLATFORM_TAGS:
        raise SystemExit(f"{archive.name}: not a known release archive")
    target = match["target"]
    platform = PLATFORM_TAGS[target]
    exe = ".exe" if target.endswith("-windows-msvc") else ""

    files = read_archive(archive)
    missing = [n for n in (*(b + exe for b in BINARIES), "README.md") if n not in files]
    # cargo-dist ships every top-level LICENSE* file: `LICENSE` up to v0.82.0,
    # `LICENSE-MIT` and `LICENSE-APACHE` after it.
    license_files = sorted(n for n in files if n.startswith("LICENSE"))
    if not license_files:
        missing.append("LICENSE*")
    if missing:
        raise SystemExit(f"{archive.name}: missing {', '.join(missing)}")

    data_dir = f"{NAME}-{version}.data"
    dist_info = f"{NAME}-{version}.dist-info"
    wheel_tag = f"py3-none-{platform}"
    # (path in wheel, contents, executable)
    entries = [(f"{data_dir}/scripts/{b}{exe}", files[b + exe], True) for b in BINARIES]
    entries += [
        (
            f"{dist_info}/METADATA",
            metadata(version, files["README.md"].decode(), license_files).encode(),
            False,
        ),
        *((f"{dist_info}/licenses/{name}", files[name], False) for name in license_files),
        (
            f"{dist_info}/WHEEL",
            "\n".join(
                [
                    "Wheel-Version: 1.0",
                    "Generator: worktrunk build-wheels.py",
                    "Root-Is-Purelib: false",
                    *(f"Tag: py3-none-{p}" for p in platform.split(".")),
                ]
            ).encode()
            + b"\n",
            False,
        ),
    ]
    record_path = f"{dist_info}/RECORD"
    record = "".join(f"{p},{record_hash(d)},{len(d)}\n" for p, d, _ in entries)
    record += f"{record_path},,\n"
    entries.append((record_path, record.encode(), False))

    out_dir.mkdir(parents=True, exist_ok=True)
    wheel_path = out_dir / f"{NAME}-{version}-{wheel_tag}.whl"
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as zf:
        for path, data, executable in entries:
            info = zipfile.ZipInfo(path, date_time=ZIP_DATE)
            info.external_attr = (0o100755 if executable else 0o100644) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            zf.writestr(info, data)
    wheel_path.write_bytes(buf.getvalue())
    return wheel_path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--version", required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    parser.add_argument("archives", nargs="+", type=Path)
    args = parser.parse_args()

    # Release tags like 0.82.0 are already valid PEP 440 versions; prerelease
    # suffixes would need normalizing, and prereleases aren't published.
    if not re.fullmatch(r"\d+\.\d+\.\d+", args.version):
        raise SystemExit(f"{args.version}: expected a MAJOR.MINOR.PATCH version")

    targets = {ARCHIVE_RE.match(a.name)["target"] for a in args.archives if ARCHIVE_RE.match(a.name)}
    if targets != set(PLATFORM_TAGS):
        raise SystemExit(f"expected archives for {sorted(PLATFORM_TAGS)}, got {sorted(targets)}")

    for archive in args.archives:
        print(build_wheel(archive, args.version, args.out_dir), file=sys.stderr)


if __name__ == "__main__":
    main()
