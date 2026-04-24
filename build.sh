#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

echo "→ building wasm skill (wasm32-unknown-unknown, release)…"
cargo build -p mission-skill --target wasm32-unknown-unknown --release

WASM_SRC="$ROOT/target/wasm32-unknown-unknown/release/mission_skill.wasm"
WASM_DST="$ROOT/mission_skill.wasm"
cp "$WASM_SRC" "$WASM_DST"
echo "  skill → $WASM_DST ($(du -h "$WASM_DST" | cut -f1))"

echo "→ building host CLI (release)…"
cargo build -p mission-analyst --release

echo
echo "done. try:"
echo "  ./target/release/mission-analyst space_missions.log"
echo "  ./target/release/mission-analyst space_missions.log --bench"
