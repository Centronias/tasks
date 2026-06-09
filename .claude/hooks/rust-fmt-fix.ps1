$json = [System.Console]::In.ReadToEnd() | ConvertFrom-Json -ErrorAction SilentlyContinue
if (-not $json) { exit 0 }

$filePath = $json.tool_input.file_path
if (-not $filePath -or $filePath -notmatch '\.rs$') { exit 0 }

cargo fmt
cargo clippy --fix --allow-dirty --allow-staged
exit 0
