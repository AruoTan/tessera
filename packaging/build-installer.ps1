$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path $PSScriptRoot -Parent
Push-Location $projectRoot
try {
    cargo build --release -p tessera --target x86_64-pc-windows-msvc --locked
    if ($LASTEXITCODE -ne 0) { throw 'Rust release build failed.' }
    $metadata = cargo metadata --no-deps --format-version 1 --locked | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read Cargo package metadata.' }
    $version = ($metadata.packages | Where-Object name -eq 'tessera').version
    if (-not $version) { throw 'Application version is missing.' }
    $compiler = Get-Command makensis.exe -ErrorAction SilentlyContinue
    $makensis = if ($compiler) { $compiler.Source } else { "${env:ProgramFiles(x86)}\NSIS\makensis.exe" }
    if (-not (Test-Path $makensis)) { throw 'Install NSIS 3 before building an installer.' }
    $artifacts = New-Item -ItemType Directory -Path dist/windows-msvc -Force
    $executable = Join-Path $metadata.target_directory 'x86_64-pc-windows-msvc/release/tessera.exe'
    $license = (Resolve-Path LICENSE).Path
    $icon = (Resolve-Path crates/tessera/assets/icons/Tessera.ico).Path
    $installer = Join-Path $artifacts.FullName "Tessera-$version-x64-setup.exe"
    & $makensis "/DAPP_VERSION=$version" "/DAPP_EXE=$executable" "/DAPP_LICENSE=$license" "/DAPP_ICON=$icon" "/DAPP_OUTPUT=$installer" "$PSScriptRoot/windows/installer.nsi"
    if ($LASTEXITCODE -ne 0) { throw 'NSIS packaging failed.' }
    # Zip a staging directory so the portable license has a conventional name.
    $staging = Join-Path $metadata.target_directory 'portable'
    New-Item -ItemType Directory -Path $staging -Force | Out-Null
    Copy-Item $executable "$staging/tessera.exe" -Force
    Copy-Item $license "$staging/LICENSE.txt" -Force
    $zip = Join-Path $artifacts.FullName 'Tessera-portable.zip'
    Compress-Archive -Path "$staging/tessera.exe", "$staging/LICENSE.txt" -DestinationPath $zip -Force
    $checksums = @($installer, $zip) | ForEach-Object {
        $hash = (Get-FileHash $_ -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $(Split-Path $_ -Leaf)"
    }
    $checksums | Set-Content "$($artifacts.FullName)/checksums.txt" -Encoding ascii
} finally {
    Pop-Location
}
