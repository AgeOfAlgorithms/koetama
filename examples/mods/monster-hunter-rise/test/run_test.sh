#!/bin/sh
# koetama.lua against the real Koetama: bash run_test.sh   (LUAJIT=... KOETAMA=... to override)
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../.." && pwd)
LUAJIT=${LUAJIT:-/c/Users/seann/miniconda3/envs/teardown/Library/bin/luajit.exe}
KOETAMA=${KOETAMA:-$repo/app/target/release/koetama.exe}
work=$(mktemp -d)
mkdir -p "$work/profiles" "$work/data"
cp "$repo/examples/profiles/monster-hunter-rise-koetama.json" "$work/profiles/"
export KOETAMA_PROFILES_DIR=$(cygpath -w "$work/profiles")
# (the files connector's test overrides: the feed file's folder, and the one folder for Koetama's files)
export SAVEPROBE_DIR=$(cygpath -w "$work/data/koetama")
export HFP_MODS=$(cygpath -w "$work/data/koetama")
"$LUAJIT" "$here/harness.lua" "$(cygpath -w "$KOETAMA")" "$(cygpath -w "$work")"
status=$?
echo "Koetama's log: $work/koetama.log"
exit $status
