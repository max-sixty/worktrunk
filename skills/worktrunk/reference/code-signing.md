# Code Signing Policy

Worktrunk's SignPath Foundation application is under review. The Windows signing pipeline currently uses a self-signed test certificate and allows unsigned releases if signing fails.

## Why signing matters

[Authenticode](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/authenticode) signatures identify a binary's publisher and verify that it has not changed since signing.

## Signing certificate

> Free code signing provided by [SignPath.io](https://signpath.io/), certificate by [SignPath Foundation](https://signpath.org/).

SignPath holds the private key in a hardware security module; maintainers do not have access to it.

## What is signed

The signing pipeline covers `wt.exe` and `git-wt.exe` in the `x86_64-pc-windows-msvc` archive on [GitHub Releases](https://github.com/max-sixty/worktrunk/releases), also distributed through winget.

## Build and signing pipeline

The [`release` workflow](https://github.com/max-sixty/worktrunk/blob/main/.github/workflows/release.yaml) builds tagged commits with [cargo-dist](https://axodotdev.github.io/cargo-dist/) on GitHub-hosted runners. It submits the Windows archive to SignPath and publishes the signed archive when signing succeeds.

## Project roles

[@max-sixty](https://github.com/max-sixty) holds all signing roles:

- **Author**: commits code and tags releases.
- **Reviewer**: reviews external contributions before release.
- **Approver**: authorizes signing requests.

Maintainers with signing authority use multi-factor authentication on GitHub and SignPath.

## Release approval

Under this policy, each signing request requires manual approval after confirming that the artifact was built from the repository's tagged commit.

## Privacy

Worktrunk collects no telemetry or analytics. It transfers information to other systems only for operations requested or configured by the user.

## Reporting a problem

Report suspected tampering or antivirus detections on official release artifacts by [opening an issue](https://github.com/max-sixty/worktrunk/issues). Suspected Defender false positives can also be submitted to [Microsoft for review](https://www.microsoft.com/en-us/wdsi/filesubmission).
