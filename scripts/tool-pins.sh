# shellcheck shell=bash
# shellcheck disable=SC2034  # every pin is read by whichever setup sources this
#
# Pinned tool versions and archive checksums, sourced by the environment setups
# that install them: scripts/codex-cloud/codex.sh and the root Taskfile's
# `setup-web`. One home, so a bump reaches both.
#
# Checksums cover the exact archive each setup downloads. PowerShell has one for
# the tarball only; `setup-web` installs the .deb instead, because pwsh aborts
# without libicu and only the .deb declares that dependency for apt to resolve.

TASK_VERSION=3.52.0
TASK_SHA256=02c679ffae53dca791804847d78b31731615894e292948397c971c87ac9e95bd

INSTA_VERSION=1.48.0
INSTA_SHA256=1c05a480a5a7f755f0ea15b2d8e2f71ad51b9c3a270d38ac72005c57ac0a1487

NEXTEST_VERSION=0.9.143
NEXTEST_SHA256=66786b9abe23920d022a182d1416b1bbc8130dd4872a9553d76985a1708dcd1e

NU_VERSION=0.115.0
NU_SHA256=da83cfe482060d2c34b6b9af829975a313bce6b92e0398c3b2a59cb38630c7b2

PWSH_VERSION=7.6.5
PWSH_TARBALL_SHA256=b34ab3b19acac1d3d4d0d3cfdb02acf62f457b0b6a962ff008132033f7566844

PRE_COMMIT_VERSION=4.6.2
