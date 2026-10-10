#!/bin/sh
# Global.lua offline (Koetama faked), then against the real Koetama: bash run_test.sh   (LUAJIT=... KOETAMA=... to
# override; KT_OFFLINE_ONLY=1: the offline part only)
set -e
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../.." && pwd)
LUAJIT=${LUAJIT:-/c/Users/seann/miniconda3/envs/teardown/Library/bin/luajit.exe}
KOETAMA=${KOETAMA:-$repo/app/target/release/koetama.exe}
"$LUAJIT" "$here/offline.lua"
[ -n "$KT_OFFLINE_ONLY" ] && exit 0
work=$(mktemp -d)
mkdir -p "$work/profiles"
cp "$repo/examples/profiles/tabletop-simulator-koetama.json" "$work/profiles/"
export KOETAMA_PROFILES_DIR=$(cygpath -w "$work/profiles")
"$LUAJIT" "$here/harness.lua" "$(cygpath -w "$KOETAMA")" "$(cygpath -w "$work")"
status=$?
echo "Koetama's log: $work/koetama.log"
exit $status
