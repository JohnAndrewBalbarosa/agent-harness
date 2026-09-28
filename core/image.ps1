param(
  [Parameter(Mandatory)][ValidateSet('size', 'resize')][string]$Mode,
  [Parameter(Mandatory)][string]$Source,
  [string]$Target,
  [int]$Width,
  [int]$Height
)
# Windows PowerShell 5.1 System.Drawing helper for core/image.py (no extra install).
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$image = [System.Drawing.Image]::FromFile($Source)
try {
  if ($Mode -eq 'size') { '{0} {1}' -f $image.Width, $image.Height; return }
  $bitmap = New-Object System.Drawing.Bitmap($Width, $Height)
  $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
  try {
    $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $graphics.DrawImage($image, 0, 0, $Width, $Height)
  } finally { $graphics.Dispose() }
  try { $bitmap.Save($Target, [System.Drawing.Imaging.ImageFormat]::Png) } finally { $bitmap.Dispose() }
} finally { $image.Dispose() }
