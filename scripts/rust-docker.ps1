<#
.SYNOPSIS
  Runs cargo for the Rust workspace inside Docker (no local Rust install needed).
.EXAMPLE
  ./scripts/rust-docker.ps1 fmt; ./scripts/rust-docker.ps1 clippy; ./scripts/rust-docker.ps1 test
  ./scripts/rust-docker.ps1 build-windows   # -> target-windows/x86_64-pc-windows-gnu/release/*.exe
#>
param(
  [Parameter(Mandatory)][ValidateSet('fmt', 'fmt-check', 'check', 'clippy', 'test', 'build-linux', 'build-windows', 'shell')]
  [string]$Task
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$image = 'agent-harness-rust-builder:1.98'

docker image inspect $image *> $null
if ($LASTEXITCODE -ne 0) {
  docker build -q -t $image -f (Join-Path $repo 'docker/rust-builder.Dockerfile') (Join-Path $repo 'docker') | Out-Null
  if ($LASTEXITCODE -ne 0) { throw "failed to build $image" }
}

$cargo = switch ($Task) {
  'fmt'           { 'cargo fmt --all' }
  'fmt-check'     { 'cargo fmt --all -- --check' }
  'check'         { 'cargo check --workspace --all-targets' }
  'clippy'        { 'cargo clippy --workspace --all-targets -- -D warnings' }
  'test'          { 'cargo test --workspace' }
  'build-linux'   { 'cargo build --workspace --release' }
  'build-windows' { 'cargo build --workspace --release --target x86_64-pc-windows-gnu && mkdir -p target-windows && cp -r target/x86_64-pc-windows-gnu/release/*.exe target-windows/' }
  'shell'         { 'bash' }
}

$interactive = if ($Task -eq 'shell') { '-it' } else { '-i' }
docker run --rm $interactive `
  -v "${repo}:/work" `
  -v agent-harness-cargo-registry:/usr/local/cargo/registry `
  -v agent-harness-cargo-target:/work/target `
  -w /work $image bash -c $cargo
exit $LASTEXITCODE
