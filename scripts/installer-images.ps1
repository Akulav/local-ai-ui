# Regenerates the NSIS installer bitmaps from src-tauri\icons\source.png:
#   src-tauri\installer\header.bmp   150 x 57  (top of the installer pages)
#   src-tauri\installer\sidebar.bmp  164 x 314 (welcome / finish pages)
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

# sidebar: dark gradient (the app's background), logo, name and tagline
$bmp, $g = New-Canvas 164 314
$rect = New-Object System.Drawing.Rectangle 0, 0, 164, 314
$top = [System.Drawing.Color]::FromArgb(255, 22, 26, 40)
$bottom = [System.Drawing.Color]::FromArgb(255, 11, 13, 18)
$grad = New-Object System.Drawing.Drawing2D.LinearGradientBrush $rect, $top, $bottom, 90
$g.FillRectangle($grad, $rect)
$glow = New-Object System.Drawing.Drawing2D.GraphicsPath
$glow.AddEllipse(2, 40, 160, 160)
$pgb = New-Object System.Drawing.Drawing2D.PathGradientBrush $glow
$pgb.CenterColor = [System.Drawing.Color]::FromArgb(90, 124, 108, 255)
$pgb.SurroundColors = @([System.Drawing.Color]::FromArgb(0, 124, 108, 255))
$g.FillPath($pgb, $glow)
$g.DrawImage($logo, 42, 80, 80, 80)
$white = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 231, 234, 242))
$grey = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 170, 177, 195))
$center = New-Object System.Drawing.StringFormat
$center.Alignment = "Center"
$g.DrawString("Lumen", (New-Object System.Drawing.Font "Segoe UI Semibold", 17), $white, (New-Object System.Drawing.RectangleF 0, 176, 164, 34), $center)
$g.DrawString("Local AI on your PC", (New-Object System.Drawing.Font "Segoe UI", 8.5), $grey, (New-Object System.Drawing.RectangleF 0, 210, 164, 20), $center)
$g.DrawString("made by Akulav", (New-Object System.Drawing.Font "Segoe UI", 7.5), $grey, (New-Object System.Drawing.RectangleF 0, 284, 164, 18), $center)
$g.Dispose()
$bmp.Save((Join-Path $out "sidebar.bmp"), [System.Drawing.Imaging.ImageFormat]::Bmp)
$bmp.Dispose()

# header: white like the page header, logo on the right
$bmp, $g = New-Canvas 150 57
$g.Clear([System.Drawing.Color]::White)
$g.DrawImage($logo, 98, 7, 43, 43)
$g.Dispose()
$bmp.Save((Join-Path $out "header.bmp"), [System.Drawing.Imaging.ImageFormat]::Bmp)
$bmp.Dispose()
$logo.Dispose()
Write-Host "wrote $out\sidebar.bmp and header.bmp"
