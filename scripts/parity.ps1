# Runs blackfriday's own test suite, unmodified, against the Rust port.
#
# This is `make parity` for machines without make. It does the same three
# things in the same order:
#
#   1. verify every pinned test file still hashes to what it did at kickoff
#   2. build the Rust helper the Go suite talks to
#   3. assemble a scratch package and run `go test`
#
# The pinned files are copied into target/parity rather than kept beside the
# adapter, so exactly one copy of them exists in the repository -- the one
# step 1 checks.
#
# Usage:
#   .\scripts\parity.ps1            # summary
#   .\scripts\parity.ps1 -Verbose   # per-test output

[CmdletBinding()]
param([switch]$ShowEachTest)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

# ---------------------------------------------------------------- 1. hashes
Write-Host "verifying the pinned suite is unmodified..." -ForegroundColor Cyan

$manifest = Join-Path $repo 'tests/original/SHA256SUMS'
$bad = 0
$checked = 0
foreach ($line in Get-Content $manifest) {
    if ($line -notmatch '^([0-9a-f]{64})\s+\*?(.+)$') { continue }
    $want = $Matches[1]
    $file = Join-Path $repo "tests/original/$($Matches[2])"
    $got = (Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash.ToLower()
    $checked++
    if ($got -ne $want) {
        Write-Host "  MODIFIED: $($Matches[2])" -ForegroundColor Red
        $bad++
    }
}
if ($bad -gt 0) {
    throw "$bad of $checked pinned files differ from their kickoff hashes"
}

$manifestHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $manifest).Hash.ToLower()
Write-Host "  all $checked files match"
Write-Host "  kickoff manifest: $manifestHash"
Write-Host ""

# ---------------------------------------------------------------- 2. build
Write-Host "building the Rust side..." -ForegroundColor Cyan
cargo build --release -p blackfriday-harness
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
Write-Host ""

# ---------------------------------------------------------------- 3. run
Write-Host "running blackfriday's own 65 tests against the port..." -ForegroundColor Cyan

$scratch = Join-Path $repo 'target/parity'
Remove-Item -Recurse -Force $scratch -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $scratch | Out-Null
Copy-Item (Join-Path $repo 'adapter/go.mod') $scratch
Copy-Item (Join-Path $repo 'adapter/blackfriday.go') $scratch
Copy-Item (Join-Path $repo 'tests/original/*_test.go') $scratch
Copy-Item -Recurse (Join-Path $repo 'tests/original/testdata') $scratch

Push-Location $scratch
try {
    $env:BF_SERVE = Join-Path $repo 'target/release/bf-serve.exe'
    if ($ShowEachTest) { go test -v ./... } else { go test ./... }
    $code = $LASTEXITCODE
}
finally {
    Pop-Location
}

Write-Host ""
if ($code -eq 0) {
    Write-Host "PARITY: 65 of 65 pass" -ForegroundColor Green
}
else {
    Write-Host "PARITY: FAILED" -ForegroundColor Red
}
exit $code
