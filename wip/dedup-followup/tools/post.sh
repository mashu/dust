#!/bin/bash
# Checks of the merged head beyond verify.sh: graphs, doc links, the Windows probe,
# the walks against the lifecycle head's, the test names against both heads' and the
# base's, and the Android lint. Logs to /home/user/wt/logs/merged.*, results in $D.
set -u
S=/tmp/claude-0/-home-user-dust/e07ea342-050f-502a-b473-6fa881d75cc0/scratchpad
D=$S/s9
L=/home/user/wt/logs
T=/home/user/wt/targets/dr
export RUSTUP_TOOLCHAIN=1.99.1 CARGO_TARGET_DIR=$T CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CI=1
SEEDS=${SEEDS:-2000}

bin_of() { # $1 = checkout, rest = cargo test args; prints the dust test binary
  (cd "$1" && cargo test -p dust --locked --no-run --message-format=json "${@:2}" 2>/dev/null) | python3 -c '
import json, sys
for line in sys.stdin:
    try: m = json.loads(line)
    except Exception: continue
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["name"] == "dust" and m["profile"]["test"]:
        print(m["executable"])'
}
list_of() { # $1 = binary, $2 = cwd -> sorted test names
  (cd "$2" && "$1" --list 2>/dev/null) | grep ': test$' | sed 's/: test$//' | sort
}
walks() { # $1 = binary, $2 = cwd, $3 = out dir
  rm -rf "$3"; mkdir -p "$3"
  (cd "$2" && DUST_LIFECYCLE="$3" DUST_LIFECYCLE_SEEDS=$SEEDS "$1" --ignored lifecycle_walk:: 2>&1) | grep -E "^test result|FAILED|panicked" | head -20
  echo "traces: $(ls "$3" | wc -l)"
}

echo "### graphs"
bash $S/dr-tools/graphs.sh /home/user/dust $L/merged.edges > $L/merged.graphs 2>&1
cat $L/merged.graphs
echo "### doc links"
/home/user/wt/doclinks.sh /home/user/dust $T app > $L/merged.doclinks-app 2>$L/merged.doclinks-app.err
/home/user/wt/doclinks.sh /home/user/dust $T core > $L/merged.doclinks-core 2>$L/merged.doclinks-core.err
cat $L/merged.doclinks-app.err $L/merged.doclinks-core.err
for k in app core; do
  if diff <(sed -E 's/^ *[0-9]+ //' $L/merged.doclinks-$k | sort) <(sed -E 's/^ *[0-9]+ //' $L/doclinks-base-$k.txt | sort) > $L/merged.doclinks-$k.diff; then
    echo "$k: same set as the base ($(wc -l < $L/merged.doclinks-$k) kinds, $(awk '{s+=$1} END {print s}' $L/merged.doclinks-$k) links)"
  else echo "$k: DIFFERS"; cat $L/merged.doclinks-$k.diff; fi
done
echo "### Windows probe"
bash $S/dr-tools/winprobe.sh /home/user/dust $S/s9-winprobe $L/merged.winprobe
rm -rf $S/s9-winprobe

echo "### merged lists and walks"
MD=$D/bin/dust-merged-desktop
MS=$(bin_of /home/user/dust --no-default-features --features mobile-silent)
list_of $MD /home/user/dust > $D/merged.desktop.list
list_of $MS /home/user/dust > $D/merged.silent.list
wc -l $D/merged.desktop.list $D/merged.silent.list
walks $MD /home/user/dust $D/walks-merged

echo "### the lifecycle head (6a424c6)"
git -C /home/user/wt/dr status --short | head -3
git -C /home/user/wt/dr checkout -q wip/dedup-runtime && git -C /home/user/wt/dr log --oneline -1
LD=$(bin_of /home/user/wt/dr --features desktop)
cp "$LD" $D/bin/dust-life-desktop
list_of $D/bin/dust-life-desktop /home/user/wt/dr > $D/life.desktop.list
wc -l $D/life.desktop.list $D/life.silent.list
walks $D/bin/dust-life-desktop /home/user/wt/dr $D/walks-life
python3 $S/dr-tools/walkdiff.py $D/walks-life $D/walks-merged equal | tail -20
echo "walkdiff exit ${PIPESTATUS[0]}"
rm -f $D/bin/dust-life-desktop

for rev in aa6af38 ab0cbf7; do
  echo "### lists at $rev"
  git -C /home/user/wt/dr checkout -q --detach $rev && git -C /home/user/wt/dr log --oneline -1
  B=$(bin_of /home/user/wt/dr --features desktop); list_of "$B" /home/user/wt/dr > $D/$rev.desktop.list
  B=$(bin_of /home/user/wt/dr --no-default-features --features mobile-silent); list_of "$B" /home/user/wt/dr > $D/$rev.silent.list
  wc -l $D/$rev.desktop.list $D/$rev.silent.list
done
git -C /home/user/wt/dr checkout -q wip/dedup-runtime && git -C /home/user/wt/dr log --oneline -1

echo "### names"
python3 - $D <<'PY'
import sys
D = sys.argv[1] + '/'
rd = lambda p: set(open(D + p).read().split())
SIGNIN = 'sync::auth::desktop::tests::a_browser_is_answered_once_its_whole_request_is_read'
for kind in ('desktop', 'silent'):
    B, S, L, M = (rd(f'{r}.{kind}.list') for r in ('ab0cbf7', 'aa6af38', 'life', 'merged'))
    Bs = B - {SIGNIN}           # the store's base, 2207d65, is ab0cbf7 without the sign-in test
    rm_s, rm_l = Bs - S, B - L
    add_s, add_l = S - Bs, L - B
    want = (B - rm_s - rm_l) | add_s | add_l
    print(f'{kind}: base {len(B)}, store {len(S)} (+{len(add_s)} -{len(rm_s)}), lifecycle {len(L)} (+{len(add_l)} -{len(rm_l)}), merged {len(M)}, want {len(want)}')
    print('  added by both:', sorted(add_s & add_l)[:5], ' removed by both:', sorted(rm_s & rm_l)[:5])
    print('  missing from the merge:', sorted(want - M)[:20])
    print('  not wanted in the merge:', sorted(M - want)[:20])
PY

echo "### Android lint"
/home/user/wt/android-clippy.sh /home/user/dust /home/user/wt/targets/s9-android > $L/merged.android 2>&1
echo "android exit $?"; tail -2 $L/merged.android
rm -rf /home/user/wt/targets/s9-android
echo "### POST DONE"
