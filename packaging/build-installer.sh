#!/usr/bin/env bash
# Build Windows x64 artifacts on Linux, including inside the development container.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
project_dir="$(cd -- "$script_dir/.." && pwd)"
target="x86_64-pc-windows-gnu"

for tool in cargo rustup x86_64-w64-mingw32-gcc x86_64-w64-mingw32-windres makensis jq zip sha256sum; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        printf 'Missing build tool: %s. Rebuild the development container or follow docs/development.md.\n' "$tool" >&2
        exit 1
    fi
done

cd -- "$project_dir"
if ! rustup target list --installed | grep -Fxq "$target"; then
    printf 'Missing Rust target. Run: rustup target add %s\n' "$target" >&2
    exit 1
fi

# Set the cross-linker only for this build; native Windows/MSVC builds keep theirs.
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="${CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER:-x86_64-w64-mingw32-gcc}"
cargo build --release -p tessera --target "$target" --locked
metadata="$(cargo metadata --no-deps --format-version 1 --locked)"
version="$(jq -er '.packages[] | select(.name == "tessera") | .version' <<< "$metadata")"
target_dir="$(jq -er '.target_directory' <<< "$metadata")"
executable="$target_dir/$target/release/tessera.exe"
artifacts_dir="$project_dir/dist/windows-gnu"
installer="Tessera-$version-x64-setup.exe"
portable="Tessera-portable.zip"

# Assemble fresh archives before publishing; repeated builds cannot retain old ZIP entries.
staging_dir="$(mktemp -d "${TMPDIR:-/tmp}/tessera-package.XXXXXX")"
trap 'rm -rf -- "$staging_dir"' EXIT
cp -- "$executable" "$staging_dir/tessera.exe"
cp -- "$project_dir/LICENSE" "$staging_dir/LICENSE.txt"
makensis -V3 -WX \
    "-DAPP_VERSION=$version" \
    "-DAPP_EXE=$staging_dir/tessera.exe" \
    "-DAPP_LICENSE=$staging_dir/LICENSE.txt" \
    "-DAPP_ICON=$project_dir/crates/tessera/assets/icons/Tessera.ico" \
    "-DAPP_OUTPUT=$staging_dir/$installer" \
    "$script_dir/windows/installer.nsi"
(
    cd -- "$staging_dir"
    zip -q -X "$portable" tessera.exe LICENSE.txt
    sha256sum "$installer" "$portable" tessera.exe LICENSE.txt > checksums.txt
)
mkdir -p -- "$artifacts_dir"
for artifact in "$installer" "$portable" tessera.exe LICENSE.txt checksums.txt; do
    cp -- "$staging_dir/$artifact" "$artifacts_dir/$artifact"
done
printf '\nWindows x64 artifacts: %s\n' "$artifacts_dir"
