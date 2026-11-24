#!/usr/bin/env bash
# Pinned Linux -> Windows x64 GNU/LLVM toolchain. No global installation.
# --prepare downloads/prepares only; --smoke builds isolated toolchain tests;
# --env prints shell exports; default builds the application (only when requested).
set -euo pipefail
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
CACHE="$ROOT/.cache/windows-toolchain"
TARGET=x86_64-pc-windows-gnullvm
LLVM_NAME=llvm-mingw-20251216-ucrt-ubuntu-22.04-x86_64
STD_NAME=rust-std-1.95.0-x86_64-pc-windows-gnullvm
RUSTC_NAME=rustc-1.95.0-x86_64-unknown-linux-gnu
HOST_STD_NAME=rust-std-1.95.0-x86_64-unknown-linux-gnu
HOST_SYSROOT="$CACHE/native-rust-1.95.0"
LLVM="$CACHE/$LLVM_NAME"
SYSROOT="$CACHE/$STD_NAME/rust-std-$TARGET"
mkdir -p "$CACHE/downloads" "$CACHE/tmp"
export TMPDIR="$CACHE/tmp"

fetch() {
    local url=$1 name=$2 sha=$3 file="$CACHE/downloads/$2"
    if [[ ! -f "$file" ]]; then
        curl --fail --location --retry 3 "$url" -o "$file.part"
        mv -- "$file.part" "$file"
    fi
    printf '%s  %s\n' "$sha" "$file" | sha256sum --check --status || {
        echo "Checksum mismatch: $file (remove it to retry)" >&2; exit 1;
    }
    if [[ ! -d "$CACHE/${name%.tar.xz}" ]]; then
        tar -xf "$file" -C "$CACHE"
    fi
}

prepare() {
    # Official release manifest commit is exactly the host compiler commit.
    [[ $(rustc -vV) == *'commit-hash: 59807616e1fa2540724bfbac14d7976d7e4a3860'* ]] || {
        echo 'Requires rustc 1.95.0, commit 59807616e1fa2540724bfbac14d7976d7e4a3860' >&2; exit 1;
    }
    # Hashes: official static.rust-lang.org 1.95.0 manifest and GitHub asset digest.
    fetch "https://static.rust-lang.org/dist/2026-04-16/$STD_NAME.tar.xz" \
        "$STD_NAME.tar.xz" 3c52a0e34e0b4abe4439cabd77383408f5f9c80b6e249fbc0855df424b45008d
    fetch "https://github.com/mstorsjo/llvm-mingw/releases/download/20251216/$LLVM_NAME.tar.xz" \
        "$LLVM_NAME.tar.xz" db561734a05eb4a0695d5e901aadd6c1cda74bba4c9e102b677f81c6fd59849a
    # NixOS rustc has different crate metadata despite an identical commit:
    # official std rejects its '(built from a source tarball)' compiler version.
    # Use an official *native Linux* compiler + host std for build scripts and
    # proc macros, and the separate Windows sysroot only for target crates.
    fetch "https://static.rust-lang.org/dist/2026-04-16/$RUSTC_NAME.tar.xz" \
        "$RUSTC_NAME.tar.xz" 8426a3d170a5879f5682f5fbdd024a1779b3951e7baba685af2d6dc32a6dfc15
    fetch "https://static.rust-lang.org/dist/2026-04-16/$HOST_STD_NAME.tar.xz" \
        "$HOST_STD_NAME.tar.xz" 047ea7098803d3500fa1072e9cee5392697e21525559e4458128a2bf874aa382
    if [[ ! -d "$HOST_SYSROOT" ]]; then
        mkdir -p "$HOST_SYSROOT"
        cp -a "$CACHE/$RUSTC_NAME/rustc/." "$HOST_SYSROOT/"
        cp -a "$CACHE/$HOST_STD_NAME/rust-std-x86_64-unknown-linux-gnu/lib/." "$HOST_SYSROOT/lib/"
    fi
    # Upstream Ubuntu executables need a real loader on NixOS. Reuse only already
    # installed native tools/libraries, copying the runtime into this cache.
    # No nix build, profile changes, rustup, or additional downloads.
    if [[ -e /etc/NIXOS && ( $(cat "$LLVM/.nixos-local-runtime" 2>/dev/null || true) != "$CACHE/linux-runtime" || $(cat "$HOST_SYSROOT/.nixos-local-runtime" 2>/dev/null || true) != "$CACHE/linux-runtime" ) ]]; then
        python3 - "$LLVM" "$CACHE/linux-runtime" "$HOST_SYSROOT" <<'PY'
import glob, os, pathlib, re, shutil, subprocess, sys
llvm, runtime, host = map(pathlib.Path, sys.argv[1:])
runtime.mkdir(parents=True, exist_ok=True)
patchers = glob.glob('/nix/store/*patchelf*/bin/patchelf')
if not patchers:
    sys.exit('NixOS needs an already installed patchelf; none was found')
patcher = patchers[0]
# ldd reports the runtime closure of existing native executables, not downloads.
programs = [patcher]
for name in ('nix', 'zstd', 'cargo'):
    p = shutil.which(name)
    if p:
        programs.append(os.path.realpath(p))
clang = shutil.which('clang')
if clang:
    original = pathlib.Path(os.path.realpath(clang)).parent.parent / 'nix-support/orig-cc'
    if original.exists():
        programs.append(str(pathlib.Path(original.read_text().strip()) / 'bin/clang'))
needed = {'libc.so.6', 'libm.so.6', 'libdl.so.2', 'librt.so.1',
          'libstdc++.so.6', 'libgcc_s.so.1', 'libz.so.1', 'libzstd.so.1',
          'ld-linux-x86-64.so.2'}
found = {}
for program in programs:
    output = subprocess.run(['ldd', program], capture_output=True, text=True).stdout
    for path in re.findall(r'/nix/store/[^\s()]+', output):
        name = os.path.basename(path)
        if name in needed and name not in found and os.path.isfile(path):
            found[name] = path
# glibc compatibility DSOs are not all linked into modern host binaries.
if 'libc.so.6' in found:
    for name in ('libpthread.so.0', 'libutil.so.1'):
        path = str(pathlib.Path(found['libc.so.6']).parent / name)
        if os.path.isfile(path):
            found[name] = path
if needed - found.keys():
    sys.exit('Missing existing native runtime libraries: ' + ', '.join(needed - found.keys()))
for name, source in found.items():
    destination = runtime / name
    destination.unlink(missing_ok=True)
    shutil.copy2(source, destination)
# Patch extracted copies only; official archive checksums remain verifiable.
for directory in (llvm / 'bin', llvm / 'lib', host / 'bin', host / 'lib',
                  host / 'lib/rustlib/x86_64-unknown-linux-gnu/bin'):
    for p in directory.rglob('*'):
        if p.is_symlink() or not p.is_file():
            continue
        with p.open('rb') as f:
            header = f.read(18)
            if header[:4] != b'\x7fELF' or int.from_bytes(header[16:18], 'little') not in (2, 3):
                continue
        interp = subprocess.run([patcher, '--print-interpreter', str(p)],
                                capture_output=True, text=True)
        if interp.returncode == 0:
            subprocess.run([patcher, '--set-interpreter',
                            str(runtime / 'ld-linux-x86-64.so.2'), str(p)], check=True)
        old_rpath = subprocess.check_output([patcher, '--print-rpath', str(p)], text=True).strip()
        search = ':'.join(filter(None, [old_rpath, str(llvm / 'lib'), str(host / 'lib'), str(runtime)]))
        subprocess.run([patcher, '--set-rpath', search, str(p)], check=True)
for root in (llvm, host):
    (root / '.nixos-local-runtime').write_text(str(runtime) + '\n')
PY
    fi
    "$LLVM/bin/x86_64-w64-mingw32-clang" --version >/dev/null
    "$HOST_SYSROOT/bin/rustc" --version >/dev/null
}

environment() {
    # Cargo splits target-specific RUSTFLAGS on whitespace (not shell quotes).
    # Keep the working target-only flags approach, but reject ambiguous paths.
    if [[ "$SYSROOT" == *[[:space:]]* ]]; then
        echo 'Windows cross-build requires a project path without whitespace.' >&2
        return 1
    fi
    # Global flags override Cargo's target-specific flags, so remove them.
    unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_BUILD_RUSTFLAGS
    export CARGO_HOME="${CARGO_HOME:-$CACHE/cargo-home}"
    export PATH="$LLVM/bin:$PATH"
    export RUSTC="$HOST_SYSROOT/bin/rustc"
    export RUSTDOC="$HOST_SYSROOT/bin/rustdoc"
    export CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER="$LLVM/bin/x86_64-w64-mingw32-clang"
    export CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_RUSTFLAGS="--sysroot=$SYSROOT -C target-feature=+crt-static -C link-arg=-static"
    export CC_x86_64_pc_windows_gnullvm="$LLVM/bin/x86_64-w64-mingw32-clang"
    export CXX_x86_64_pc_windows_gnullvm="$LLVM/bin/x86_64-w64-mingw32-clang++"
    export AR_x86_64_pc_windows_gnullvm="$LLVM/bin/llvm-ar"
    export RANLIB_x86_64_pc_windows_gnullvm="$LLVM/bin/llvm-ranlib"
    # ring's cc-rs build uses the target-specific compiler, never native clang.
    # No global RUSTFLAGS/CC: native build scripts/proc macros retain host std.
    export CARGO_TARGET_DIR="$CACHE/target"
}

verify_windows_exe() {
    local executable=$1 report=$2 dll count=0
    "$LLVM/bin/llvm-readobj" --file-headers --coff-imports "$executable" > "$report"
    if ! grep -q '^Format: COFF-x86-64$' "$report" ||
       ! grep -q 'Machine: IMAGE_FILE_MACHINE_AMD64' "$report"; then
        echo "Not an AMD64 Windows executable: $executable" >&2
        return 1
    fi
    # Deliberate allowlist: unknown DLLs fail closed, including VC++/MinGW
    # redistributables. API sets and UCRT are built into supported Windows 10+.
    while IFS= read -r dll; do
        count=$((count + 1))
        case "${dll,,}" in
            kernel32.dll|ntdll.dll|user32.dll|gdi32.dll|ws2_32.dll|advapi32.dll|\
            bcrypt.dll|bcryptprimitives.dll|shell32.dll|userenv.dll|ucrtbase.dll|\
            api-ms-win-*.dll|ext-ms-win-*.dll) ;;
            *)
                echo "Unexpected non-system or unapproved DLL import: $dll" >&2
                echo "Import report: $report" >&2
                return 1
                ;;
        esac
    done < <(awk '/^  Name: / { print $2 }' "$report")
    if (( count == 0 )); then
        echo "No DLL imports found; cannot verify executable: $executable" >&2
        return 1
    fi
}

smoke() {
    mkdir -p "$CACHE/smoke"
    printf '#include <windows.h>\nint main(void) { return GetCurrentProcessId() == 0; }\n' > "$CACHE/smoke/hello.c"
    "$CC_x86_64_pc_windows_gnullvm" -static "$CACHE/smoke/hello.c" -o "$CACHE/smoke/hello-c.exe"
    printf 'fn main() { println!("Windows std smoke test"); }\n' > "$CACHE/smoke/hello.rs"
    "$RUSTC" --target "$TARGET" --sysroot "$SYSROOT" \
        -C "linker=$CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER" \
        -C target-feature=+crt-static -C link-arg=-static \
        "$CACHE/smoke/hello.rs" -o "$CACHE/smoke/hello-rust.exe"
    verify_windows_exe "$CACHE/smoke/hello-rust.exe" "$CACHE/smoke/hello-rust-imports.txt"
    # Also verify native host std/linking, as required by Cargo build scripts.
    "$RUSTC" "$CACHE/smoke/hello.rs" -o "$CACHE/smoke/hello-native"
    "$CACHE/smoke/hello-native"
}

case "${1:-}" in
    --prepare) prepare ;;
    --env)
        environment
        printf '%s\n' 'unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_BUILD_RUSTFLAGS'
        for name in PATH TMPDIR CARGO_HOME RUSTC RUSTDOC CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER \
            CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_RUSTFLAGS CC_x86_64_pc_windows_gnullvm \
            CXX_x86_64_pc_windows_gnullvm AR_x86_64_pc_windows_gnullvm \
            RANLIB_x86_64_pc_windows_gnullvm CARGO_TARGET_DIR; do
            printf 'export %s=%q\n' "$name" "${!name}"
        done ;;
    --smoke) prepare; environment; smoke ;;
    --help|-h) printf '%s\n' 'Usage: scripts/build-windows.sh [--prepare|--env|--smoke|cargo-build-options...]' ;;
    *)
        prepare
        environment
        cd "$ROOT"
        cargo build --locked --release --target "$TARGET" "$@"
        executable="$CARGO_TARGET_DIR/$TARGET/release/screen-answer.exe"
        verify_windows_exe "$executable" "$CACHE/app-exe-imports.txt"
        mkdir -p "$ROOT/bin"
        artifact=$(mktemp "$ROOT/bin/.screen-answer.exe.XXXXXX")
        trap 'rm -f -- "$artifact"' EXIT
        cp -- "$executable" "$artifact"
        chmod 0755 "$artifact"
        mv -fT -- "$artifact" "$ROOT/bin/screen-answer.exe"
        trap - EXIT
        sha256sum "$ROOT/bin/screen-answer.exe"
        ;;
esac
