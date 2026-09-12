Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$outputRoot = Join-Path $repoRoot "out"
$nativeBuild = Join-Path $outputRoot "builds\package-native-win32"
$tauriTarget = Join-Path $outputRoot "builds\package-tauri"
$portableRoot = Join-Path $outputRoot "portable"
$portableDir = Join-Path $portableRoot "Navmut"
$portableStageDir = Join-Path $portableRoot "Navmut.staging"
$archivePath = Join-Path $outputRoot "Navmut-windows.zip"
$archiveStagePath = Join-Path $outputRoot "Navmut-windows.staging.zip"
$dataSource = Join-Path $repoRoot "data\points_of_interest.json"
$mapCatalogSource = Join-Path $repoRoot "data\maps.json"
$mapAssetsSource = Join-Path $repoRoot "data\maps"
$readmeSource = Join-Path $repoRoot "README.md"

function Assert-OwnedOutputPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $resolvedOutputRoot = [IO.Path]::GetFullPath($outputRoot).TrimEnd('\') + '\'
    $resolvedPath = [IO.Path]::GetFullPath($Path)
    if (-not $resolvedPath.StartsWith($resolvedOutputRoot, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to use output path outside the package output root: $resolvedPath"
    }
}

function Invoke-RequiredCommand {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,
        [Parameter(Mandatory = $true)]
        [scriptblock]$Command
    )

    & $Command
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "$Name failed with exit code $exitCode."
    }
}

function Assert-RequiredFile {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Required packaging file is missing: $Path"
    }
}

function Get-RelativeOutputPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Root,
        [Parameter(Mandatory = $true)]
        [System.IO.FileSystemInfo]$Item
    )

    return $Item.FullName.Substring($Root.Length).TrimStart([char[]]@('\', '/')).Replace('\', '/')
}

function Get-ExpectedPortableFiles {
    @(
        "navmut.exe",
        "navmut-bridge.exe",
        "navmut-helper.exe",
        "navmut-helper-hook.dll",
        "data/points_of_interest.json",
        "data/maps.json",
        "README.md",
        "bridge.md"
    )
    Get-ChildItem -LiteralPath $mapAssetsSource -File -Recurse | ForEach-Object {
            "data/maps/$(Get-RelativeOutputPath -Root $mapAssetsSource -Item $_)"
        }
}

function Assert-PortableInventory {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Root
    )

    $expectedFiles = @(Get-ExpectedPortableFiles)
    $expectedDirectories = @("data", "data/maps")
    $actualFiles = @(Get-ChildItem -LiteralPath $Root -Force -File -Recurse | ForEach-Object {
            Get-RelativeOutputPath -Root $Root -Item $_
        })
    $actualDirectories = @(Get-ChildItem -LiteralPath $Root -Force -Directory -Recurse | ForEach-Object {
            Get-RelativeOutputPath -Root $Root -Item $_
        })
    $missingFiles = @($expectedFiles | Where-Object { $actualFiles -notcontains $_ })
    $unexpectedFiles = @($actualFiles | Where-Object { $expectedFiles -notcontains $_ })
    $missingDirectories = @($expectedDirectories | Where-Object { $actualDirectories -notcontains $_ })
    $unexpectedDirectories = @($actualDirectories | Where-Object { $expectedDirectories -notcontains $_ })

    if (($missingFiles.Count -gt 0) -or ($unexpectedFiles.Count -gt 0) -or
        ($missingDirectories.Count -gt 0) -or ($unexpectedDirectories.Count -gt 0)) {
        $details = @()
        if ($missingFiles.Count -gt 0) {
            $details += "missing files: $($missingFiles -join ', ')"
        }
        if ($unexpectedFiles.Count -gt 0) {
            $details += "unexpected files: $($unexpectedFiles -join ', ')"
        }
        if ($missingDirectories.Count -gt 0) {
            $details += "missing directories: $($missingDirectories -join ', ')"
        }
        if ($unexpectedDirectories.Count -gt 0) {
            $details += "unexpected directories: $($unexpectedDirectories -join ', ')"
        }
        throw "Portable artifact inventory mismatch ($($details -join '; '))."
    }
}

function Assert-NoPortableUserData {
    if (-not (Test-Path -LiteralPath $portableDir -PathType Container)) {
        return
    }
    $expectedFiles = @(Get-ExpectedPortableFiles)
    $userFiles = @(Get-ChildItem -LiteralPath $portableDir -Force -File -Recurse | Where-Object {
            (Get-RelativeOutputPath -Root $portableDir -Item $_) -notin $expectedFiles
        } | ForEach-Object { $_.FullName })
    if ($userFiles.Count -gt 0) {
        throw "Portable output contains user data. Move the application out of '$portableDir' before rebuilding: $($userFiles -join ', ')"
    }
}

foreach ($outputPath in @($nativeBuild, $tauriTarget, $portableRoot, $portableDir, $portableStageDir, $archivePath, $archiveStagePath)) {
    Assert-OwnedOutputPath -Path $outputPath
}

$cargoTargetWasSet = Test-Path -LiteralPath "Env:CARGO_TARGET_DIR"
$previousCargoTarget = $null
if ($cargoTargetWasSet) {
    $previousCargoTarget = $env:CARGO_TARGET_DIR
}
$locationPushed = $false

try {
    New-Item -ItemType Directory -Force -Path $outputRoot | Out-Null
    New-Item -ItemType Directory -Force -Path $portableRoot | Out-Null
    Assert-NoPortableUserData

    foreach ($freshPath in @($nativeBuild, $tauriTarget, $portableStageDir, $archiveStagePath)) {
        if (Test-Path -LiteralPath $freshPath) {
            Remove-Item -LiteralPath $freshPath -Recurse -Force
        }
    }

    Push-Location $repoRoot
    $locationPushed = $true
    try {
        Invoke-RequiredCommand -Name "CMake configure" -Command {
            & cmake -S (Join-Path $repoRoot "native\windows-x86") -B $nativeBuild -A Win32
        }
        Invoke-RequiredCommand -Name "CMake build" -Command {
            & cmake --build $nativeBuild --config Release
        }
        Invoke-RequiredCommand -Name "CTest" -Command {
            & ctest --test-dir $nativeBuild -C Release --output-on-failure
        }

        $nativeHelper = Join-Path $nativeBuild "Release\navmut-helper.exe"
        $nativeHook = Join-Path $nativeBuild "Release\navmut-helper-hook.dll"
        Assert-RequiredFile -Path $nativeHelper
        Assert-RequiredFile -Path $nativeHook

        $env:CARGO_TARGET_DIR = $tauriTarget
        Invoke-RequiredCommand -Name "Windows x86 bridge build" -Command {
            & cargo build --locked --release -p navmut-platform --bin navmut-bridge --target i686-pc-windows-msvc
        }
        $bridgeExecutable = Join-Path $tauriTarget "i686-pc-windows-msvc\release\navmut-bridge.exe"
        Assert-RequiredFile -Path $bridgeExecutable
        Invoke-RequiredCommand -Name "Tauri build" -Command {
            & npm run tauri -- build --no-bundle
        }

        $tauriExecutable = Join-Path $tauriTarget "release\navmut.exe"
        Assert-RequiredFile -Path $tauriExecutable
        Assert-RequiredFile -Path $dataSource
        Assert-RequiredFile -Path $mapCatalogSource
        Assert-RequiredFile -Path $readmeSource
        if (-not (Test-Path -LiteralPath $mapAssetsSource -PathType Container)) {
            throw "Required packaged map directory is missing: $mapAssetsSource"
        }

        New-Item -ItemType Directory -Force -Path (Join-Path $portableStageDir "data") | Out-Null
        Copy-Item -LiteralPath $tauriExecutable -Destination (Join-Path $portableStageDir "navmut.exe")
        Copy-Item -LiteralPath $bridgeExecutable -Destination (Join-Path $portableStageDir "navmut-bridge.exe")
        Copy-Item -LiteralPath $readmeSource -Destination (Join-Path $portableStageDir "README.md")
        Copy-Item -LiteralPath (Join-Path $repoRoot "docs\bridge.md") -Destination (Join-Path $portableStageDir "bridge.md")
        Copy-Item -LiteralPath $nativeHelper -Destination (Join-Path $portableStageDir "navmut-helper.exe")
        Copy-Item -LiteralPath $nativeHook -Destination (Join-Path $portableStageDir "navmut-helper-hook.dll")
        Copy-Item -LiteralPath $dataSource -Destination (Join-Path $portableStageDir "data\points_of_interest.json")
        Copy-Item -LiteralPath $mapCatalogSource -Destination (Join-Path $portableStageDir "data\maps.json")
        Copy-Item -LiteralPath $mapAssetsSource -Destination (Join-Path $portableStageDir "data") -Recurse
        Assert-PortableInventory -Root $portableStageDir

        if (Test-Path -LiteralPath $portableDir) {
            Remove-Item -LiteralPath $portableDir -Recurse -Force
        }
        Move-Item -LiteralPath $portableStageDir -Destination $portableDir
        [IO.Compression.ZipFile]::CreateFromDirectory($portableDir, $archiveStagePath, [IO.Compression.CompressionLevel]::Optimal, $true)
        Move-Item -LiteralPath $archiveStagePath -Destination $archivePath -Force
    }
    finally {
        Pop-Location
        $locationPushed = $false
    }
}
catch {
    if (Test-Path -LiteralPath $portableStageDir) {
        Remove-Item -LiteralPath $portableStageDir -Recurse -Force -ErrorAction SilentlyContinue
    }
    if (Test-Path -LiteralPath $archiveStagePath) {
        Remove-Item -LiteralPath $archiveStagePath -Force -ErrorAction SilentlyContinue
    }
    Write-Error $_
    exit 1
}
finally {
    if ($locationPushed) {
        Pop-Location
    }
    if ($cargoTargetWasSet) {
        $env:CARGO_TARGET_DIR = $previousCargoTarget
    }
    else {
        Remove-Item -LiteralPath "Env:CARGO_TARGET_DIR" -ErrorAction SilentlyContinue
    }
}

Write-Host "Windows ZIP: out\Navmut-windows.zip"
Write-Host "Application folder: out\portable\Navmut"
