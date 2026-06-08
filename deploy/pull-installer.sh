#!/usr/bin/env bash
# Pull the newest published installer from GitHub Releases into the public
# download folder served by Caddy at squadlink.raumdock.org/download.
# Public repo → no auth/token needed. Run by a systemd timer on LXC 103.
set -euo pipefail

REPO="cccdemon/RDOC-SACompanion"
DEST="/opt/RDOC-Suite/downloads/squadlink"
API="https://api.github.com/repos/${REPO}/releases?per_page=10"
# Robust against transient GitHub/CDN 5xx right after a release is published.
RETRY="--retry 6 --retry-delay 4 --retry-all-errors"

mkdir -p "$DEST"

# Newest release is first in the array; grab its asset URLs (incl. prereleases).
urls="$(curl -fsSL $RETRY -H 'Accept: application/vnd.github+json' "$API" \
  | grep -oE '"browser_download_url": *"[^"]+"' | cut -d'"' -f4 || true)"
[ -n "$urls" ] || { echo "no releases yet"; exit 0; }

fetch() {
  # $1 = case-insensitive filename suffix to match, $2 = output filename
  local url
  url="$(printf '%s\n' "$urls" | grep -iE "$1" | head -1 || true)"
  [ -n "$url" ] || { echo "no asset for $1"; return 0; }
  curl -fsSL $RETRY -o "$DEST/$2.tmp" "$url"
  mv "$DEST/$2.tmp" "$DEST/$2"
  echo "updated $2  <-  $url"
}

fetch 'setup\.exe$'        RDOC-SquadLink-Lite-Setup.exe
fetch '_x64_en-US\.msi$|\.msi$'  RDOC-SquadLink-Lite.msi
