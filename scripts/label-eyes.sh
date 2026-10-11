#!/bin/sh
set -e
cd "$(dirname "$0")/../src-tauri"
exec cargo run --features dev-tools --bin label_eyes --release -- "$@"
