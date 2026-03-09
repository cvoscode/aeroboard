# Building and Releasing Aeroboard

Aeroboard now ships as a pure Python package plus release-managed runtime artifacts.

## Build outputs

Each release produces three artifact classes:

1. Python distributions: `aeroboard-x.y.z.tar.gz` and `aeroboard-x.y.z-py3-none-any.whl`
2. Web bundle: `aeroboard-web-vx.y.z.tar.gz`
3. Server binaries:
   - `aeroboard-server-vx.y.z-linux-x86_64.tar.gz`
   - `aeroboard-server-vx.y.z-linux-aarch64.tar.gz`
   - `aeroboard-server-vx.y.z-macos-x86_64.tar.gz`
   - `aeroboard-server-vx.y.z-macos-aarch64.tar.gz`
   - `aeroboard-server-vx.y.z-windows-x86_64.zip`

## What gets compiled

The release pipeline compiles these components:

1. The React dashboard once with `npm install && npm run build`
2. The Rust server once per target platform with `cargo build --release`
3. The Python wheel and sdist with `uv build`

The published Python package does not embed platform binaries. Instead, the CLI downloads the matching server and web artifacts from the GitHub release for the installed package version on first use.

When installing directly from GitHub into a Python environment, prefer a tagged ref such as `git+https://github.com/cvoscode/aeroboard.git@v0.1.0` so the installed package version and release artifacts stay aligned.

## Local development builds

For repository development, use:

```bash
uv sync --extra examples --dev
uv run aeroboard setup --build-from-source
```

That path keeps the inner loop local and avoids downloading release artifacts.

## Release flow

1. Update the version in `pyproject.toml`, `aeroboard-client/src/aeroboard_client/_version.py`, and `aeroboard-server/Cargo.toml`
2. Push a git tag named `vX.Y.Z`
3. `.github/workflows/release.yml` builds and uploads the web bundle, server binaries, and Python distributions to the GitHub release
4. `.github/workflows/publish-pypi.yml` publishes the Python distributions to PyPI

## Runtime environment variables

These overrides are useful for testing unpublished artifacts:

1. `AEROBOARD_GITHUB_REPOSITORY`: alternate `owner/repo` for release downloads
2. `AEROBOARD_RELEASE_BASE_URL`: fully qualified base URL for release assets
3. `AEROBOARD_SERVER_BINARY`: explicit path to a server binary
4. `AEROBOARD_WEB_DIST_DIR`: explicit path to a built frontend `dist` directory