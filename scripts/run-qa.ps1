[CmdletBinding()]
param(
    [string]$DiretorioTarget = "",
    [switch]$PularRelease
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if ($DiretorioTarget) {
    $env:CARGO_TARGET_DIR = [System.IO.Path]::GetFullPath($DiretorioTarget)
}

function Invoke-CargoGate {
    <#
    .SYNOPSIS
    Executa um gate Cargo e interrompe o roteiro em qualquer falha.
    #>
    param(
        [Parameter(Mandatory)]
        [string[]]$Argumentos
    )

    Write-Host "cargo $($Argumentos -join ' ')"
    & cargo @Argumentos
    if ($LASTEXITCODE -ne 0) {
        throw "Gate falhou: cargo $($Argumentos -join ' ')"
    }
}

Invoke-CargoGate @("fmt", "--all", "--check")
Invoke-CargoGate @("check", "--all-targets", "--locked")
Invoke-CargoGate @("clippy", "--all-targets", "--locked", "--", "-D", "warnings")
Invoke-CargoGate @("test", "--all-targets", "--locked")
Invoke-CargoGate @("doc", "--no-deps", "--locked")

if (-not $PularRelease) {
    Invoke-CargoGate @("build", "--release", "--locked")
}

Write-Host "QA concluído sem falhas."
