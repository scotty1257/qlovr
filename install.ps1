# Builds FastLogViewer and installs it for the current user (no admin rights needed):
#   - copies the exe to %LOCALAPPDATA%\Programs\FastLogViewer
#   - adds "Open in FastLogViewer" to the right-click menu of every file
#   - lists it under "Open with" so it can be made the default for .log files
#   - adds a Start menu shortcut
# Run again to update to a new build; run uninstall.ps1 to remove it all.
$ErrorActionPreference = 'Stop'

$installDir = Join-Path $env:LOCALAPPDATA 'Programs\FastLogViewer'
$exe = Join-Path $installDir 'fastlogviewer.exe'
$icon = "$env:SystemRoot\System32\SHELL32.dll,260"
$openCommand = "`"$exe`" `"%1`""

cargo build --release -p fastlogviewer --manifest-path (Join-Path $PSScriptRoot 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

New-Item -ItemType Directory -Force $installDir | Out-Null
try {
    Copy-Item (Join-Path $PSScriptRoot 'target\release\fastlogviewer.exe') $exe -Force
} catch {
    throw "Couldn't replace $exe - close any open FastLogViewer windows and run this again."
}

# Registry keys go through .NET because PowerShell treats the '*' key name as a wildcard.
function Set-RegistryKey([string]$path, [hashtable]$values) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($path)
    foreach ($name in $values.Keys) { $key.SetValue($name, $values[$name]) }
    $key.Close()
}

# Right-click menu on every file (on Windows 11 it's under "Show more options").
Set-RegistryKey 'Software\Classes\*\shell\FastLogViewer' @{ '' = 'Open in FastLogViewer'; 'Icon' = $icon }
Set-RegistryKey 'Software\Classes\*\shell\FastLogViewer\command' @{ '' = $openCommand }

# "Open with" entry.
Set-RegistryKey 'Software\Classes\Applications\fastlogviewer.exe' @{ 'FriendlyAppName' = 'FastLogViewer' }
Set-RegistryKey 'Software\Classes\Applications\fastlogviewer.exe\DefaultIcon' @{ '' = $icon }
Set-RegistryKey 'Software\Classes\Applications\fastlogviewer.exe\shell\open\command' @{ '' = $openCommand }
Set-RegistryKey 'Software\Classes\.log\OpenWithList\fastlogviewer.exe' @{}

$shortcutPath = Join-Path ([Environment]::GetFolderPath('Programs')) 'FastLogViewer.lnk'
$shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcutPath)
$shortcut.TargetPath = $exe
$shortcut.IconLocation = $icon
$shortcut.Save()

Write-Host "Installed to $exe"
