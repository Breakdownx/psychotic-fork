# Run the terminal UI from this checkout.
$manifest = Join-Path $PSScriptRoot 'Cargo.toml'
& cargo run --locked --release --manifest-path $manifest -- --tui @args
exit $LASTEXITCODE
