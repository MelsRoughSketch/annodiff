# CI and crates.io releases

CI runs on pushes to `main`, pull requests, and manual dispatches. Feature branch
pushes are checked through their PR, avoiding duplicate runs. It checks Rust
formatting, Clippy, unit/integration tests, CLI and PTY interactions, and license
notices on Linux. A separate job checks all targets and builds the packaged crate
with the minimum supported Rust version, 1.88.0. Benchmarks are compiled, but
timing results are not used as CI thresholds. The live Codex test stays ignored.

## Dependabot auto-merge

`dependabot_merge.yml` enables auto-merge for Dependabot patch updates to `main`.
Minor and major updates require manual merging. It uses `GITHUB_TOKEN` and does
not check out or execute pull request code.

Before enabling it, configure branch protection or a ruleset for `main` to require
the `Lint and tests` and `Rust 1.88 and package` status checks. Then enable
Settings → General → Pull Requests → Allow auto-merge, keeping merge commits
enabled. Without required checks, auto-merge can merge before CI finishes.
Private repositories may need a paid GitHub plan for branch protection; otherwise,
complete this setup after making the repository public.

Cargo updates may fail the license-notice check until `THIRD_PARTY_NOTICES.md` is
regenerated with `python3 licenses/generate.py` and reviewed. Push that change to
the update branch; auto-merge waits for the required checks to pass. Required
reviews still need human approval.

See [GitHub's Dependabot automation documentation](https://docs.github.com/en/code-security/tutorials/secure-your-dependencies/automate-dependabot-with-actions).

## One-time setup

1. Sign in to crates.io and verify your email. For a new crate, publish its first
   version locally from a clean, committed checkout after CI succeeds:

   ```sh
   cargo login
   cargo publish --dry-run --locked
   cargo publish --locked
   ```

   `cargo login` prompts for a crates.io API token. Use a short-lived token with
   permission to publish `annodiff`; revoke it after this initial publication.
   Do not put it in this repository or GitHub secrets.

2. In GitHub repository Settings → Environments, create `crates-io`.

3. In the `annodiff` crate settings on crates.io, add a GitHub Trusted Publisher:

   | Field | Value |
   |---|---|
   | Repository owner | `MelsRoughSketch` |
   | Repository name | `annodiff` |
   | Workflow filename | `publish.yml` |
   | Environment | `crates-io` |

   See the [official Trusted Publishing documentation](https://crates.io/docs/trusted-publishing).
   The workflow exchanges GitHub's identity for a temporary publishing token;
   no persistent crates.io secret is required.

The automated workflow is for subsequent versions. Creating a GitHub Release
for the already-published first version will attempt to publish it again and
fail; do not use that as the first test of this workflow.

## Subsequent releases

1. Bump `version` in `Cargo.toml` and update `Cargo.lock` with `cargo check`.
   If dependencies changed, regenerate notices with
   `python3 licenses/generate.py` and review them.
2. Commit and push the changes, then wait for CI to pass.
3. Create a GitHub Release using a tag matching the manifest, e.g. `v0.1.1`,
   pointing to that commit. Publish it as a regular release.

Publishing the release reruns CI on the tagged commit. Only after both CI jobs
succeed does `publish.yml` check the version, run a publication dry run,
authenticate, and publish to crates.io. Drafts, prereleases, and tag pushes alone
do not publish a crate. No GitHub binary assets are generated.

A published crate version cannot be overwritten. If a run fails before upload,
fix the cause and rerun it; if upload succeeded, check crates.io before retrying.
