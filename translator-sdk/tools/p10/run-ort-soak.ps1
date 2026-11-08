param([int]$Rounds=10000)
$Root='E:\augenstern\translate'
Set-Location (Join-Path $Root 'translator-sdk')
. (Join-Path $Root 'translator-sdk\tools\p5\windows-build-env.ps1') -Root $Root | Out-Null
$env:P4_ORT_MODEL_DIR = Join-Path $Root '.build\model-cache\ort-model'
$env:P10_CHECKPOINT = Join-Path $Root ".build\reports\p10-ort-$Rounds.checkpoint.jsonl"
$env:P10_RUN_ID = "p10-$Rounds-" + [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
$log = Join-Path $Root ".build\reports\p10-ort-$Rounds.log"
& cargo run -p translator-nmt-ort --features ort-runtime --release --example p10_soak -- $Rounds *> $log
$code=$LASTEXITCODE
Write-Output ("EXIT=" + $code)
Write-Output ("LOG=" + $log)
Write-Output ("CHECKPOINT=" + $env:P10_CHECKPOINT)
Get-Content $log -Tail 6
