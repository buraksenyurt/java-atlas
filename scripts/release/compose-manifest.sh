#!/usr/bin/env bash
# Platform parçalarını tek bir release.json'da birleştirir.
#
# Kullanım:
#   compose-manifest.sh <araç-id> <sürüm> <tag> <owner/repo> <commit> <parça-klasörü> > release.json
set -euo pipefail

if [ "$#" -ne 6 ]; then
  sed -n '2,6p' "$0" >&2
  exit 2
fi

tool=$1 version=$2 tag=$3 repo=$4 commit=$5 dir=$6

shopt -s nullglob
parts=("$dir"/*.json)
[ "${#parts[@]}" -gt 0 ] || { echo "::error::$dir içinde parça yok" >&2; exit 1; }

jq -s --arg tool "$tool" --arg version "$version" --arg tag "$tag" \
      --arg repo "$repo" --arg commit "$commit" \
      --arg created "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
      '{
         schema: 1,
         tool: $tool,
         repo: $repo,
         version: $version,
         tag: $tag,
         commit: $commit,
         created_at: $created,
         readme: "README.md",
         platforms: (reduce .[] as $p ({}; . + $p))
       }' "${parts[@]}"
