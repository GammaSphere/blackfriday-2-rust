# Runs both benchmark programs and reports the numbers side by side.
#
# Latency and throughput are measured inside each program (bench/rust/main.rs
# and bench/go/main.go, which are the same program written twice). Peak memory
# and startup are measured from out here, because asking a process how much
# memory it is using needs either unsafe or a dependency on the Rust side, and
# this repository claims neither.
#
# Usage:
#   pwsh bench/run.ps1 [-Iterations 200] [-Repeats 5]
#
# -Repeats runs each binary several times and keeps the best, which suppresses
# scheduler noise on a machine that is not otherwise quiet.

[CmdletBinding()]
param(
    [int]$Iterations = 40,
    [int]$Batch = 40,
    [int]$Repeats = 5,
    [string]$Corpus = "tests/original/testdata"
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

$rustBin = Join-Path $repo 'target/release/bf-bench.exe'
$goBin = Join-Path $repo 'bench/go/bf-bench-go.exe'
$corpusAbs = Join-Path $repo $Corpus

if (-not (Test-Path $rustBin)) { throw "missing $rustBin -- cargo build --release -p blackfriday-bench" }
if (-not (Test-Path $goBin)) { throw "missing $goBin -- (cd bench/go; go build -o bf-bench-go.exe .)" }

function Invoke-Bench {
    param([string]$Exe)
    $raw = & $Exe -corpus $corpusAbs -n $Iterations -batch $Batch
    $map = @{}
    foreach ($line in $raw) {
        if ($line -match '^([a-z0-9_]+)=(.*)$') { $map[$Matches[1]] = $Matches[2] }
    }
    return $map
}

# Peak working set of one full benchmark run.
#
# Sampled by polling rather than read from PeakWorkingSet64 after exit: that
# property came back as zero here, and a number that is silently zero is worse
# than no number at all.
function Measure-PeakMemory {
    param([string]$Exe)
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Exe
    $psi.Arguments = "-corpus `"$corpusAbs`" -n 40 -batch 40"
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $p = [System.Diagnostics.Process]::Start($psi)
    $peak = 0
    while (-not $p.HasExited) {
        try {
            $p.Refresh()
            if ($p.WorkingSet64 -gt $peak) { $peak = $p.WorkingSet64 }
        } catch { }
        Start-Sleep -Milliseconds 5
    }
    $null = $p.StandardOutput.ReadToEnd()
    $p.WaitForExit()
    return [math]::Round($peak / 1MB, 2)
}

# Wall time of a process that starts, renders one byte, and exits.
function Measure-Startup {
    param([string]$Exe, [int]$Samples = 30)
    $times = @()
    for ($i = 0; $i -lt $Samples; $i++) {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        & $Exe -mode startup | Out-Null
        $sw.Stop()
        $times += $sw.Elapsed.TotalMilliseconds
    }
    $sorted = $times | Sort-Object
    return [math]::Round($sorted[[int]($sorted.Count / 2)], 2)
}

Write-Host "corpus:     $corpusAbs"
Write-Host "iterations: $Iterations  batch: $Batch  repeats: $Repeats"
Write-Host ""

$results = @{}
foreach ($side in @(@{n = 'rust'; e = $rustBin }, @{n = 'go'; e = $goBin })) {
    $best = $null
    for ($r = 0; $r -lt $Repeats; $r++) {
        $m = Invoke-Bench -Exe $side.e
        if ($null -eq $best -or [double]$m['p50_ms'] -lt [double]$best['p50_ms']) { $best = $m }
    }
    $best['peak_mib'] = Measure-PeakMemory -Exe $side.e
    $best['startup_ms'] = Measure-Startup -Exe $side.e
    $results[$side.n] = $best
    Write-Host "$($side.n): done"
}

$rs = $results['rust']
$go = $results['go']

function Ratio { param($a, $b) if ([double]$b -eq 0) { return 'n/a' } return ('{0:N2}x' -f ([double]$b / [double]$a)) }

Write-Host ""
Write-Host ("{0,-22} {1,12} {2,12} {3,10}" -f 'metric', 'rust', 'go', 'rust is')
Write-Host ("-" * 60)
foreach ($k in @('min_ms', 'p50_ms', 'p90_ms', 'p99_ms', 'max_ms')) {
    Write-Host ("{0,-22} {1,12} {2,12} {3,10}" -f $k, $rs[$k], $go[$k], (Ratio $rs[$k] $go[$k]))
}
Write-Host ("{0,-22} {1,12} {2,12} {3,10}" -f 'throughput_mib_s', $rs['throughput_mib_s'], $go['throughput_mib_s'], (Ratio $go['throughput_mib_s'] $rs['throughput_mib_s']))
Write-Host ("{0,-22} {1,12} {2,12} {3,10}" -f 'peak_mib', $rs['peak_mib'], $go['peak_mib'], (Ratio $rs['peak_mib'] $go['peak_mib']))
Write-Host ("{0,-22} {1,12} {2,12} {3,10}" -f 'startup_ms', $rs['startup_ms'], $go['startup_ms'], (Ratio $rs['startup_ms'] $go['startup_ms']))
Write-Host ""
Write-Host ("documents={0} corpus_bytes={1}" -f $rs['documents'], $rs['corpus_bytes'])
