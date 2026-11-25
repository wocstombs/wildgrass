$ErrorActionPreference = 'Continue'

# Refresh PATH from the registry so tools installed earlier in the session are visible.
$machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$env:Path = (@($machinePath, $userPath) | Where-Object { $_ }) -join ';'

function Test-MpvInstalled {
    if (Get-Command mpv -ErrorAction SilentlyContinue) {
        return $true
    }
    if ($env:ProgramFiles) {
        $mpvExe = Join-Path $env:ProgramFiles 'MPV Player\mpv.exe'
        if (Test-Path $mpvExe) {
            return $true
        }
    }
    return $false
}

$missing = @()
if (-not (Test-MpvInstalled)) {
    $missing += @{ Name = 'mpv'; Id = 'shinchiro.mpv' }
}
if (-not (Get-Command yt-dlp -ErrorAction SilentlyContinue)) {
    $missing += @{ Name = 'yt-dlp'; Id = 'yt-dlp.yt-dlp' }
}
if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) {
    $missing += @{ Name = 'ffmpeg'; Id = 'Gyan.FFmpeg' }
}

if ($missing.Count -eq 0) {
    exit 0
}

if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
    $names = ($missing | ForEach-Object { $_.Name }) -join ', '
    Write-Host "winget is not available. Please install these dependencies manually: $names"
    exit 0
}

foreach ($dep in $missing) {
    Write-Host "Installing $($dep.Name) ($($dep.Id))..."
    & winget install --id $dep.Id -e --accept-source-agreements --accept-package-agreements
}

exit 0
