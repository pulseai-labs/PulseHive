#!/usr/bin/env bash
# d13 gate: a standalone consumer depending on `pulsehive` with only the
# `openai` transport feature must resolve and run without pulsehive-runtime
# or pulsehive-db in its graph, and stay under the 132-package baseline.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE="$ROOT/tests/fixtures/transport-only-consumer"

# Build artifacts land under the gitignored /target/ tree; an inherited
# CARGO_TARGET_DIR is honoured as-is.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target/transport-only}"

# Seed resolution from the workspace lockfile when present so the package
# count tracks the workspace's pinned versions. The fixture's Cargo.lock is
# gitignored, so copying it leaves `git status` unchanged.
if [[ -f "$ROOT/Cargo.lock" ]]; then
    cp "$ROOT/Cargo.lock" "$FIXTURE/Cargo.lock"
fi

# Optional comma list of extra meta-crate features, e.g.
# TRANSPORT_ONLY_EXTRA_FEATURES=runtime — proves the failure path. Each is
# passed as pulsehive/<feature> to every cargo call.
feature_list=""
if [[ -n "${TRANSPORT_ONLY_EXTRA_FEATURES:-}" ]]; then
    IFS=',' read -ra extra_features <<< "$TRANSPORT_ONLY_EXTRA_FEATURES"
    for f in "${extra_features[@]}"; do
        f="$(printf '%s' "$f" | tr -d '[:space:]')"
        [[ -n "$f" ]] && feature_list+="${feature_list:+,}pulsehive/$f"
    done
fi
feature_args=()
[[ -n "$feature_list" ]] && feature_args=(--features "$feature_list")

tree="$(cargo tree --manifest-path "$FIXTURE/Cargo.toml" --color never -e normal,build --prefix none --format '{p}' ${feature_args[@]+"${feature_args[@]}"})"
pkgs="$(printf '%s\n' "$tree" | sed 's/ (\*)$//' | sort -u)"

bad="$(printf '%s\n' "$pkgs" | grep -E '^(pulsehive-runtime|pulsehive-db) ' || true)"
if [[ -n "$bad" ]]; then
    while IFS= read -r line; do
        echo "transport-only check failed: ${line%% *} present in consumer graph" >&2
    done <<< "$bad"
    exit 1
fi

n="$(printf '%s\n' "$pkgs" | wc -l | tr -d ' ')"
echo "resolved packages: $n (baseline 132)"
if (( n >= 132 )); then
    echo "transport-only check failed: $n resolved packages >= 132" >&2
    exit 1
fi

out="$(cargo run --quiet --manifest-path "$FIXTURE/Cargo.toml" ${feature_args[@]+"${feature_args[@]}"})"
printf '%s\n' "$out"
if [[ "$out" != *"transport-only consumer ready: model="* ]]; then
    echo "transport-only check failed: consumer did not print the ready line" >&2
    exit 1
fi
echo "consumer ran: ok"
