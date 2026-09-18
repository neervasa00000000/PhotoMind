#!/bin/sh
set -e
cd "$(dirname "$0")/../src-tauri"
exec cargo run --bin label_eyes --release -- "$@"
