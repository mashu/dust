#!/usr/bin/env bash
# Gradle init hook for the Android project dx generates.
#
# dx 0.7 leaves four things out for this app, and has a setting for none:
#
# - It always writes Android's stock green robot as the launcher icon (an
#   adaptive icon under mipmap-anydpi-v26, which is what Android 8+ shows),
#   and never reads bundle.icon.
# - It always writes versionCode = 1. Play turns away an upload whose
#   version code is not above every one uploaded before it, so every build
#   after the first would be refused.
# - It asks for no native debug symbols, so Play has nothing to name the
#   functions in a crash with. Asking the plugin for SYMBOL_TABLE is not
#   enough: rustc already removed the debug info, the plugin's strip then
#   changes nothing, and it packs no symbol file. The Play script writes
#   that file itself, and refuses to finish without it.
# - Its manifest has the activity handle only a turn of the screen
#   (orientation|screenLayout|screenSize|keyboardHidden) itself. Any other
#   configuration change makes Android recreate the activity, and the
#   webview with it; Dioxus then has no window left, and tao ends the
#   process. Dark mode turned on, a keyboard or a paddle adapter plugged
#   in, the font or display size changed, the language, a foldable opened:
#   each closed the app, mid-session or not. The hook has the activity
#   handle every change itself (ANDROID_CONFIG_CHANGES).
#
# Source this and call install_android_gradle_hook before
# `dx bundle --platform android`. The caller removes $ANDROID_GRADLE_HOOK
# when the build exits; Gradle loads every script in ~/.gradle/init.d.

# The workspace version in the Cargo.toml under `root`, e.g. 0.14.4.
dust_version() {
  awk '
    /^\[workspace.package\]/ { in_pkg = 1; next }
    in_pkg && /^\[/ { in_pkg = 0 }
    in_pkg && /^version *=/ { gsub(/[" ]/, "", $3); print $3; exit }
  ' "$1/Cargo.toml"
}

# The Android version code for a MAJOR.MINOR.PATCH version:
# MAJOR * 1000000 + MINOR * 1000 + PATCH, so 0.14.4 is 14004. A later version
# is always a higher code, as Play requires, while MINOR and PATCH stay under
# 1000.
dust_version_code() {
  local version=$1
  if [[ ! "$version" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
    echo "Version '$version' is not MAJOR.MINOR.PATCH" >&2
    return 1
  fi
  local major=$((10#${BASH_REMATCH[1]}))
  local minor=$((10#${BASH_REMATCH[2]}))
  local patch=$((10#${BASH_REMATCH[3]}))
  if ((minor > 999 || patch > 999)); then
    echo "Version '$version' has no version code: MINOR and PATCH must stay under 1000" >&2
    return 1
  fi
  echo $((major * 1000000 + minor * 1000 + patch))
}

# Every configuration change the activity handles itself, rather than be
# recreated for it: every flag `android:configChanges` takes under the
# compile SDK in Dioxus.toml. The webview lays itself out again for a new
# size or density, and the app's page has nothing to reload for the rest.
ANDROID_CONFIG_CHANGES="mcc|mnc|locale|touchscreen|keyboard|keyboardHidden|navigation|screenLayout|fontScale|uiMode|orientation|density|screenSize|smallestScreenSize|layoutDirection|colorMode|fontWeightAdjustment|grammaticalGender"

install_android_gradle_hook() {
  local root=$1
  local gradle_init="${GRADLE_USER_HOME:-$HOME/.gradle}/init.d"
  if [[ ! -d "$root/assets/android" ]]; then
    echo "Launcher icon resources missing at $root/assets/android" >&2
    echo "Run scripts/render-android-icons.py after updating assets/icon.png" >&2
    return 1
  fi
  local version code
  version="$(dust_version "$root")"
  code="$(dust_version_code "$version")" || return 1
  # The NDK the Rust code was built with. A path whose revision disagrees
  # with ndkVersion is ignored, and the plugin resolves the NDK when the
  # plugin is applied, so both are set then.
  local ndk_path='// No ANDROID_NDK_HOME: the Android plugin looks for its own NDK.'
  local ndk_version='// No NDK revision: the Android plugin keeps its pinned version.'
  if [[ -n "${ANDROID_NDK_HOME:-}" && -d "$ANDROID_NDK_HOME" ]]; then
    ndk_path="android.ndkPath = \"${ANDROID_NDK_HOME}\""
    local revision
    revision="$(awk -F' = ' '/^Pkg\.Revision/ { print $2; exit }' "$ANDROID_NDK_HOME/source.properties")"
    if [[ -z "$revision" ]]; then
      echo "No Pkg.Revision in $ANDROID_NDK_HOME/source.properties" >&2
      return 1
    fi
    ndk_version="android.ndkVersion = \"${revision}\""
  fi
  mkdir -p "$gradle_init"
  ANDROID_GRADLE_HOOK="$gradle_init/dust-android.gradle"
  cat > "$ANDROID_GRADLE_HOOK" << EOF
gradle.beforeProject { project ->
    // As the plugin is applied, before variants exist. The NDK the strip
    // tool is taken from is chosen then.
    project.pluginManager.withPlugin("com.android.application") {
        def android = project.extensions.findByName("android")
        if (android != null) {
            ${ndk_path}
            ${ndk_version}
        }
    }
    // Registered before the project's build script runs, so it runs before
    // the Android plugin's own afterEvaluate, the one that makes the
    // variants from the DSL: the DSL can still be changed here, and what is
    // set here is what they are made from.
    project.afterEvaluate {
        def android = project.extensions.findByName("android")
        if (android == null) return
        if (android.defaultConfig.applicationId != "dev.dust.morse") return

        // The version, from Cargo.toml, in place of the versionCode = 1 dx
        // writes.
        android.defaultConfig.versionCode = ${code}
        android.defaultConfig.versionName = "${version}"
        println "Dust: version ${version}, version code ${code}"

        // The native symbol table, packed into the bundle where Play reads
        // it: crashes and ANRs in the Rust code then name their functions.
        // The libraries shipped in the app are stripped of it.
        android.defaultConfig.ndk.debugSymbolLevel = "SYMBOL_TABLE"

        def iconRes = new File("${root}/assets/android")
        if (!iconRes.isDirectory()) {
            throw new GradleException("Dust launcher icon dir missing: " + iconRes)
        }
        def res = project.file("src/main/res")
        project.delete(
            new File(res, "drawable/ic_launcher_background.xml"),
            new File(res, "drawable-v24/ic_launcher_foreground.xml")
        )
        ["mdpi", "hdpi", "xhdpi", "xxhdpi", "xxxhdpi"].each { density ->
            project.delete(new File(res, "mipmap-" + density + "/ic_launcher.webp"))
        }
        project.copy {
            from iconRes
            into res
        }
        println "Dust: installed launcher icon from " + iconRes

        // The activity handles every configuration change itself: one it
        // leaves to Android recreates it, and that ends the app. The
        // manifest is read when it is merged, after this.
        def manifest = project.file("src/main/AndroidManifest.xml")
        def written = manifest.getText("UTF-8")
        def handled = 'android:configChanges="${ANDROID_CONFIG_CHANGES}"'
        def patched = written.replaceFirst(/android:configChanges="[^"]*"/, handled)
        if (!patched.contains(handled)) {
            throw new GradleException("Dust: no android:configChanges in " + manifest)
        }
        if (patched != written) {
            manifest.write(patched, "UTF-8")
        }
        println "Dust: the activity handles every configuration change"
    }
}
EOF
  echo "Android: version ${version}, version code ${code}"
}

# The native debug symbols packed into the app bundle at `aab`, one entry a
# line: what Play reads to name the functions in a crash. Read with the
# JDK's jar, which every Android build already needs.
android_bundle_symbols() {
  local listing
  listing="$(jar tf "$1")" || return 1
  grep '^BUNDLE-METADATA/com.android.tools.build.debugsymbols/.' <<< "$listing" || true
}

# Put a symbol table for every native library into `aab`, where Play reads
# it, and strip that table out of the library the app ships. The Android
# plugin skips both when rustc has already removed the debug info. No-op
# when the bundle already has the symbols. Rewrites the bundle, so call it
# before signing.
pack_android_native_symbols() {
  local aab=$1
  local existing bin objcopy strip
  existing="$(android_bundle_symbols "$aab")"
  if [[ -n "$existing" ]]; then
    return 0
  fi
  bin="${ANDROID_NDK_HOME:?ANDROID_NDK_HOME is not set}/toolchains/llvm/prebuilt/linux-x86_64/bin"
  objcopy="$bin/llvm-objcopy"
  strip="$bin/llvm-strip"
  if [[ ! -x "$objcopy" || ! -x "$strip" ]]; then
    echo "NDK llvm-objcopy or llvm-strip missing in $bin" >&2
    return 1
  fi
  python3 - "$aab" "$objcopy" "$strip" << 'PY'
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

aab = Path(sys.argv[1])
objcopy = sys.argv[2]
strip = sys.argv[3]
prefix = "BUNDLE-METADATA/com.android.tools.build.debugsymbols/"

def is_signature(name: str) -> bool:
    if not name.startswith("META-INF/"):
        return False
    upper = name.upper()
    return upper.endswith((".SF", ".RSA", ".DSA", ".EC")) or name == "META-INF/MANIFEST.MF"

with zipfile.ZipFile(aab, "r") as src:
    libraries = [
        info
        for info in src.infolist()
        if info.filename.startswith("base/lib/") and info.filename.endswith(".so")
    ]
    if not libraries:
        sys.exit("No native libraries in " + str(aab))
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        replacements = {}
        for info in libraries:
            parts = info.filename.split("/")
            # base/lib/<abi>/<name>.so
            if len(parts) != 4:
                sys.exit("Unexpected native library path " + info.filename)
            abi, name = parts[2], parts[3]
            raw = tmp_path / name
            raw.write_bytes(src.read(info.filename))
            sym = tmp_path / f"{name}.sym"
            stripped = tmp_path / f"{name}.stripped"
            subprocess.run([objcopy, "--strip-debug", str(raw), str(sym)], check=True)
            subprocess.run([strip, "--strip-unneeded", "-o", str(stripped), str(raw)], check=True)
            if sym.stat().st_size == 0:
                sys.exit("Empty symbol file for " + info.filename)
            replacements[info.filename] = stripped.read_bytes()
            replacements[f"{prefix}{abi}/{name}.sym"] = sym.read_bytes()
        # Beside the bundle: /tmp can be another filesystem, and replace
        # cannot cross it.
        out = aab.with_suffix(".aab.repacked")
        with zipfile.ZipFile(out, "w") as dst:
            for info in src.infolist():
                if is_signature(info.filename) or info.filename in replacements:
                    continue
                dst.writestr(info, src.read(info.filename))
            for name, data in replacements.items():
                dst.writestr(name, data, compress_type=zipfile.ZIP_DEFLATED)
        out.replace(aab)
PY
}

# Fail unless the APK at `apk` carries the version and version code the
# Cargo.toml under `root` asks for: proof the hook reached the build.
check_android_apk_version() {
  local root=$1 apk=$2
  local version code aapt2 badging
  version="$(dust_version "$root")"
  code="$(dust_version_code "$version")" || return 1
  aapt2="$(find "${ANDROID_HOME:?ANDROID_HOME is not set}/build-tools" -name aapt2 -type f | sort -V | tail -1)"
  if [[ -z "$aapt2" ]]; then
    echo "No aapt2 under $ANDROID_HOME/build-tools" >&2
    return 1
  fi
  # The first line only, cut here rather than by `head`: under pipefail,
  # aapt2 writing on into a closed pipe would fail the whole check.
  badging="$("$aapt2" dump badging "$apk")" || return 1
  badging="${badging%%$'\n'*}"
  echo "$badging"
  if [[ "$badging" != *"versionCode='${code}' versionName='${version}'"* ]]; then
    echo "$apk is not version ${version} with version code ${code}" >&2
    return 1
  fi
}
