---
description: Bump patch version, commit all changes, and create a GitHub release
---

When the user says "let's release a patch" or similar, perform the following steps:

1. Bump the patch version in Cargo.toml (e.g., 1.3.7 → 1.3.8)
2. Commit all changes with a descriptive commit message
3. Commit the version bump separately with message "Bump version to X.X.X"
4. Push all commits to GitHub master branch
5. Run `make github-release` to build binaries and create the GitHub release

This is a complete release workflow that should be executed as a sequence.
