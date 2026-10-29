#!/usr/bin/env bash
# Tag'deki sürümün projedeki sürüm alanlarıyla aynı olduğunu doğrular.
#
# Kullanım:
#   check-version.sh <tag> <sürüm> [<sürüm> ...]
#
# Örnek:
#   check-version.sh v0.1.0 "$(jq -r .version package.json)" "$(jq -r .version src-tauri/tauri.conf.json)"
set -euo pipefail

tag=$1; shift
version=${tag#v}

if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
  echo "::error::Tag semver değil: $tag (beklenen: v1.2.3)" >&2
  exit 1
fi

for found in "$@"; do
  if [ "$found" != "$version" ]; then
    echo "::error::Tag $tag ama projedeki sürüm $found. Önce sürümü güncelleyip commit'le." >&2
    exit 1
  fi
done

echo "$version"
