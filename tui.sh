#!/bin/sh
# Run the terminal UI from this checkout.
set -eu
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
if [ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = arm64 ]; then
    version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$project_dir/Cargo.toml" | head -n 1)
    binary="$project_dir/release/AG_${version}_macos/ag_unlocker"
    if [ -x "$binary" ]; then
        exec "$binary" --tui "$@"
    fi
fi
exec cargo run --locked --release --manifest-path "$project_dir/Cargo.toml" -- --tui "$@"
