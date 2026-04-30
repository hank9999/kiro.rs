[CmdletBinding()]
param(
    [switch]$SkipCargo,
    [switch]$SkipFrontend,
    [switch]$SkipDocker,
    [string]$DockerImageTag = "kiro-rs-local-verify"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Write-Step {
    param([string]$Message)

    Write-Host ""
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Find-VcVars64 {
    $candidates = @(
        "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat",
        "C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat",
        "C:\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
    )

    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate) {
            return $candidate
        }
    }

    throw "vcvars64.bat not found. Install Visual Studio Build Tools with the C++ x64 toolchain."
}

function Ensure-DockerPath {
    $dockerBin = "C:\Program Files\Docker\Docker\resources\bin"
    if (Test-Path -LiteralPath $dockerBin) {
        $pathParts = @($env:PATH -split ";")
        if ($pathParts -notcontains $dockerBin) {
            $env:PATH = "$dockerBin;$($env:PATH)"
        }
    }

    if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
        throw "docker.exe was not found. Start Docker Desktop or add Docker's bin directory to PATH."
    }
}

function Invoke-CmdWithVcVars {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Command
    )

    $vcvars64 = Find-VcVars64
    $cmdLine = "call ""$vcvars64"" && $Command"

    & cmd.exe /d /s /c $cmdLine
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed: $Command"
    }
}

$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot

try {
    if (-not $SkipCargo) {
        Write-Step "Rust checks (vcvars64 + cargo check --tests)"
        Invoke-CmdWithVcVars "cargo check --tests"
    }

    if (-not $SkipFrontend) {
        Write-Step "Admin UI build"
        Push-Location (Join-Path $repoRoot "admin-ui")
        try {
            if (-not (Test-Path -LiteralPath "node_modules")) {
                npm install
                if ($LASTEXITCODE -ne 0) {
                    throw "npm install failed"
                }
            }

            npm run build
            if ($LASTEXITCODE -ne 0) {
                throw "npm run build failed"
            }
        }
        finally {
            Pop-Location
        }
    }

    if (-not $SkipDocker) {
        Write-Step "Docker validation"
        Ensure-DockerPath

        docker version
        if ($LASTEXITCODE -ne 0) {
            throw "docker version failed"
        }

        docker build -t $DockerImageTag .
        if ($LASTEXITCODE -ne 0) {
            throw "docker build failed"
        }
    }

    Write-Step "All checks completed"
}
finally {
    Pop-Location
}
