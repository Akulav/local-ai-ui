# Regenerates the Windows setup's wizard pictures (Inno Setup) from src-tauri\icons\source.png:
#   src-tauri\installer\wizard-large.bmp      164 x 314  (Welcome / Finish pages)
#   src-tauri\installer\wizard-large-200.bmp  328 x 628  (the same at 200% display scaling)
#   src-tauri\installer\wizard-small.bmp       55 x 55   (top right of the other pages)
#   src-tauri\installer\wizard-small-200.bmp  110 x 110
# Run it after changing the logo:  powershell -ExecutionPolicy Bypass -File scripts\installer-images.ps1
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
$root = Split-Path -Parent $PSScriptRoot
$src = Join-Path $root "src-tauri\icons\source.png"
$out = Join-Path $root "src-tauri\installer"
New-Item -ItemType Directory -Force $out | Out-Null
$logo = [System.Drawing.Image]::FromFile($src)

function New-Canvas($w, $h) {
    $bmp = New-Object System.Drawing.Bitmap $w, $h, ([System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = "AntiAlias"
    $g.InterpolationMode = "HighQualityBicubic"
    $g.TextRenderingHint = "AntiAliasGridFit"
    return @($bmp, $g)
}

# Welcome / Finish: dark gradient (the app's background), logo glow, name, tagline and the credit
function Write-Large($scale, $name) {
    $w = 164 * $scale; $h = 314 * $scale
    $bmp, $g = New-Canvas $w $h
    $rect = New-Object System.Drawing.Rectangle 0, 0, $w, $h
    $top = [System.Drawing.Color]::FromArgb(255, 22, 26, 40)
    $bottom = [System.Drawing.Color]::FromArgb(255, 11, 13, 18)
    $grad = New-Object System.Drawing.Drawing2D.LinearGradientBrush $rect, $top, $bottom, 90
    $g.FillRectangle($grad, $rect)
    $glow = New-Object System.Drawing.Drawing2D.GraphicsPath
    $glow.AddEllipse(2 * $scale, 40 * $scale, 160 * $scale, 160 * $scale)
    $pgb = New-Object System.Drawing.Drawing2D.PathGradientBrush $glow
    $pgb.CenterColor = [System.Drawing.Color]::FromArgb(90, 124, 108, 255)
    $pgb.SurroundColors = @([System.Drawing.Color]::FromArgb(0, 124, 108, 255))
    $g.FillPath($pgb, $glow)
    $g.DrawImage($logo, 42 * $scale, 80 * $scale, 80 * $scale, 80 * $scale)
    $white = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 231, 234, 242))
    $grey = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 170, 177, 195))
    $center = New-Object System.Drawing.StringFormat
    $center.Alignment = "Center"
    $px = [System.Drawing.GraphicsUnit]::Pixel
    $g.DrawString("Lumen", (New-Object System.Drawing.Font "Segoe UI Semibold", (23 * $scale), $px), $white, (New-Object System.Drawing.RectangleF 0, (176 * $scale), $w, (34 * $scale)), $center)
    $g.DrawString("Local AI on your PC", (New-Object System.Drawing.Font "Segoe UI", (11.5 * $scale), $px), $grey, (New-Object System.Drawing.RectangleF 0, (210 * $scale), $w, (20 * $scale)), $center)
    $g.DrawString("Made by Akulav", (New-Object System.Drawing.Font "Segoe UI", (10 * $scale), $px), $grey, (New-Object System.Drawing.RectangleF 0, (284 * $scale), $w, (18 * $scale)), $center)
    $g.Dispose()
    $bmp.Save((Join-Path $out $name), [System.Drawing.Imaging.ImageFormat]::Bmp)
    $bmp.Dispose()
}

# the other pages: the logo on white (the page header's colour)
function Write-Small($size, $name) {
    $bmp, $g = New-Canvas $size $size
    $g.Clear([System.Drawing.Color]::White)
    $pad = [int]($size * 0.06)
    $g.DrawImage($logo, $pad, $pad, $size - 2 * $pad, $size - 2 * $pad)
    $g.Dispose()
    $bmp.Save((Join-Path $out $name), [System.Drawing.Imaging.ImageFormat]::Bmp)
    $bmp.Dispose()
}

Write-Large 1 "wizard-large.bmp"
Write-Large 2 "wizard-large-200.bmp"
Write-Small 55 "wizard-small.bmp"
Write-Small 110 "wizard-small-200.bmp"
$logo.Dispose()
Write-Host "wrote the wizard pictures to $out"
