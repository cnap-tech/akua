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

# Rewrite go-src to use the forked path.
go_src="$crate_root/go-src"
relative_fork="../third_party/helm-fork"
echo "[fork/apply.sh] rewriting go.mod replace directive → $relative_fork"
(cd "$go_src" && go mod edit -replace "helm.sh/helm/v4=$relative_fork" && go mod tidy)

echo "[fork/apply.sh] done. Build with: task build:helm-engine-wasm"
