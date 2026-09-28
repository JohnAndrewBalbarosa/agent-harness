[CmdletBinding()]
param(
  [string]$RepositoryRoot,
  [string]$Version='8.30.1'
)

$ErrorActionPreference='Stop'
if(-not $RepositoryRoot){$RepositoryRoot=Split-Path -Parent $PSScriptRoot}  # PS 5.1: $PSScriptRoot is empty in param defaults
$RepositoryRoot=[IO.Path]::GetFullPath($RepositoryRoot)
$asset="gitleaks_${Version}_windows_x64.zip"
$release="https://github.com/gitleaks/gitleaks/releases/download/v$Version"
$work=Join-Path ([IO.Path]::GetTempPath()) "codex-harness-gitleaks-$Version"
if(Test-Path -LiteralPath $work){Remove-Item -LiteralPath $work -Recurse -Force}
New-Item -ItemType Directory -Path $work|Out-Null
try{
  $archive=Join-Path $work $asset
  $checksums=Join-Path $work 'checksums.txt'
  Invoke-WebRequest -Uri "$release/$asset" -OutFile $archive
  Invoke-WebRequest -Uri "$release/gitleaks_${Version}_checksums.txt" -OutFile $checksums
  $line=Get-Content -LiteralPath $checksums|Where-Object{$_ -match "\s+$([regex]::Escape($asset))$"}|Select-Object -First 1
  if(-not $line){throw "Official checksum for $asset was not found."}
  $expected=($line -split '\s+')[0].ToUpperInvariant()
  $actual=(Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash
  if($actual -ne $expected){throw 'Gitleaks archive checksum mismatch.'}
  Expand-Archive -LiteralPath $archive -DestinationPath $work -Force
  $exe=Join-Path $work 'gitleaks.exe'
  if(-not(Test-Path -LiteralPath $exe)){throw 'Pinned Gitleaks executable was not extracted.'}

  $canary=Join-Path $work 'canary'
  New-Item -ItemType Directory -Path $canary|Out-Null
  # Random per run: a sequential low-entropy token (e.g. ABC...xyz) is ignored by Gitleaks' entropy filter.
  $alphabet='ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789'.ToCharArray()
  $bytes=New-Object byte[] 36; $rng=[Security.Cryptography.RandomNumberGenerator]::Create(); $rng.GetBytes($bytes); $rng.Dispose()
  $synthetic='ghp_'+(-join ($bytes | ForEach-Object { $alphabet[$_ % $alphabet.Length] }))
  [IO.File]::WriteAllText((Join-Path $canary 'canary.txt'),('token = "'+$synthetic+'"'))
  # Gitleaks logs to stderr; Windows PowerShell 5.1 turns that into a terminating error under 'Stop'.
  $ErrorActionPreference='Continue'
  & $exe detect --no-git --source $canary --redact --no-banner --exit-code 23 2>&1 | Out-Null
  if($LASTEXITCODE -ne 23){throw "Gitleaks v$Version failed its synthetic detection canary."}

  & $exe detect --no-git --source $RepositoryRoot --redact --no-banner 2>&1 | ForEach-Object { "$_" } | Write-Host
  $ErrorActionPreference='Stop'
  if($LASTEXITCODE -ne 0){throw 'Gitleaks detected a potential secret in the repository.'}
  [pscustomobject]@{tool='gitleaks';version=$Version;checksum=$actual;canary='passed';scan='passed'}|ConvertTo-Json -Compress
}finally{
  if(Test-Path -LiteralPath $work){Remove-Item -LiteralPath $work -Recurse -Force}
}
