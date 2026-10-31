#!/bin/sh
# Builds the DMG, publishes it as a GitHub release and updates the Homebrew cask.
# Usage: scripts/release.sh <path to a homebrew-tap checkout>
# Bump the version in src-tauri/tauri.conf.json first, and commit it. The app is signed ad hoc
# ("-"), so any Apple Silicon Mac can run this; no certificate is needed.
set -eu

tap=${1:?usage: scripts/release.sh <path to a homebrew-tap checkout>}
cask="$tap/Casks/pantau.rb"
[ -f "$cask" ] || { echo "No cask at $cask. Copy docs/release/pantau.rb there first." >&2; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "Commit your changes first." >&2; exit 1; }

version=$(node -p "require('./src-tauri/tauri.conf.json').version")
tag="v$version"
pnpm tauri build --bundles app,dmg

dmg="src-tauri/target/release/bundle/dmg/Pantau_${version}_aarch64.dmg"
[ -f "$dmg" ] || { echo "No DMG at $dmg" >&2; exit 1; }
sha=$(shasum -a 256 "$dmg" | cut -d' ' -f1)
gh release create "$tag" "$dmg" -R fizipan/pantau --title "$tag" --generate-notes

sed -i '' -e "s/^  version \".*\"/  version \"$version\"/" -e "s/^  sha256 \".*\"/  sha256 \"$sha\"/" "$cask"
git -C "$tap" add Casks/pantau.rb
git -C "$tap" commit -m "pantau $version"
git -C "$tap" push
echo "Released $tag: brew upgrade --cask pantau"
