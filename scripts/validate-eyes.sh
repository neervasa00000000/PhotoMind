#!/bin/sh
set -e
cd "$(dirname "$0")/../src-tauri"
PHOTOS="${1:-../eye-validation/photos}"
GT="${2:-../eye-validation/ground_truth.json}"
exec cargo run --bin validate_eyes --release -- "$PHOTOS" --ground-truth "$GT"
