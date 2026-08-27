#!/usr/bin/env bash
# Rebuild and overwrite the most recent GitHub release (or a chosen tag).
#
# Usage:
#   ./scripts/release-latest.sh
#   ./scripts/release-latest.sh --force --ci
#
# Requires: gh, git, cargo, rustc, uv (native builds with bundled Python)

set -euo pipefail

TAG=""
TARGET=""
FORCE=0
SKIP_BUILD=0
SKIP_TAG_PUSH=0
SKIP_UPLOAD=0
CI=0
CI_ONLY=0

usage() {
  sed -n '2,12p' "$0" | sed 's/^# \?//'
  echo ""
  echo "Options:"
  echo "  --tag TAG           Release tag (default: latest on GitHub)"
  echo "  --target TRIPLE     Rust target triple (default: host)"
  echo "  --force             Skip confirmation"
  echo "  --skip-build        Use existing target/ packages"
  echo "  --skip-tag-push     Do not move the tag to HEAD"
  echo "  --skip-upload       Build only, do not upload"
  echo "  --ci                Also dispatch Release builds workflow (Linux + macOS)"
  echo "  --ci-only           Dispatch workflow only (no local build)"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --tag) TAG="$2"; shift 2 ;;
    --target) TARGET="$2"; shift 2 ;;
    --force) FORCE=1; shift ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --skip-tag-push) SKIP_TAG_PUSH=1; shift ;;
    --skip-upload) SKIP_UPLOAD=1; shift ;;
    --ci) CI=1; shift ;;
    --ci-only) CI_ONLY=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 1 ;;
  esac
done

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

gh auth status >/dev/null

if [[ -z "$TAG" ]]; then
  TAG="$(gh release list --limit 1 --json tagName -q '.[0].tagName')"
  [[ -n "$TAG" ]] || { echo "No GitHub releases found." >&2; exit 1; }
fi

if [[ -z "$TARGET" ]]; then
  TARGET="$(rustc -vV | sed -n 's/^host: //p')"
fi

PRERELEASE="$(gh release view "$TAG" --json isPrerelease -q .isPrerelease)"
NAME="$(gh release view "$TAG" --json name -q .name)"
HEAD="$(git rev-parse --short HEAD)"

echo "Release tag:     $TAG"
echo "Release name:    $NAME"
echo "Host target:     $TARGET"
echo "HEAD:            $HEAD"
echo "Prerelease:      $PRERELEASE"
echo

if [[ "$FORCE" -ne 1 ]]; then
  read -r -p "Overwrite GitHub release '$TAG' with current HEAD? [y/N] " answer
  [[ "$answer" =~ ^[Yy]$ ]] || { echo "Aborted."; exit 1; }
fi

if [[ "$SKIP_TAG_PUSH" -eq 0 ]]; then
  echo "Moving tag $TAG to HEAD..."
  commit="$(git rev-parse HEAD)"
  git tag -f "$TAG"
  git push -f origin "refs/tags/$TAG"
  if [[ "$PRERELEASE" == "true" ]]; then
    gh release edit "$TAG" --target "$commit" --prerelease
  else
    gh release edit "$TAG" --target "$commit"
  fi
fi

if [[ "$CI_ONLY" -eq 0 ]]; then
  if [[ "$SKIP_BUILD" -eq 0 ]]; then
    echo "Downloading models..."
    cargo xtask models
    echo "Building release package for $TARGET..."
    cargo xtask package --version "$TAG" --target "$TARGET"
  fi

  if [[ "$SKIP_UPLOAD" -eq 0 ]]; then
    shopt -s nullglob
    assets=(target/tundra-"$TAG"-"$TARGET".*)
    shopt -u nullglob
    [[ ${#assets[@]} -gt 0 ]] || {
      echo "No package found under target/ for $TAG / $TARGET" >&2
      exit 1
    }
    echo "Uploading ${#assets[@]} asset(s) with --clobber..."
    gh release upload "$TAG" "${assets[@]}" --clobber
  fi
fi

if [[ "$CI" -eq 1 || "$CI_ONLY" -eq 1 ]]; then
  echo "Dispatching Release builds workflow (Linux + macOS)..."
  gh workflow run release.yml -f "tag=$TAG"
  echo "CI run started. Watch: gh run list --workflow=release.yml"
fi

OWNER="$(gh repo view --json nameWithOwner -q .nameWithOwner)"
echo "Done. Release: https://github.com/$OWNER/releases/tag/$TAG"
