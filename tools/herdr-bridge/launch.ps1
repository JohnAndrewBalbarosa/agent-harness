param(
    [Parameter(Mandatory=$true)]
    [ValidateSet('personal','cy','feu')]
    [string]$Profile,
    [Parameter(Mandatory=$true)]
    [string]$Project,
    [string]$Label = '',
    [switch]$NewWindow,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$argumentJson = if ([Console]::IsInputRedirected) { [Console]::In.ReadToEnd() } else { '[]' }
$CodexArguments = if ([string]::IsNullOrWhiteSpace($argumentJson)) { @() } else { @($argumentJson | ConvertFrom-Json) }
$resolvedProject = (Resolve-Path -LiteralPath $Project).Path
$herdr = Join-Path $env:LOCALAPPDATA 'Programs\Herdr\bin\herdr.exe'
if (-not (Test-Path -LiteralPath $herdr)) { throw "Herdr is not installed at $herdr" }

$profileHomes = @{
    personal = (Join-Path $env:USERPROFILE '.codex')
    cy = (Join-Path $env:USERPROFILE '.codex-cy')
    feu = (Join-Path $env:USERPROFILE '.codex-feu')
}
$codexHome = $profileHomes[$Profile]
if (-not (Test-Path -LiteralPath $codexHome)) { throw "Missing Codex profile home: $codexHome" }

$launcher = Join-Path $env:LOCALAPPDATA "Microsoft\WindowsApps\codex-$Profile.cmd"
if (-not (Test-Path -LiteralPath $launcher)) { throw "Missing Codex profile launcher: $launcher" }

function ConvertTo-PowerShellLiteral([string]$Value) {
    return "'" + $Value.Replace("'", "''") + "'"
}

# A bare `herdr` process not parented by another herdr.exe is an attached client window.
# The Herdr server is persistent, so a new workspace appears in any attached client: no extra window needed.
function Test-HerdrClientAttached {
    $herdrProcesses = @(Get-CimInstance Win32_Process -Filter "Name='herdr.exe'")
    $herdrIds = @($herdrProcesses | ForEach-Object { [int]$_.ProcessId })
    foreach ($process in $herdrProcesses) {
        $arguments = ([string]$process.CommandLine -replace '^\s*("[^"]*"|\S+)\s*', '').Trim()
        $parentIsHerdr = $herdrIds -contains [int]$process.ParentProcessId
        if (-not $arguments -and -not $parentIsHerdr -and [int]$process.ParentProcessId -ne $PID) { return $true }
    }
    return $false
}

$commandParts = @('&', (ConvertTo-PowerShellLiteral $launcher))
$commandParts += @($CodexArguments | ForEach-Object { ConvertTo-PowerShellLiteral $_ })
$command = $commandParts -join ' '

if ($DryRun) {
    [pscustomobject]@{
        profile = $Profile
        project = $resolvedProject
        launcher = $launcher
        arguments = @($CodexArguments)
        command = $command
    } | ConvertTo-Json -Compress
    exit 0
}

$shell = (Get-Command pwsh.exe -ErrorAction SilentlyContinue).Source
if (-not $shell) { $shell = (Get-Command powershell.exe).Source }
$escapedHerdr = $herdr.Replace("'", "''")
if ($NewWindow -or -not (Test-HerdrClientAttached)) {
    Start-Process -FilePath $shell -WorkingDirectory $resolvedProject -ArgumentList @('-NoExit','-Command',"& '$escapedHerdr'")
}
$deadline = (Get-Date).AddSeconds(15)
do {
    Start-Sleep -Milliseconds 250
    $status = & $herdr status --json 2>$null | ConvertFrom-Json -ErrorAction SilentlyContinue
} until (($status -and $status.server.running) -or (Get-Date) -ge $deadline)
if (-not $status -or -not $status.server.running) { throw 'Herdr server did not become ready within 15 seconds.' }

if (-not $Label) { $Label = "$(Split-Path -Leaf $resolvedProject) [$($Profile.ToUpperInvariant())]" }
$workspace = & $herdr workspace create --cwd $resolvedProject --label $Label --focus | ConvertFrom-Json
if ($workspace.error) { throw $workspace.error.message }
$pane = if ($workspace.result.root_pane.pane_id) { $workspace.result.root_pane.pane_id } elseif ($workspace.result.pane_id) { $workspace.result.pane_id } else { $null }
if (-not $pane) { throw 'Herdr did not return a root pane id.' }

$runOutput = & $herdr pane run $pane $command 2>&1
if ($LASTEXITCODE -ne 0) { throw "Herdr pane launch failed: $($runOutput | Select-Object -Last 1)" }
[pscustomobject]@{ profile=$Profile; project=$resolvedProject; workspace=$workspace.result.workspace.workspace_id; pane=$pane; arguments=@($CodexArguments) } | ConvertTo-Json -Compress
