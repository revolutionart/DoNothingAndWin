param([string]$Iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe")
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
$repo = (Get-Location).Path
$probe = Join-Path $repo ('review/launcher-update-' + [guid]::NewGuid().ToString('N'))
$payload = Join-Path $probe 'payload'
$destination = Join-Path $probe 'installed launcher'
New-Item -ItemType Directory -Path $payload | Out-Null
# This fixture never runs the actual launcher or changes its installation.
$source = @'
using System;
using System.IO;
public class Probe {
    public static int Main(string[] args) {
        string root = Environment.GetEnvironmentVariable("DNW_UPDATE_PROBE");
        if (String.IsNullOrEmpty(root)) return 3;
        if (args.Length == 1 && args[0] == "--check-running") {
            string mode = File.ReadAllText(Path.Combine(root, "mode.txt"));
            string until = Path.Combine(root, "until.txt");
            if (mode == "delay" && !File.Exists(until))
                File.WriteAllText(until, DateTime.UtcNow.AddSeconds(2).Ticks.ToString());
            bool busy = mode == "busy" || (mode == "delay" && DateTime.UtcNow.Ticks < Int64.Parse(File.ReadAllText(until)));
            File.AppendAllText(Path.Combine(root, "checks.txt"), busy ? "busy\n" : "idle\n");
            return busy ? 2 : 0;
        }
        File.WriteAllText(Path.Combine(root, "restarted.txt"), String.Join(" ", args));
        return 0;
    }
}
'@
$sourcePath = Join-Path $probe 'Probe.cs'
Set-Content -LiteralPath $sourcePath -Value $source -Encoding UTF8
$compiler = Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
& $compiler /nologo /target:winexe ('/out:' + (Join-Path $payload 'DoNothingAndWinLauncher.exe')) $sourcePath
if ($LASTEXITCODE -ne 0) { throw 'Probe compilation failed' }

# Compile the production installer logic with an isolated AppId and fixture payload.
$installer = Get-Content installer/launcher.iss -Raw
$installer = $installer.Replace('AppId={{C5C3A601-CFC1-4597-A7CA-D77735208B6D}', 'AppId=DNW-Update-Probe-' + [guid]::NewGuid().ToString('N'))
$installer = $installer.Replace('Source: "..\dist\app\*"', 'Source: "' + $payload + '\*"')
$installer = $installer.Replace('OutputDir=..\dist', 'OutputDir=' + $probe)
$installer = $installer.Replace('LicenseFile=..\LICENSE', 'LicenseFile=' + (Join-Path $repo 'LICENSE'))
$installer = $installer.Replace('InfoBeforeFile=welcome.txt', 'InfoBeforeFile=' + (Join-Path $repo 'installer/welcome.txt'))
$script = Join-Path $probe 'launcher.iss'
Set-Content -LiteralPath $script -Value $installer -Encoding UTF8
& $Iscc /Q /DAppVersion=9.9.9 $script
if ($LASTEXITCODE -ne 0) { throw 'Fixture installer compilation failed' }
$setup = Join-Path $probe 'DoNothingAndWin-Setup-9.9.9.exe'
$previousProbe = $env:DNW_UPDATE_PROBE
$startupKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$startupBefore = (Get-ItemProperty -LiteralPath $startupKey).EngineWorkspace
try {
    $env:DNW_UPDATE_PROBE = $probe
    Set-Content -LiteralPath (Join-Path $probe 'mode.txt') -Value 'delay' -NoNewline
    $args = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/SP-', '/NORESTART', '/NOCLOSEAPPLICATIONS', '/NOFORCECLOSEAPPLICATIONS', '/DNWUPDATE=1', ('/DIR="' + $destination + '"'), ('/LOG="' + (Join-Path $probe 'setup.log') + '"'))
    $process = Start-Process -FilePath $setup -ArgumentList $args -WindowStyle Hidden -PassThru -Wait
    if ($process.ExitCode -ne 0) { throw "Update installer failed: $($process.ExitCode)" }
    $restarted = Join-Path $probe 'restarted.txt'
    for ($i = 0; $i -lt 50 -and -not (Test-Path -LiteralPath $restarted); $i++) { Start-Sleep -Milliseconds 100 }
    if ((Get-Content -LiteralPath $restarted -Raw) -ne '--launch-from-desktop') { throw 'Missing desktop restart handoff' }
    $checks = Get-Content -LiteralPath (Join-Path $probe 'checks.txt')
    if ($checks -notcontains 'busy' -or $checks[-1] -ne 'idle') { throw 'Installer did not wait for normal shutdown' }
    $startupAfter = (Get-ItemProperty -LiteralPath $startupKey).EngineWorkspace
    if ($startupBefore -ne $startupAfter) { throw 'Updater changed the real startup setting' }

    # A launcher that stays busy must not be replaced or restarted, even silently.
    Remove-Item -LiteralPath $restarted
    Set-Content -LiteralPath (Join-Path $probe 'mode.txt') -Value 'busy' -NoNewline
    $installed = Join-Path $destination 'DoNothingAndWinLauncher.exe'
    $before = (Get-Item -LiteralPath $installed).LastWriteTimeUtc
    $process = Start-Process -FilePath $setup -ArgumentList $args -WindowStyle Hidden -PassThru -Wait
    if ($process.ExitCode -eq 0) { throw 'Installer accepted a busy launcher' }
    if ((Test-Path -LiteralPath $restarted) -or (Get-Item -LiteralPath $installed).LastWriteTimeUtc -ne $before) { throw 'Busy launcher was modified or restarted' }
    Write-Output 'PASS: waited for shutdown, installed, restarted through desktop, preserved startup setting, and refused a busy launcher.'
} finally {
    # Unregister only this randomly identified fixture using its own uninstaller.
    Set-Content -LiteralPath (Join-Path $probe 'mode.txt') -Value 'idle' -NoNewline
    $uninstaller = Join-Path $destination 'unins000.exe'
    if (Test-Path -LiteralPath $uninstaller) {
        $process = Start-Process -FilePath $uninstaller -ArgumentList '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART' -WindowStyle Hidden -PassThru -Wait
        if ($process.ExitCode -ne 0) { Write-Warning "Fixture uninstall failed: $($process.ExitCode). Fixture: $destination" }
    }
    $env:DNW_UPDATE_PROBE = $previousProbe
}
