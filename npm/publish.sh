#!/bin/sh
# Publishes the npm packages from the archives dist builds: each platform
# package, then tokendraw, so its optional dependencies exist before it
# can be installed. A version already on npm is skipped, so a failed
# publish can be run again.
#
#   npm/publish.sh ARCHIVES VERSION TAG [--dry-run]
set -eu
archives=$(cd "$1" && pwd)
version=$2
tag=$3
dry_run=${4:-}
npm=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

set --
for target in $(node -e 'console.log(Object.keys(require(process.argv[1])).join(" "))' "$npm/platforms.json"); do
    mkdir "$work/$target"
    if [ -f "$archives/tokendraw-$target.zip" ]; then
        unzip -q "$archives/tokendraw-$target.zip" tokendraw.exe -d "$work/$target"
        set -- "$@" "$target=$work/$target/tokendraw.exe"
    else
        tar -xJf "$archives/tokendraw-$target.tar.xz" -C "$work/$target" --strip-components 1 "tokendraw-$target/tokendraw"
        set -- "$@" "$target=$work/$target/tokendraw"
    fi
done
node "$npm/build.js" "$version" "$work/packages" "$@"

for dir in "$work"/packages/tokendraw-* "$work/packages/tokendraw"; do
    name=$(basename "$dir")
    if [ -n "$dry_run" ]; then
        npm publish --dry-run --access public --tag "$tag" "$dir"
    elif npm view "$name@$version" version >/dev/null 2>&1; then
        echo "$name@$version is already on npm"
    else
        npm publish --access public --provenance --tag "$tag" "$dir"
    fi
done
