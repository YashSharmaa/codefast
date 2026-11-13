# Releasing

1. Set the new version in `Cargo.toml`, then repeat it wherever
   `python3 tests/packaging/manifests/validate.py` says it differs (the manifests,
   `bin/codefast`, `server.json` and the packages under `npm/`). Build once so that
   `Cargo.lock` follows.
2. Run `cargo test --release` and the packaging checks in `packaging.md`.
3. Update the numbers in `README.md` from `bench/results/` if a benchmark changed.
4. Commit, push, and wait for the `ci` workflow to pass.
5. Tag `vX.Y.Z` and push the tag. The `release` workflow builds the four targets and
   publishes them with `SHA256SUMS`. A release stays a draft until every asset is attached.
6. The `install-check` workflow then installs the published release on Linux (x86-64 and
   arm64) and macOS (Apple silicon and Intel). The release is done when it is green.

Never move a tag that people may already have installed from. Publish a new version instead.
