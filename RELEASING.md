# Releasing bRAGa

Releases are cut by pushing a `v*` tag. `.github/workflows/release.yml` then builds every artifact
and attaches them to a GitHub Release. Registry publishing is wired but dormant: each publish job is
skipped when its secrets are absent (see [Publishing secrets](#publishing-secrets)), so the pipeline
needs no secrets to produce a complete, green release.

## Cut a release

1. Bump the version everywhere it is declared, plus the changelog:
   - `Cargo.toml` (`[workspace.package] version`)
   - `python/pyproject.toml` (`[project] version`)
   - `jvm/build.gradle` (`version`)
   - `dotnet/src/Braga/Braga.csproj` (`<Version>`)
   - `CHANGELOG.md` (move `[Unreleased]` under the new version)
2. Commit the bump on `main` and tag it:

   ```sh
   git tag -a v0.1.0 -m "v0.1.0"
   git push origin v0.1.0
   ```

3. Watch the **Release** workflow. It fires on the tag and creates the GitHub Release at
   `https://github.com/artemidius/bRAGa/releases`.

A manual `workflow_dispatch` run builds and uploads every artifact to the workflow run, but does not
create a GitHub Release: a Release must attach to a tag.

## What gets built

| artifact | job | notes |
| --- | --- | --- |
| `braga-macos-arm64`, `braga-linux-x64`, `braga-windows-x64.exe` | `native` | platform-suffixed CLI binaries; also smoke-tested with `--version` |
| `braga_ffi` cdylib (`.dylib` / `.so` / `.dll`) | `native` | the frozen C ABI both wrappers load |
| platform-tagged Python wheels | `python-wheels` | cibuildwheel, `cp310`–`cp313`, macOS/Linux/Windows |
| JVM jars | `jvm` | `linux-x64` and `macos-arm64`; the Windows jar is deferred until `build.gradle` has a Windows path |
| .NET nupkg | `dotnet` | contains all three RIDs (`osx-arm64`, `linux-x64`, `win-x64`) |

The release ships wheels only, no sdist: a source tarball would lack the Cargo workspace and cdylib, so
it cannot be installed standalone. Installing from a checkout (`pip install ./python`) still works
where `cargo` is available and compiles the cdylib on demand.

The tag itself is the Go release (no artifact is published to the Go module proxy).

Musl/Alpine is not built: no musl cdylib exists, so Alpine users install from source.

## Publishing secrets

Publishing is off by default: `release.yml` runs a `detect-secrets` job that exposes only booleans
for secret presence, and each publish job is skipped when its secrets are absent. A fork, a
token-less tag push, or a dry `workflow_dispatch` therefore still produces a complete, green release
with every artifact on the GitHub Release. Set secrets under
**Settings → Secrets and variables → Actions**.

| secret(s) | publish job | registry | absent → |
| --- | --- | --- | --- |
| `PYPI_API_TOKEN` | `pypi-publish` | PyPI | job skipped |
| `NUGET_API_KEY` | `nuget-publish` | NuGet.org | job skipped |
| `MAVEN_USERNAME` + `MAVEN_PASSWORD` | `maven-publish` | Maven Central | job skipped |
| `GPG_PRIVATE_KEY` + `GPG_PASSPHRASE` | `maven-publish` | Maven Central (signing) | job skipped |

- `PYPI_API_TOKEN` is a PyPI API token scoped to the `braga` project.
- `NUGET_API_KEY` is a NuGet.org API key with push permission.
- `MAVEN_USERNAME` / `MAVEN_PASSWORD` are Sonatype Central Portal credentials.
- `GPG_PRIVATE_KEY` is an ASCII-armored private key and `GPG_PASSPHRASE` its passphrase. `build.gradle`
  signs only when the key is present, so `./gradlew build` and `./gradlew check` work with no GPG
  setup.

The publish jobs run on `ubuntu-latest`, so the Maven artifact is the linux-x64 jar; the macOS arm64
jar remains a GitHub Release asset. `./gradlew publish` signs (when `GPG_PRIVATE_KEY` is set) and
uploads the jar to Sonatype's OSSRH-compatible staging endpoint. A full Maven Central release still
needs sources/javadoc artifacts and the Portal release step, so JitPack — which builds `jvm/` from the
tag with no secrets — remains the supported Maven channel until that lands.

## Out of scope / follow-ups

- **Swift `.xcframework`** — not produced. The local toolchain is broken (Xcode 16.2 on macOS 26) and
  distribution needs an Apple Developer identity plus notarization. Swift releases will be a separate
  workflow.
- **crates.io** — the Apache crates are not published yet. `braga-extra` is **AGPL-3.0-or-later** and
  must never be published by this Apache-2.0 pipeline; publishing it is a separate, deliberate
  follow-up.
- **Maven Central** — `./gradlew publish` uploads a signed jar to Sonatype's OSSRH-compatible staging
  endpoint. Completing a Central release (sources/javadoc artifacts, Portal upload) is a follow-up;
  until then JitPack builds `jvm/` from the tag with no secrets. Only the linux-x64 jar is published;
  the macOS arm64 jar is a GitHub Release asset.
