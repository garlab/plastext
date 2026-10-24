#!/usr/bin/env bash
# Only the app, ui and editor crates may depend on GPUI, directly or not (spec §7.3).
set -euo pipefail
cd "$(dirname "$0")/.."

failed=0
for manifest in crates/*/Cargo.toml; do
  name=$(sed -n 's/^name = "\(.*\)"$/\1/p' "$manifest" | head -n 1)
  case "$name" in
    plastext-app | plastext-ui | plastext-editor) continue ;;
  esac
  # Not `grep -q`: exiting early would SIGPIPE `cargo tree` and fail the pipeline.
  if cargo tree --quiet -p "$name" -e normal --prefix none | grep '^gpui v' >/dev/null; then
    echo "error: $name depends on gpui; only app, ui and editor may (spec §7.3)" >&2
    failed=1
  fi
done
exit "$failed"
