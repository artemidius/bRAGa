# Releasing bRAGa

Releases are cut by pushing a `v*` tag. `.github/workflows/release.yml` then builds every artifact
and attaches them to a GitHub Release. Registry publishing is a follow-up (see the end of this file);
the pipeline itself needs no secrets.

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

## Secrets

None are required for the build and GitHub Release. Future registry publishing should gate each job
on `secrets.X != ''` so a fork or a token-less run stays green:

| secret(s) | registry |
| --- | --- |
| `PYPI_TOKEN` | PyPI |
| `NUGET_API_KEY` | NuGet.org |
| `MAVEN_USERNAME` + `MAVEN_PASSWORD` (+ GPG key for Central) | Maven Central |

## Out of scope / follow-ups

- **Swift `.xcframework`** — not produced. The local toolchain is broken (Xcode 16.2 on macOS 26) and
  distribution needs an Apple Developer identity plus notarization. Swift releases will be a separate
  workflow.
- **crates.io** — the Apache crates are not published yet. `braga-extra` is **AGPL-3.0-or-later** and
  must never be published by this Apache-2.0 pipeline; publishing it is a separate, deliberate
  follow-up.
- **Registry publish jobs** — PyPI, NuGet, and Maven publishing are not yet wired into `release.yml`.
  Until Maven Central signing lands, Maven consumers can use JitPack, which builds from the tag with
  no secrets.
