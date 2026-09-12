#!/usr/bin/env bash
# rust-objcopy (used by `dx bundle` to strip) loads libLLVM.dylib via
# @rpath → @loader_path/../lib, i.e. rustlib/<host>/lib. rustup leaves the
# dylib in the toolchain's top-level lib/ instead, so strip aborts with
# "Library not loaded: @rpath/libLLVM.dylib".
set -euo pipefail

sysroot="$(rustc --print sysroot)"
host="$(rustc -vV | awk '/^host:/{print $2}')"
dest="${sysroot}/lib/rustlib/${host}/lib"
mkdir -p "$dest"
if ! find "${sysroot}/lib" -maxdepth 1 -name 'libLLVM*.dylib' | grep -q .; then
  echo "libLLVM.dylib not found under ${sysroot}/lib"
  find "$sysroot" -name 'libLLVM*' | head -50
  exit 1
fi
find "${sysroot}/lib" -maxdepth 1 -name 'libLLVM*.dylib' -exec ln -sf {} "$dest/" \;
echo "Linked LLVM dylibs into ${dest}:"
ls -l "$dest"/libLLVM*
objcopy="${sysroot}/lib/rustlib/${host}/bin/rust-objcopy"
if [[ ! -x "$objcopy" ]]; then
  echo "rust-objcopy not found at ${objcopy}"
  ls -l "${sysroot}/lib/rustlib/${host}/bin" || true
  exit 1
fi
if [[ -n "${GITHUB_ENV:-}" ]]; then
  echo "PATH=${sysroot}/lib/rustlib/${host}/bin:${PATH}" >> "$GITHUB_ENV"
  echo "DYLD_LIBRARY_PATH=${sysroot}/lib:${dest}${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}" >> "$GITHUB_ENV"
  echo "DYLD_FALLBACK_LIBRARY_PATH=${sysroot}/lib:${dest}" >> "$GITHUB_ENV"
fi
"$objcopy" --version
