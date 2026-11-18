export PATH="$HOME/.cargo/bin:$PATH" TMPDIR=$HOME/builds/convert-lead/tmp
mkdir -p out/shots
run() { echo "== $*"; "$@" > out/step.log 2>&1; c=$?; tail -40 out/step.log; cat out/step.log >> out/check.log; echo "== exit $c: $*"; [ $c -eq 0 ] || FAIL=1; }
FAIL=0
run cargo fmt --all --check
run cargo clippy -j 3 -p dutchman-core -p dutchman-solidcraft -p dutchman -p dutchman-web --all-targets --locked -- -D warnings
run cargo clippy -j 3 -p dutchman-gui --all-targets --features test-support --locked -- -D warnings
run cargo test -j 3 --locked -p dutchman-core --lib
run cargo test -j 3 --locked -p dutchman --lib
run cargo test -j 3 --locked -p dutchman-solidcraft --lib progress
run cargo test -j 3 --locked -p dutchman-gui --features test-support --lib
run env DUTCHMAN_GUI_SCREENSHOTS=$PWD/out/shots cargo test -j 3 --locked -p dutchman-gui --features test-support --test ui
ls -la out/shots
echo "ALL FAIL=$FAIL"
exit $FAIL
