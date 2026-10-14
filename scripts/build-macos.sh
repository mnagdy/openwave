#!/bin/sh
set -eu
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$project_dir"
cargo build --release --locked
app_dir="$project_dir/dist/OpenRoom.app"
mkdir -p "$app_dir/Contents/MacOS" "$app_dir/Contents/Resources"
build_dir=${CARGO_TARGET_DIR:-target}
cp "$build_dir/release/open-room" "$app_dir/Contents/MacOS/open-room"
cp scripts/Info.plist "$app_dir/Contents/Info.plist"
cp LICENSE THIRD_PARTY_LICENSES.txt "$app_dir/Contents/Resources/"
if xattr "$app_dir" | grep -q '^com.apple.FinderInfo$'; then
    xattr -d com.apple.FinderInfo "$app_dir"
fi
codesign --force --deep --sign - "$app_dir"
codesign --verify --deep --strict "$app_dir"
printf 'Built %s\n' "$app_dir"
