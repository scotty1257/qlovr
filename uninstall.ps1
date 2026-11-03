# Removes everything install.ps1 added. Leaves recent files, notes and settings alone.
$ErrorActionPreference = 'Stop'

$installDir = Join-Path $env:LOCALAPPDATA 'Programs\FastLogViewer'

foreach ($path in @(
        'Software\Classes\*\shell\FastLogViewer',
        'Software\Classes\Applications\fastlogviewer.exe',
        'Software\Classes\.log\OpenWithList\fastlogviewer.exe')) {
    [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($path, $false)
}

Remove-Item (Join-Path ([Environment]::GetFolderPath('Programs')) 'FastLogViewer.lnk') -ErrorAction SilentlyContinue
if (Test-Path $installDir) {
    try {
        Remove-Item $installDir -Recurse -Force
    } catch {
        throw "Couldn't remove $installDir - close any open FastLogViewer windows and run this again."
    }
}

Write-Host 'FastLogViewer uninstalled'
