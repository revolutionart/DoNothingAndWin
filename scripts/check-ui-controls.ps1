param([string]$SourceRoot = (Join-Path (Split-Path -Parent $PSScriptRoot) 'src'))
$ErrorActionPreference = 'Stop'
$taskRoot = (Resolve-Path -LiteralPath $SourceRoot).Path
$taskLoader = Join-Path $taskRoot 'ui/loading.rs'
$taskViolations = @()
foreach ($taskFile in Get-ChildItem -LiteralPath $taskRoot -Recurse -Filter '*.rs' -File) {
    if ($taskFile.FullName -eq $taskLoader) { continue }
    $taskText = [IO.File]::ReadAllText($taskFile.FullName)
    foreach ($taskMatch in [regex]::Matches($taskText, '\bSpinner\s*::|\.\s*spinner\s*\(')) {
        $taskLine = 1 + [regex]::Matches($taskText.Substring(0, $taskMatch.Index), "`n").Count
        $taskRelative = $taskFile.FullName.Substring($taskRoot.Length).TrimStart('\', '/')
        $taskViolations += "${taskRelative}:${taskLine}: use loading::spinner(theme); size and color belong in ui/loading.rs."
    }
}
if ($taskViolations.Count) { throw ($taskViolations -join [Environment]::NewLine) }
Write-Output 'Shared UI loader check passed.'
