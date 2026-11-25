param(
    [ValidateSet("", "cpu", "cuda", "rocm", "hip", "vulkan")][string]$Backend = "cpu",
    [string]$CudaArch = "",
    [string]$RocmArch = "",
    [ValidateSet("debug", "dev", "release")][string]$BuildProfile = "debug",
    [ValidateSet("x86_64-pc-windows-msvc")][string]$Target = "x86_64-pc-windows-msvc",
    [switch]$RuntimeOnly
)
$ErrorActionPreference = "Stop"
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))

function Resolve-CommandPath {
    param([string]$Name)

    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }
    return $null
}

function Add-ToPath {
    param([string]$Directory)

    if (-not $Directory -or -not (Test-Path $Directory)) {
        return
    }

    $pathEntries = @($env:PATH -split [System.IO.Path]::PathSeparator)
    if ($pathEntries -contains $Directory) {
        return
    }

    $env:PATH = "$Directory$([System.IO.Path]::PathSeparator)$env:PATH"
}

function Import-CmdEnvironment {
    param(
        [Parameter(Mandatory = $true)]
        [string]$CommandLine
    )

    $output = & cmd.exe /s /c "$CommandLine && set"
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to initialize Windows build environment with command: $CommandLine"
    }

    foreach ($line in $output) {
        if ($line -match '^(?<name>[^=]+)=(?<value>.*)$') {
            Set-Item -Path "env:$($Matches.name)" -Value $Matches.value
        }
    }
}

function Get-MissingCommands {
    param([string[]]$Commands)

    $missing = @()
    foreach ($command in $Commands) {
        if (-not (Resolve-CommandPath $command)) {
            $missing += $command
        }
    }
    return $missing
}

function Ensure-MsvcToolchain {
    $requiredTools = @("cl", "link", "lib", "rc", "mt")
    $missingTools = @(Get-MissingCommands -Commands $requiredTools)
    if ($missingTools.Count -eq 0) {
        return
    }

    Write-Host "MSVC/Windows SDK tools missing before vcvars64 import: $($missingTools -join ', ')"

    $programFilesX86 = ${env:ProgramFiles(x86)}
    $vswhereCandidates = @()
    if ($programFilesX86) {
        $vswhereCandidates += (Join-Path $programFilesX86 "Microsoft Visual Studio\Installer\vswhere.exe")
    }
    if ($env:ProgramFiles) {
        $vswhereCandidates += (Join-Path $env:ProgramFiles "Microsoft Visual Studio\Installer\vswhere.exe")
    }
    $vswhereFromPath = Resolve-CommandPath "vswhere"
    if ($vswhereFromPath) {
        $vswhereCandidates += $vswhereFromPath
    }

    $vcvars64 = $null
    $vswhere = $vswhereCandidates | Where-Object { $_ -and (Test-Path $_) } | Select-Object -Unique -First 1
    if ($vswhere) {
        $installationPathOutput = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath | Select-Object -First 1
        $installationPath = if ($installationPathOutput) { $installationPathOutput.Trim() } else { "" }
        if ($installationPath) {
            $candidate = Join-Path $installationPath "VC\Auxiliary\Build\vcvars64.bat"
            if (Test-Path $candidate) {
                $vcvars64 = $candidate
            }
        }
    }

    if (-not $vcvars64) {
        $searchRoots = @($programFilesX86, $env:ProgramFiles) | Where-Object { $_ } | Select-Object -Unique
        foreach ($searchRoot in $searchRoots) {
            $candidate = Get-ChildItem -Path $searchRoot -Filter vcvars64.bat -Recurse -ErrorAction SilentlyContinue |
                Where-Object { $_.FullName -like '*Microsoft Visual Studio*VC\Auxiliary\Build\vcvars64.bat' } |
                Select-Object -First 1
            if ($candidate) {
                $vcvars64 = $candidate.FullName
                break
            }
        }
    }

    if (-not (Test-Path $vcvars64)) {
        throw "Visual Studio Build Tools with vcvars64.bat were not found on this Windows runner."
    }

    Import-CmdEnvironment "`"$vcvars64`" >nul"

    $missingTools = @(Get-MissingCommands -Commands $requiredTools)
    if ($missingTools.Count -gt 0) {
        throw "MSVC toolchain initialization completed, but required Visual Studio/Windows SDK tools are still unavailable in PATH: $($missingTools -join ', '). Install the Windows SDK and Visual Studio Build Tools on this runner."
    }
}

function Ensure-CudaToolchain {
    if (Resolve-CommandPath "nvcc") {
        return
    }

    $candidates = @()
    if ($env:CUDA_PATH) {
        $candidates += (Join-Path $env:CUDA_PATH "bin")
    }
    if ($env:ProgramFiles) {
        $toolkitRoot = Join-Path $env:ProgramFiles "NVIDIA GPU Computing Toolkit\CUDA"
        if (Test-Path $toolkitRoot) {
            $candidates += Get-ChildItem -Path $toolkitRoot -Directory | Sort-Object Name -Descending | ForEach-Object {
                Join-Path $_.FullName "bin"
            }
        }
    }

    foreach ($candidate in $candidates) {
        if (Test-Path (Join-Path $candidate "nvcc.exe")) {
            Add-ToPath $candidate
            return
        }
    }

    throw "nvcc not found. Install the CUDA toolkit and ensure nvcc.exe is in PATH."
}

function Resolve-PackagingBash {
    $kernel = try { (& bash -c "uname -s" 2>$null | Out-String).Trim() } catch { "" }
    if ($kernel -match '^(MINGW|MSYS|CYGWIN)') {
        return "bash"
    }
    $git = Resolve-CommandPath "git"
    if ($git) {
        # git.exe sits in <Git>\cmd or <Git>\bin, and Git Bash in <Git>\bin.
        $gitBash = Join-Path (Split-Path -Parent (Split-Path -Parent $git)) "bin\bash.exe"
        if (Test-Path $gitBash) {
            return $gitBash
        }
    }
    throw "Packaging the native runtime needs Git Bash, MSYS2 or Cygwin bash, but bash on PATH is not one (uname -s: '$kernel') and no Git Bash was found next to git. Put Git for Windows' bin directory first on PATH."
}

function Invoke-NativeCommand {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Command,
        [string[]]$Arguments = @()
    )

    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
        $argString = if ($Arguments.Count -gt 0) { " " + ($Arguments -join " ") } else { "" }
        throw "Command failed with exit code ${LASTEXITCODE}: $Command$argString"
    }
}
Push-Location $repoRoot
try {
    Ensure-MsvcToolchain
    if (-not $Backend) { $Backend = "cpu" }
    if ($Backend -eq "cuda") { Ensure-CudaToolchain }
    $bash = Resolve-PackagingBash
    if (-not $env:CC) { $env:CC = "cl" }
    if (-not $env:CXX) { $env:CXX = "cl" }
    $env:CMAKE_GENERATOR = "Ninja"
    if ($CudaArch) { $env:LLAMA_STAGE_CUDA_ARCHITECTURES = $CudaArch }
    if ($RocmArch) { $env:LLAMA_STAGE_AMDGPU_TARGETS = $RocmArch }
    $profile = if ($BuildProfile -eq "release") { "release" } else { "debug" }
    $runtimeOut = if ($RuntimeOnly) { "dist/native-runtimes" } else { "target/$profile/native-runtimes" }
    Invoke-NativeCommand $bash @("scripts/package-native-runtime.sh", "--build", "--backend", $Backend, "--target", $Target, "--out", $runtimeOut)
    if (-not $RuntimeOnly) {
        $recipe = if ($BuildProfile -eq "release") { "skippy-cli-release-build" } else { "skippy-cli-build" }
        Invoke-NativeCommand "just" @($recipe)
        Write-Host "Built Skippy: target/$profile/skippy.exe"
    }
} finally {
    Pop-Location
}
