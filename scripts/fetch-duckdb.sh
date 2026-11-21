#!/bin/sh
set -eu
dest=${1:?destination directory}
case "$(uname -s)/$(uname -m)" in
  Darwin/*) asset=osx-universal; lib=libduckdb.dylib; checksum=e0bc007d9b0094c0970ac1847a8601d10aad07cbd2910ce586ec810f77b638d6 ;;
  Linux/x86_64) asset=linux-amd64; lib=libduckdb.so; checksum=b845005f5132a7d8180057c35e14a7626632258782f871a90861b19c1c03841b ;;
  Linux/aarch64) asset=linux-arm64; lib=libduckdb.so; checksum=b72ed9f05003f5e9d2015f7ceada6416b377d9dd33169cdde3c9e33897856eee ;;
  *) echo "Unsupported build platform" >&2; exit 1 ;;
esac
mkdir -p "$dest"
archive=$(mktemp)
trap 'rm -f "$archive"' EXIT
curl -fsSL --retry 3 "https://github.com/duckdb/duckdb/releases/download/v1.5.6/libduckdb-$asset.zip" -o "$archive"
if command -v sha256sum >/dev/null; then
  actual=$(sha256sum "$archive")
else
  actual=$(shasum -a 256 "$archive")
fi
test "${actual%% *}" = "$checksum"
unzip -oq "$archive" duckdb.h "$lib" -d "$dest"
