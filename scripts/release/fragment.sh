#!/usr/bin/env bash
# Bir platformun release.json parçasını üretir.
#
# Kullanım:
#   fragment.sh <platform> <çıktı.json> <taşınabilir-dosya> <biçim> <giriş> [yükleyici:biçim ...]
#
#   platform   windows-x64 | windows-arm64 | linux-x64 | macos-arm64
#   biçim      zip | tar.gz | file
#   giriş      arşiv içinde çalıştırılacak dosya (biçim "file" ise dosyanın kendisi)
#
# Örnek:
#   fragment.sh windows-x64 fragment.json dist/easy-git-0.1.0-windows-x64.zip zip easy-git.exe \
#     dist/easy-git-0.1.0-windows-x64-setup.exe:nsis dist/easy-git-0.1.0-windows-x64.msi:msi
set -euo pipefail

if [ "$#" -lt 5 ]; then
  sed -n '2,15p' "$0" >&2
  exit 2
fi

platform=$1 out=$2 portable=$3 format=$4 entry=$5
shift 5

for f in "$portable" "${@%:*}"; do
  [ -f "$f" ] || { echo "::error::Dosya bulunamadı: $f" >&2; exit 1; }
done

sha() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

asset() {
  jq -n --arg a "$(basename "$1")" --arg s "$(sha "$1")" \
        --argjson z "$(wc -c < "$1" | tr -d ' \r')" --arg f "$2" \
        '{asset: $a, sha256: $s, size: $z, format: $f}'
}

installers='[]'
for spec in "$@"; do
  file=${spec%:*} fmt=${spec##*:}
  installers=$(jq -c --argjson i "$(asset "$file" "$fmt")" '. + [$i]' <<<"$installers")
done

jq -n --arg p "$platform" --arg e "$entry" \
      --argjson portable "$(asset "$portable" "$format")" \
      --argjson installers "$installers" \
      '{($p): {portable: ($portable + {entry: $e}), installers: $installers}}' > "$out"

echo "$platform → $out"
cat "$out"
