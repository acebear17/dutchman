export PATH="$HOME/.cargo/bin:$PATH" TMPDIR=$HOME/builds/convert-lead/tmp
set -e
mkdir -p out/shots
W=$PWD
M=$HOME/builds/convert-lead/feature-progress-measure
rm -rf "$M"; mkdir -p "$M" out
cargo build -j 3 --release --locked -p dutchman --bin dutchman
TD=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')
cp "$TD/release/dutchman" "$M/branch"
mkdir -p "$M/src-base" && tar -xzf .lane-base.tar.gz -C "$M/src-base"
cd "$M/src-base"
CARGO_TARGET_DIR=$HOME/builds/convert-lead/fp-base-target cargo build -j 3 --release --locked -p dutchman --bin dutchman
cp $HOME/builds/convert-lead/fp-base-target/release/dutchman "$M/base"
cd "$W"
P=$HOME/builds/convert-lead/pub50/in
R=/var/home/ray/Documents/GitHub/railcore-f3z
echo "set,round,binary,seconds,progress_lines" > out/times.csv
run() { # set round bin args...
  local set=$1 round=$2 bin=$3; shift 3
  local o="$M/o-$set-$bin-$round"; rm -rf "$o"; mkdir -p "$o"
  local s=$(date +%s.%N)
  nice -n 10 "$M/$bin" convert-all "$@" --out-dir "$o/sc" --no-verify --json --jobs 1 > "$o/stdout.json" 2> "$o/stderr.txt" || true
  local e=$(date +%s.%N)
  echo "$set,$round,$bin,$(python3 -c "print(round($e-$s,2))"),$(grep -c dutchman.progress/1 "$o/stderr.txt" || true)" >> out/times.csv
}
mkdir -p "$M/rc"; cp "$R"/*.f3z "$M/rc/"
for round in 1 2; do
  for bin in base branch; do run pub50 $round $bin "$P"; run railcore $round $bin "$M/rc"; done
  for bin in branch base; do run pub50 $((round+2)) $bin "$P"; run railcore $((round+2)) $bin "$M/rc"; done
done
python3 - "$M" > out/features.txt <<'PY'
import json,sys,glob,os
def walk(v,acc):
    if isinstance(v,dict):
        h=v.get("history")
        if isinstance(h,dict) and isinstance(h.get("total"),int) and "features" in h: acc.append(h["total"])
        for x in v.values(): walk(x,acc)
    elif isinstance(v,list):
        for x in v: walk(x,acc)
for set_ in ("pub50","railcore"):
    acc=[]
    for line in open(os.path.join(sys.argv[1],f"o-{set_}-branch-1","stdout.json")):
        try: walk(json.loads(line),acc)
        except Exception: pass
    print(set_, "reports with history:", len(acc), "features total:", sum(acc), "max per design:", max(acc) if acc else 0)
PY
grep -h '"feature"' "$M"/o-railcore-branch-1/stderr.txt | head -3 | sed 's/"path":"[^"]*"/"path":"…"/' > out/sample-progress.txt || true
cat out/times.csv out/features.txt

set +e
DUTCHMAN_GUI_SCREENSHOTS=$PWD/out/shots cargo test -j 3 --locked -p dutchman-gui --features test-support --test ui -- a_running_card_shows_the_feature > out/shots.log 2>&1; echo "shots exit $?"
ls out/shots
