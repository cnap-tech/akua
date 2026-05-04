#!/bin/sh
# Apply the client-go strip fork on helm v4.1.4.
#
# Idempotent: checks for an existing `third_party/helm-fork/` with the
# patch already applied; re-applies only if the checkout is missing or
# pristine. Exits cleanly if everything's already in place.
#
# Called by `task build:helm-engine-wasm:forked` and CI. Writes to
# `../third_party/helm-fork/`, then rewrites `../go-src/go.mod` to
# `replace` upstream with the fork. Both paths are gitignored.

set -eu

here="$(cd "$(dirname "$0")" && pwd)"
crate_root="$(cd "$here/.." && pwd)"
fork_dir="$crate_root/third_party/helm-fork"
patch="$here/helm-v4.1.4.patch"
helm_tag="v4.1.4"

mkdir -p "$crate_root/third_party"

# Concurrency guard: serialize parallel apply.sh invocations. The
# Taskfile graph already converges on a single call after the
# build:engines→sdk:test cleanup, but defending here too keeps the
# script safe to invoke directly (e.g. local task fan-out, future
# additions). mkdir is atomic across POSIX, so the lock-acquire
# loop is portable without flock(1).
lock="$crate_root/third_party/.apply.lock"
while ! mkdir "$lock" 2>/dev/null; do
    sleep 0.5
done
trap 'rmdir "$lock" 2>/dev/null || true' EXIT

# Validate any cached fork tree is healthy before trusting it. CI
# restores the helm-fork directory from actions/cache; a partial or
# corrupted restore leaves `.git` present but `git status` exiting 128
# (fatal), which previously crashed the patched-already check below.
# Treat a sick `.git` as "no fork tree" — re-clone from scratch.
if [ -d "$fork_dir/.git" ] && ! (cd "$fork_dir" && git status --porcelain >/dev/null 2>&1); then
    echo "[fork/apply.sh] $fork_dir/.git is unhealthy (git status failed); re-cloning."
    rm -rf "$fork_dir"
fi

# Fast path: fork tree already patched (committed by us on top of the
# upstream v4.1.4 tag). Use `git apply --check --reverse` as the
# idempotence test — it succeeds iff the patch is fully applied, fails
# otherwise. No sentinel files, no separate state to get out of sync.
if [ -d "$fork_dir/.git" ]; then
    if (cd "$fork_dir" && git apply --check --reverse "$patch") 2>/dev/null; then
        echo "[fork/apply.sh] $fork_dir is already patched — skipping."
        exit 0
    fi
    # Guard: refuse to operate on a fork dir with uncommitted hand-edits,
    # those would be silently blown away by a clean re-apply.
    if ! (cd "$fork_dir" && git diff --quiet) || ! (cd "$fork_dir" && git diff --quiet --cached); then
        echo "[fork/apply.sh] $fork_dir has uncommitted changes; refusing to modify. Reset the tree or delete third_party/ to rebuild from scratch."
        exit 1
    fi
fi

# Clone upstream helm at the pinned tag (fresh or re-fetch).
if [ ! -d "$fork_dir/.git" ]; then
    echo "[fork/apply.sh] cloning helm $helm_tag into $fork_dir"
    rm -rf "$fork_dir"
    git clone --depth 1 --branch "$helm_tag" https://github.com/helm/helm.git "$fork_dir"
fi

echo "[fork/apply.sh] applying $patch"
(cd "$fork_dir" && patch -p1 < "$patch")

# Commit the patched state. Without this, the working tree stays dirty
# forever and a second concurrent invocation (e.g. fan-in from multiple
# `task` deps reaching `build:helm-engine-wasm` at once) trips the
# "refusing to modify" guard on `git diff --quiet` and exits 1. With
# the commit, `git apply --check --reverse` correctly detects the
# patched-and-committed state on every subsequent call.
(cd "$fork_dir" \
    && git add -A \
    && git -c user.email=akua-fork@local -c user.name=akua-fork \
           commit -q -m "akua: client-go strip patch")

# Rewrite go-src to use the forked path.
go_src="$crate_root/go-src"
relative_fork="../third_party/helm-fork"
echo "[fork/apply.sh] rewriting go.mod replace directive → $relative_fork"
(cd "$go_src" && go mod edit -replace "helm.sh/helm/v4=$relative_fork" && go mod tidy)

echo "[fork/apply.sh] done. Build with: task build:helm-engine-wasm"
