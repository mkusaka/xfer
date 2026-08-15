---
name: release-xfer
description: Release the xfer CLI to GitHub and mkusaka/homebrew-tap. Use when the user asks to prepare, publish, monitor, repair, or verify an xfer version release, including Cargo.toml version alignment, exact-head CI, vX.Y.Z tags, GitHub Release archives, packaged skills, and the Homebrew Formula update.
---

# Release xfer

Release from `main` through `.github/workflows/release.yml`. Treat the Git tag, GitHub Release, release archives, and Homebrew Formula as separate states.

## Prepare the version

1. Fetch `origin/main` and tags with `git` and inspect the working tree.
2. Read the package version from `Cargo.toml` and existing semantic tags.
3. Require the release tag to be exactly `v<package-version>`. Never move or replace an existing tag.
4. If changing the version, inspect recent commit messages before committing. Stage only intended files, run the repository-required secret scan, then commit in repository style.
5. Confirm `HOMEBREW_TAP_TOKEN` exists without reading its value:

```bash
gh secret list --repo mkusaka/xfer | rg '^HOMEBREW_TAP_TOKEN\b'
```

## Gate on exact-head CI

Push `main`, read back its SHA, and wait for the `CI` run whose `headSha` matches it. Do not tag while that run is pending or failing.

```bash
git push origin main
git ls-remote origin refs/heads/main
gh run list --repo mkusaka/xfer --branch main --limit 10 \
  --json databaseId,headSha,workflowName,status,conclusion,url
gh run watch <run-id> --repo mkusaka/xfer --exit-status
```

## Publish and monitor

```bash
git tag -a "v${VERSION}" -m "xfer v${VERSION}"
git push origin "v${VERSION}"
gh run list --repo mkusaka/xfer --workflow Release --limit 10 \
  --json databaseId,headSha,status,conclusion,url
gh run watch <run-id> --repo mkusaka/xfer --exit-status
```

The workflow verifies Rust checks, creates a draft release, packages native Apple Silicon and Intel archives with all repository skills, publishes the release, and dispatches `Formula/xfer.rb` to `mkusaka/homebrew-tap`.

## Verify delivery

Do not report completion until all facts are read back:

```bash
gh release view "v${VERSION}" --repo mkusaka/xfer \
  --json tagName,isDraft,url,assets
gh api repos/mkusaka/homebrew-tap/contents/Formula/xfer.rb \
  --jq '.content' | base64 --decode
```

Require a successful exact-tag Release run, a non-draft release, both `darwin-arm64` and `darwin-x64` archives, and Formula version and URLs matching the release. If delivery fails, inspect `gh run view <run-id> --log-failed`; do not delete drafts, force-push, or retag automatically.
