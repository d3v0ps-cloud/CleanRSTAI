#!/usr/bin/env bash
# Build, test and optionally publish a release from this machine, without GitHub Actions.
#
#   scripts/release-local.sh                 # checks + tests, then build every target this
#                                            # machine can build into dist/v<version>/
#   scripts/release-local.sh --publish       # ...then create the GitHub Release and push the
#                                            # files to GitHub Packages (ghcr.io)
#   scripts/release-local.sh --skip-tests    # build only
#
# The version comes from Cargo.toml; the tag is v<version>. Targets are built when their
# toolchain is present and skipped (with a note) otherwise:
#   Linux x86_64 / aarch64   native build, or cross with aarch64-linux-gnu-gcc
#   Windows x86_64           cross with the MinGW toolchain (x86_64-w64-mingw32-gcc);
#                            the .msi needs wixl (msitools)
#   macOS universal          only on a Mac (cargo-bundle, lipo, hdiutil)
#
# Publishing needs `gh`, and `oras` for GitHub Packages, plus a token in GH_TOKEN (or
# GITHUB_TOKEN): a classic personal access token with the `repo` and `write:packages` scopes
# (ghcr.io doesn't accept fine-grained tokens). The commit being released must already be
# on GitHub. Files already in dist/v<version>/ (e.g. a macOS build from a Mac or from an
# earlier workflow run) are published too, so build on a Mac first if you need it.
set -euo pipefail
cd "$(dirname "$0")/.."

publish=0
tests=1
for arg in "$@"; do
  case "$arg" in
    --publish) publish=1 ;;
    --skip-tests) tests=0 ;;
    -h | --help) sed -n '2,22p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
tag="v$version"
dist="dist/$tag"
repo="d3v0ps-cloud/CleanRSTAI"
built=()
skipped=()

step() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }
skip() { skipped+=("$1: $2"); echo "skipped: $2"; }

if [ "$publish" = 1 ]; then
  token="${GH_TOKEN:-${GITHUB_TOKEN:-}}"
  [ -n "$token" ] || { echo "--publish needs a token in GH_TOKEN or GITHUB_TOKEN" >&2; exit 1; }
  export GH_TOKEN="$token"
  have gh || { echo "--publish needs the GitHub CLI (gh)" >&2; exit 1; }
  gh auth status >/dev/null 2>&1 || { echo "GH_TOKEN is not valid for github.com (gh auth status failed)" >&2; exit 1; }
  if gh release view "$tag" --repo "$repo" >/dev/null 2>&1; then
    echo "Release $tag already exists; bump the version in Cargo.toml first." >&2
    exit 1
  fi
  commit=$(git rev-parse HEAD)
  git fetch -q origin
  if ! git branch -r --contains "$commit" | grep -q .; then
    echo "Commit $commit is not on GitHub yet; push it first." >&2
    exit 1
  fi
fi

if [ "$tests" = 1 ]; then
  step "Checks and tests"
  cargo fmt --check
  cargo clippy --all-targets -- -D warnings
  cargo test
  if have npx; then
    cargo test -- --ignored
  else
    echo "npx not found: end-to-end MCP tests not run"
  fi
fi

# Keep files already in dist/ (e.g. a macOS build copied from a Mac); rebuilt ones are replaced.
mkdir -p "$dist"
rm -f "$dist/SHA256SUMS.txt"

step "Linux x86_64"
if [ "$(uname -s)" = Linux ] && [ "$(uname -m)" = x86_64 ]; then
  cargo build --release --target x86_64-unknown-linux-gnu
  tar -czf "$dist/CleanRSTAI-linux-x86_64.tar.gz" -C target/x86_64-unknown-linux-gnu/release cleanrstai
  built+=(linux-x86_64)
else
  skip linux-x86_64 "needs a Linux x86_64 host"
fi

step "Linux aarch64"
if [ "$(uname -s)" = Linux ] && [ "$(uname -m)" = aarch64 ]; then
  cargo build --release --target aarch64-unknown-linux-gnu
elif have aarch64-linux-gnu-gcc; then
  rustup target add aarch64-unknown-linux-gnu >/dev/null 2>&1 || true
  CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    cargo build --release --target aarch64-unknown-linux-gnu
fi
if [ -f target/aarch64-unknown-linux-gnu/release/cleanrstai ] && { [ "$(uname -m)" = aarch64 ] || have aarch64-linux-gnu-gcc; }; then
  tar -czf "$dist/CleanRSTAI-linux-aarch64.tar.gz" -C target/aarch64-unknown-linux-gnu/release cleanrstai
  built+=(linux-aarch64)
else
  skip linux-aarch64 "needs an ARM64 Linux host or aarch64-linux-gnu-gcc"
fi

step "Windows x86_64"
if have x86_64-w64-mingw32-gcc; then
  rustup target add x86_64-pc-windows-gnu >/dev/null 2>&1 || true
  cargo build --release --target x86_64-pc-windows-gnu
  exe=target/x86_64-pc-windows-gnu/release/cleanrstai.exe
  rm -f "$dist/CleanRSTAI-windows-x86_64.zip"
  (cd "$(dirname "$exe")" && zip -q -9 "$OLDPWD/$dist/CleanRSTAI-windows-x86_64.zip" cleanrstai.exe)
  built+=(windows-x86_64.zip)
  if have wixl; then
    wixl -a x64 -D "Version=$version" -D "Exe=$exe" \
      -o "$dist/CleanRSTAI-windows-x86_64.msi" packaging/windows/wixl.wxs
    built+=(windows-x86_64.msi)
  else
    skip windows-msi "wixl (msitools) not found"
  fi
else
  skip windows-x86_64 "x86_64-w64-mingw32-gcc (MinGW) not found"
fi

step "macOS universal"
if [ "$(uname -s)" = Darwin ]; then
  have cargo-bundle || cargo install cargo-bundle --locked
  rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
  cargo bundle --release --target aarch64-apple-darwin
  cargo build --release --target x86_64-apple-darwin
  app=target/aarch64-apple-darwin/release/bundle/osx/CleanRSTAI.app
  lipo -create -output "$app/Contents/MacOS/cleanrstai" \
    target/aarch64-apple-darwin/release/cleanrstai target/x86_64-apple-darwin/release/cleanrstai
  codesign --force --deep --sign - "$app"
  ditto -c -k --keepParent "$app" "$dist/CleanRSTAI-macos-universal.zip"
  staging=$(mktemp -d)
  cp -R "$app" "$staging/"
  ln -s /Applications "$staging/Applications"
  hdiutil create -volname CleanRSTAI -srcfolder "$staging" -ov -format UDZO \
    "$dist/CleanRSTAI-macos-universal.dmg" >/dev/null
  built+=(macos-universal)
elif compgen -G "$dist/CleanRSTAI-macos-*" >/dev/null; then
  echo "using the macOS files already in $dist"
else
  skip macos "needs a Mac (Apple SDK, lipo, hdiutil)"
fi

step "Checksums"
(cd "$dist" && sha256sum -- * | tee SHA256SUMS.txt)
for f in "$dist"/*; do
  case "$(basename "$f")" in
    *macos*) [[ " ${built[*]:-} " == *" macos-universal "* ]] || echo "note: $(basename "$f") was not built in this run" ;;
  esac
done

if [ "$publish" = 1 ]; then
  step "GitHub Release $tag"
  notes="Built and published locally with scripts/release-local.sh."
  if [ ${#skipped[@]} -gt 0 ]; then
    notes+=$'\n\nNot included in this release:\n'
    for s in "${skipped[@]}"; do notes+="- $s"$'\n'; done
  fi
  gh release create "$tag" "$dist"/* --repo "$repo" --target "$commit" \
    --title "$tag" --generate-notes --notes "$notes" \
    $([[ "$tag" == *-* ]] && echo --prerelease)

  step "GitHub Packages (ghcr.io)"
  if have oras; then
    image="ghcr.io/$(echo "${repo%%/*}" | tr '[:upper:]' '[:lower:]')/cleanrstai"
    login=$(gh api user --jq .login)
    echo "$GH_TOKEN" | oras login ghcr.io -u "$login" --password-stdin
    files=()
    for f in "$dist"/*; do
      name=$(basename "$f")
      case "$name" in
        *.zip) files+=("$name:application/zip") ;;
        *.tar.gz) files+=("$name:application/gzip") ;;
        *.dmg) files+=("$name:application/x-apple-diskimage") ;;
        *.msi) files+=("$name:application/x-msi") ;;
        *) files+=("$name:text/plain") ;;
      esac
    done
    tags="$tag"
    [[ "$tag" == *-* ]] || tags="$tag,latest"
    (cd "$dist" && oras push "$image:$tags" \
      --artifact-type application/vnd.cleanrstai.release.v1 \
      --annotation "org.opencontainers.image.source=https://github.com/$repo" \
      --annotation "org.opencontainers.image.description=CleanRSTAI desktop builds" \
      --annotation "org.opencontainers.image.version=$tag" \
      "${files[@]}")
  else
    skip ghcr "oras not found; the GitHub Release was published, GitHub Packages was not"
  fi
fi

step "Done: $tag"
echo "Built: ${built[*]:-nothing}"
for s in "${skipped[@]}"; do echo "Skipped $s"; done
echo "Files: $dist/"
