#!/usr/bin/env bash
set -euo pipefail

install_dir="${1:?Pass a destination directory for GitHub CLI}"
version=2.105.0
distribution="gh_${version}_linux_amd64"
archive="$install_dir/$distribution.tar.gz"
sha256=79e7ab5db738c48777b696e374f8b428f0647debfb424f44d2f426370407fe2f

mkdir -p "$install_dir"
curl --fail --location --retry 3 --output "$archive" \
  "https://github.com/cli/cli/releases/download/v$version/$distribution.tar.gz"
printf '%s  %s\n' "$sha256" "$archive" | sha256sum --check
tar -xzf "$archive" --strip-components=2 -C "$install_dir" "$distribution/bin/gh"
rm -- "$archive"
"$install_dir/gh" --version
