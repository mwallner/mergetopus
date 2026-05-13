# Copilot Code Review Instructions

When reviewing pull requests in this repository, always check the following:

## CI / Actions

- Verify that **all GitHub Actions workflow runs triggered by the PR have completed successfully** (status: ✅ passed) before approving or leaving a positive review.
- If any workflow run has failed or is still in progress, explicitly call this out in the review and block approval until the failures are investigated and resolved.
- Check that the build passes on **all platforms** (Linux and Windows).

## Code Quality

- Ensure no unnecessary heap allocations (e.g. prefer `as_deref().unwrap_or(...)` over `.clone().unwrap_or_else(...)`).
- Verify that new dependencies or toolchain version pins are compatible with each other and with the CI environment.
- Flag any hardcoded version constraints (e.g. in `rust-toolchain.toml`) that may conflict with dependency requirements.
