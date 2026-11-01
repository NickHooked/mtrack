#!/usr/bin/env bash
# Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
#
# This program is free software: you can redistribute it and/or modify it under
# the terms of the GNU General Public License as published by the Free Software
# Foundation, version 3.
#
# This program is distributed in the hope that it will be useful, but WITHOUT
# ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
# FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License along with
# this program. If not, see <https://www.gnu.org/licenses/>.
#
# Non-destructive checks for setup package selection, CI feature separation,
# and cargo-deb base/video metadata.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

ok() {
  echo "  ok: $*"
}

assert_contains() {
  case "$1" in
    *"$2"*) ;;
    *) fail "$3 (missing: $2)" ;;
  esac
}

assert_not_contains() {
  case "$1" in
    *"$2"*) fail "$3 (unexpected: $2)" ;;
    *) ;;
  esac
}

echo "== setup dry-run package selection =="
mkdir -p "$TMP/bin"
cat >"$TMP/bin/sudo" <<'EOF'
#!/usr/bin/env bash
echo sudo-called >>"${MTRACK_TEST_SIDE_EFFECTS:?}"
exit 99
EOF
cat >"$TMP/bin/rustup" <<'EOF'
#!/usr/bin/env bash
echo rustup-called >>"${MTRACK_TEST_SIDE_EFFECTS:?}"
exit 99
EOF
chmod +x "$TMP/bin/sudo" "$TMP/bin/rustup"
side_effects="$TMP/side-effects"
base_output="$(cd "$ROOT" && MTRACK_TEST_SIDE_EFFECTS="$side_effects" PATH="$TMP/bin:$PATH" ./setup.sh --dry-run)"
video_output="$(cd "$ROOT" && MTRACK_TEST_SIDE_EFFECTS="$side_effects" PATH="$TMP/bin:$PATH" ./setup.sh --dry-run --video)"
[ ! -e "$side_effects" ] || fail "setup --dry-run invoked a state-changing command"
assert_contains "$base_output" "sudo apt-get install -y build-essential pkg-config libasound2-dev libudev-dev libssl-dev protobuf-compiler" "base apt package list"
assert_not_contains "$base_output" "gstreamer" "base setup must remain free of GStreamer"
assert_contains "$video_output" "sudo apt-get install -y build-essential pkg-config libasound2-dev libudev-dev libssl-dev protobuf-compiler libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev gstreamer1.0-tools gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav" "video apt package list"
ok "setup package selection is explicit and side-effect free"

echo "== manifest and workflow intent =="
python3 - "$ROOT" <<'PY'
from pathlib import Path
import sys
import tomllib

root = Path(sys.argv[1])
data = tomllib.loads((root / "Cargo.toml").read_text())
deb = data["package"]["metadata"]["deb"]
video = deb["variants"]["video"]

for field in ("depends", "recommends"):
    assert "gstreamer" not in deb.get(field, "").lower(), f"base {field} contains GStreamer"
assert video["features"] == ["video"], video["features"]
assert video["conflicts"] == "mtrack"
assert video["replaces"] == "mtrack"
assert video["provides"] == "mtrack"
assert "libgstreamer1.0-0" in video["depends"]
assert "libgstreamer-plugins-base1.0-0" in video["depends"]
assert "gstreamer1.0-plugins-base" in video["depends"]
for package in ("gstreamer1.0-plugins-good", "gstreamer1.0-plugins-bad", "gstreamer1.0-libav"):
    assert package in video["recommends"], package

workflow = (root / ".github/workflows/mtrack.yaml").read_text()
clippy = workflow.split("  clippy:\n", 1)[1].split("\n  rustfmt-check:\n", 1)[0]
assert "gstreamer" not in clippy.lower(), "ordinary clippy installs GStreamer"
assert "--all-features" not in clippy, "ordinary clippy enables all features"
assert "--no-default-features" in clippy, "ordinary clippy is not explicitly audio-only"
video_job = workflow.split("  video:\n", 1)[1].split("\n  test-deb:\n", 1)[0]
assert "cargo clippy" in video_job and "--features video" in video_job, "video clippy coverage missing"
assert "cargo test --features video video" in video_job, "video diagnostics/startup tests missing"
assert "cargo test --features video --test video_engine" in video_job, "video engine/preview tests missing"
PY
ok "manifest metadata and CI feature lanes match their intent"

echo "== cargo-deb video package metadata =="
command -v cargo-deb >/dev/null || fail "cargo-deb is required"
command -v dpkg-deb >/dev/null || fail "dpkg-deb is required"
mkdir -p "$TMP/target/release" "$TMP/debs"
install -m 755 /bin/true "$TMP/target/release/mtrack"
(
  cd "$ROOT"
  CARGO_TARGET_DIR="$TMP/target" cargo deb --variant video --no-build --no-strip --fast --output "$TMP/debs/" >/dev/null
)
debs=("$TMP"/debs/mtrack-video_*.deb)
[ "${#debs[@]}" -eq 1 ] && [ -f "${debs[0]}" ] || fail "cargo-deb did not produce exactly one mtrack-video package"
control="$(dpkg-deb -f "${debs[0]}")"
assert_contains "$control" "Package: mtrack-video" "video package name"
assert_contains "$control" "Depends:" "video package dependencies"
for dependency in "libasound2t64 | libasound2" libudev1 libgstreamer1.0-0 libgstreamer-plugins-base1.0-0 gstreamer1.0-plugins-base; do
  assert_contains "$control" "$dependency" "video package dependency $dependency"
done
assert_contains "$control" "Recommends: ola, gstreamer1.0-plugins-good, gstreamer1.0-plugins-bad, gstreamer1.0-libav" "video package recommendations"
assert_contains "$control" "Conflicts: mtrack" "video package conflicts"
assert_contains "$control" "Replaces: mtrack" "video package replaces"
assert_contains "$control" "Provides: mtrack" "video package provides"
ok "cargo-deb emits the mtrack-video package and expected control metadata"

rm -f "$TMP/debs"/*.deb
(
  cd "$ROOT"
  CARGO_TARGET_DIR="$TMP/target" cargo deb --no-build --no-strip --fast --output "$TMP/debs/" >/dev/null
)
base_debs=("$TMP"/debs/mtrack_*.deb)
[ "${#base_debs[@]}" -eq 1 ] && [ -f "${base_debs[0]}" ] || fail "cargo-deb did not produce exactly one base mtrack package"
base_control="$(dpkg-deb -f "${base_debs[0]}")"
assert_contains "$base_control" "Package: mtrack" "base package name"
assert_not_contains "$base_control" "gstreamer" "base package metadata must remain free of GStreamer"
ok "cargo-deb emits a GStreamer-free base package"

echo "All non-destructive packaging metadata checks passed."
