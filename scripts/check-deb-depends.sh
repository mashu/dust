#!/usr/bin/env bash
# Fail unless the Depends of the .deb given names every package whose
# shared libraries its binary links, as dpkg-shlibdeps finds them on this
# machine. dx writes Depends from Dioxus.toml and looks at no binary, so a
# library a new dependency brings in would otherwise ship undeclared, and
# Dust would fail to start wherever that library is not installed.
#
#   scripts/check-deb-depends.sh dist/dust_0.16.1_amd64.deb
set -euo pipefail

deb=$1
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

dpkg-deb -x "$deb" "$work/root"
mkdir -p "$work/debian"
printf 'Source: dust\n\nPackage: dust\nArchitecture: any\n' > "$work/debian/control"
needed="$(cd "$work" && dpkg-shlibdeps -O root/usr/bin/dust)"
needed="${needed#shlibs:Depends=}"
declared="$(dpkg-deb -f "$deb" Depends)"
echo "Depends: $declared"

# Package names alone: no versions, every alternative counted.
names() {
  sed -E 's/\([^)]*\)//g; s/[,|]/ /g' <<< "$1" | xargs -n1 | sort -u
}
missing="$(comm -23 <(names "$needed") <(names "$declared"))"
if [[ -n "$missing" ]]; then
  echo "The binary links libraries from packages its Depends leaves out:" >&2
  echo "  ${missing//$'\n'/$'\n'  }" >&2
  echo "Add them to [bundle.deb] depends in Dioxus.toml." >&2
  exit 1
fi
echo "Depends covers every library the binary links."
