# Code Signing Policy

This page is Worktrunk's **code signing policy**. It describes what gets signed, which certificate is used, how the signing pipeline works, and who authorizes each release. It exists both to document the process for users and to satisfy the transparency requirements of the [SignPath Foundation](https://signpath.org/) open-source code signing program.

## Why signing matters

Worktrunk's Windows binaries (`wt.exe` and `git-wt.exe`) are small, native executables. Microsoft Defender's machine-learning heuristics routinely flag unsigned native executables of this shape as generic threats (for example `Trojan:Win32/Wacatac.B!ml`) — a false positive driven by the *absence of a trusted signature*, not by anything in the code. A valid [Authenticode](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/authenticode) signature gives Defender's cloud something signed to trust, which is what stops that class of false positive. Signing the Windows artifacts is the durable fix.

The macOS and Linux release artifacts, the crates.io source distribution, and `cargo install` builds are unaffected by this policy — `cargo install` compiles locally from source and never downloads a pre-built artifact.

## Signing certificate

> Free code signing provided by [SignPath.io](https://signpath.io/), certificate by [SignPath Foundation](https://signpath.org/).

The certificate's private key is generated and held on SignPath's Hardware Security Module (HSM); no maintainer ever possesses the private key. Signing happens only through SignPath's service, invoked from Worktrunk's release pipeline.

## What is signed

- `wt.exe` and `git-wt.exe` — the Windows binaries shipped in the `x86_64-pc-windows-msvc` release archive on each [GitHub Release](https://github.com/max-sixty/worktrunk/releases) and distributed via [winget](https://github.com/microsoft/winget-pkgs) (`winget install max-sixty.worktrunk`). `git-wt.exe` is the same program built as a git subcommand.

Nothing else is signed under this policy. Signed artifacts contain only code built from this repository; any bundled third-party libraries are used unmodified.

## Build and signing pipeline

1. Every release is triggered by pushing a version tag to [`max-sixty/worktrunk`](https://github.com/max-sixty/worktrunk). No release is built from any other source.
2. The [`release` workflow](https://github.com/max-sixty/worktrunk/blob/main/.github/workflows/release.yaml) builds the platform binaries with [cargo-dist](https://axodotdev.github.io/cargo-dist/) on GitHub-hosted runners, from the tagged commit only.
3. The Windows binary is submitted to SignPath for signing via the [SignPath GitHub Action](https://github.com/SignPath/github-action-submit-signing-request). The signed binary is returned to the workflow and published in the release archive.
4. The build is reproducible from public source: the workflow, the toolchain pin ([`rust-toolchain.toml`](https://github.com/max-sixty/worktrunk/blob/main/rust-toolchain.toml)), and the release configuration ([`dist-workspace.toml`](https://github.com/max-sixty/worktrunk/blob/main/dist-workspace.toml)) are all tracked in this repository.

## Project roles

Worktrunk follows SignPath's team model:

| Role | Responsibility | Who |
|------|----------------|-----|
| **Author** | Trusted developer who commits and tags releases | [@max-sixty](https://github.com/max-sixty) |
| **Reviewer** | Reviews contributions from outside the trusted set before they can be released | [@max-sixty](https://github.com/max-sixty) |
| **Approver** | Authorizes each signing request for a release | [@max-sixty](https://github.com/max-sixty) |

All maintainers with signing authority use multi-factor authentication on their GitHub and SignPath accounts.

## Release approval

Every signing request requires **manual approval** by an Approver before the certificate is applied — signing is never fully automated. A release is signed only after the Approver has confirmed the artifact was built from the tagged commit in this repository.

## Privacy

Worktrunk is a local-first command-line tool. It performs no telemetry, collects no analytics, and transmits no user data. Network access happens only when a command the user ran needs it (for example, fetching CI status for `wt list --full`); the [FAQ](https://worktrunk.dev/faq/) and inline documentation describe exactly when. The signed binaries add no data collection of any kind.

## Reporting a problem

If you believe a signed Worktrunk binary has been tampered with, or you receive an antivirus detection on an official release artifact, please [open an issue](https://github.com/max-sixty/worktrunk/issues). For a suspected Defender false positive, you can also report the file to Microsoft through the [Windows Defender submission portal](https://www.microsoft.com/en-us/wdsi/filesubmission) (as *"I believe this file is safe"*), which corrects the cloud definition for every user.
