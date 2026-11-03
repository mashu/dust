#!/usr/bin/env bash
# Gradle init hook for the Android project dx generates.
#
# dx 0.7 leaves six things out for this app, and has a setting for none:
#
# - It writes Android Gradle plugin 8.7 with the separate Kotlin plugin, and
#   Material Components, which nothing in the app uses. Play asks for AGP
#   9.0 or later for R8's newer optimizations, and flags Material's date
#   picker for Window.setStatusBarColor and setNavigationBarColor, which
#   Android 15 deprecates. Before any build script is read, the hook moves
#   the project to AGP 9 (ANDROID_GRADLE_PLUGIN_VERSION), whose Kotlin is
#   built in (no kotlin-android plugin, no kotlinOptions, Kotlin sources
#   where AGP looks for them), and drops Material Components. AGP 9 also
#   removed android.defaults.buildfeatures.buildconfig, which dx still
#   writes and which the plugin refuses to apply while it is set. Gradle
#   reads that file before an init script runs, so the hook drops the line
#   and loads the build's properties again. The app script already sets
#   buildFeatures.buildConfig. dx also sets android.nonFinalResIds=false,
#   and AGP 9's optimized resource shrinking, which the release build uses,
#   requires non-final IDs. The same reload sets that property true.
#
# - It always writes Android's stock green robot as the launcher icon (an
#   adaptive icon under mipmap-anydpi-v26, which is what Android 8+ shows),
#   and never reads bundle.icon.
# - It always writes versionCode = 1. Play turns away an upload whose
#   version code is not above every one uploaded before it, so every build
#   after the first would be refused.
# - It shrinks the release build's code with R8 but not its resources, so
#   every resource of AppCompat and the webview library ships whether the
#   app reaches it or not; Play flags the missing resource shrinking among
#   its R8 findings. The hook turns it on for the release build.
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
#   handle every change itself (ANDROID_CONFIG_CHANGES). The same rewrite
#   takes out android:extractNativeLibs, which dx sets true: AGP 9 rejects
#   that attribute, and the legacy packaging it asks for is what Play
#   refuses of an app targeting Android 15. Without it the plugin stores
#   the native libraries uncompressed and aligned.
#
# The app's own activity (android/MainActivity.kt, which Dioxus.toml points
# dx at) keeps the webview clear of the system bars and the keyboard once
# Android 15 draws it edge to edge, and its theme (assets/android/values*)
# colours the bars without the deprecated calls.
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

# The Android Gradle plugin the project is moved to, and the Kotlin Gradle
# plugin it is built with: AGP 9.0's own, the least its built-in Kotlin
# takes. AGP 9.0 runs on the Gradle 9.1 dx's wrapper asks for; a later 9.x
# would need a later Gradle than dx writes.
ANDROID_GRADLE_PLUGIN_VERSION="9.0.1"
KOTLIN_GRADLE_PLUGIN_VERSION="2.2.10"

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
// AGP ${ANDROID_GRADLE_PLUGIN_VERSION} with its built-in Kotlin, and no Material Components, in
// the build files dx writes: settled once the settings are read, before
// either build script is compiled, so each is read as written here. Only
// Dust's project (its application id) is touched.
//
// Gradle reads gradle.properties before this init script runs, so these
// lines are rewritten and the build's properties are loaded again. AGP 9
// removed android.defaults.buildfeatures.buildconfig and will not apply
// while it is set. buildFeatures.buildConfig in the app script turns
// BuildConfig on. Optimized resource shrinking requires non-final resource
// IDs, and dx sets android.nonFinalResIds=false.
def dustRoot = gradle.startParameter.projectDir
if (dustRoot == null) {
    dustRoot = gradle.startParameter.currentDir
}
def dustApp = new File(dustRoot, "app/build.gradle.kts")
def dustProps = new File(dustRoot, "gradle.properties")
if (dustApp.isFile() && dustProps.isFile()
    && dustApp.getText("UTF-8").contains('applicationId = "dev.dust.morse"')) {
    def kept = dustProps.readLines("UTF-8").findAll { line ->
        !line.trim().startsWith("android.defaults.buildfeatures.buildconfig")
    }.collect { line ->
        line.trim() == "android.nonFinalResIds=false" ? "android.nonFinalResIds=true" : line
    }
    def rewritten = kept.join("\n") + "\n"
    if (rewritten != dustProps.getText("UTF-8")) {
        dustProps.write(rewritten, "UTF-8")
        println "Dust: gradle.properties adjusted for AGP 9"
    }
    def controller = gradle.services.get(org.gradle.api.internal.properties.GradlePropertiesController)
    def buildId = gradle.owner.buildIdentifier
    controller.unloadGradleProperties(buildId)
    controller.loadGradleProperties(buildId, dustRoot, true)
}

gradle.settingsEvaluated { settings ->
    def rootScript = new File(settings.rootDir, "build.gradle.kts")
    def appScript = new File(settings.rootDir, "app/build.gradle.kts")
    if (!rootScript.isFile() || !appScript.isFile()) return
    if (!appScript.getText("UTF-8").contains('applicationId = "dev.dust.morse"')) return

    def lines = rootScript.readLines("UTF-8")
    def agp = lines.findIndexOf { it.trim().startsWith('classpath("com.android.tools.build:gradle:') }
    if (agp < 0) {
        throw new GradleException("Dust: no Android Gradle plugin on the classpath in " + rootScript)
    }
    lines[agp] = '        classpath("com.android.tools.build:gradle:${ANDROID_GRADLE_PLUGIN_VERSION}")'
    def kgp = lines.findIndexOf { it.trim().startsWith('classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:') }
    if (kgp >= 0) {
        lines[kgp] = '        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:${KOTLIN_GRADLE_PLUGIN_VERSION}")'
    }
    rootScript.write(lines.join("\n") + "\n", "UTF-8")

    // Built-in Kotlin compiles the sources under src/main/kotlin and
    // src/main/java itself, to the Java target compileOptions sets; the
    // kotlin-android plugin, its kotlinOptions and Kotlin sources added to
    // the Java set are errors under it.
    def kept = []
    def skipping = 0
    appScript.readLines("UTF-8").each { line ->
        def trimmed = line.trim()
        if (skipping > 0) {
            skipping += line.count("{") - line.count("}")
            return
        }
        if (trimmed == 'id("org.jetbrains.kotlin.android")') return
        if (trimmed.startsWith('implementation("com.google.android.material:material:')) return
        if (trimmed == "kotlinOptions {" || trimmed == "sourceSets {") {
            skipping = line.count("{") - line.count("}")
            return
        }
        kept << line
    }
    appScript.write(kept.join("\n") + "\n", "UTF-8")
    println "Dust: Android Gradle plugin ${ANDROID_GRADLE_PLUGIN_VERSION}, built-in Kotlin, no Material Components"
}

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

        // The release build's resources shrunk, as its code already is: R8
        // drops every resource nothing reaches, AppCompat's and the
        // webview library's unused layouts and drawables among them. The
        // app reaches its own only from the manifest (the launcher icon,
        // the label, the theme, the network security config), which the
        // shrinker always keeps, and nothing looks one up by name.
        def release = android.buildTypes.findByName("release")
        if (release == null) {
            throw new GradleException("Dust: no release build type to shrink")
        }
        release.shrinkResources = true
        println "Dust: the release build shrinks its resources"

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
        // AGP 9 rejects this attribute. Leaving it out stores the native
        // libraries uncompressed and 16 KB aligned, which Play requires.
        patched = patched.replaceFirst(/\s*android:extractNativeLibs="[^"]*"/, "")
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

# Fail unless the APK or app bundle at `archive` was built with the Android
# Gradle plugin the hook moves the project to, or a later one, as its build
# metadata records, and holds none of Material Components' classes: proof
# the hook's move to AGP 9 and its drop of Material reached the build.
check_android_build() {
  python3 - "$1" "$ANDROID_GRADLE_PLUGIN_VERSION" << 'PY'
import re
import sys
import zipfile

archive, wanted = sys.argv[1], sys.argv[2]
metadata_names = (
    "META-INF/com/android/build/gradle/app-metadata.properties",
    "BUNDLE-METADATA/com.android.tools.build.gradle/app-metadata.properties",
)
with zipfile.ZipFile(archive) as bundle:
    names = bundle.namelist()
    metadata = next((name for name in metadata_names if name in names), None)
    if metadata is None:
        sys.exit("No Android Gradle plugin metadata in " + archive)
    properties = dict(
        line.split("=", 1)
        for line in bundle.read(metadata).decode("utf-8").splitlines()
        if "=" in line
    )
    built = properties.get("androidGradlePluginVersion", "")
    version = lambda text: tuple(int(part) for part in re.findall(r"\d+", text)[:3])
    if not built or version(built) < version(wanted):
        sys.exit(f"{archive} was built with Android Gradle plugin {built or 'unknown'}, not {wanted} or later")
    dexes = [name for name in names if re.fullmatch(r"(base/dex/|)classes\d*\.dex", name)]
    if not dexes:
        sys.exit("No dex files in " + archive)
    material = [name for name in dexes if b"Lcom/google/android/material/" in bundle.read(name)]
    if material:
        sys.exit("Material Components' classes in " + ", ".join(material))
    print(f"Android Gradle plugin {built}; no Material Components in {', '.join(dexes)}")
PY
}

# Fail unless every native library in the APK or app bundle at `archive`
# has its segments aligned to 16 KB pages. Play takes no app targeting
# Android 15 or later without that, and a phone with 16 KB pages may not
# load the library at all. .cargo/config.toml asks the linker for it.
check_android_page_size() {
  python3 - "$1" << 'PY'
import re
import struct
import sys
import zipfile

archive = sys.argv[1]
page = 16384
with zipfile.ZipFile(archive) as bundle:
    libraries = [
        name for name in bundle.namelist() if re.fullmatch(r"(base/)?lib/[^/]+/[^/]+\.so", name)
    ]
    if not libraries:
        sys.exit("No native libraries in " + archive)
    misaligned = []
    for name in libraries:
        elf = bundle.read(name)
        if elf[:4] != b"\x7fELF":
            sys.exit(name + " is not an ELF file")
        wide = elf[4] == 2
        order = "<" if elf[5] == 1 else ">"
        word = "Q" if wide else "I"
        (table,) = struct.unpack_from(order + word, elf, 0x20 if wide else 0x1C)
        size, count = struct.unpack_from(order + "HH", elf, 0x36 if wide else 0x2A)
        for index in range(count):
            header = table + index * size
            (kind,) = struct.unpack_from(order + "I", elf, header)
            (align,) = struct.unpack_from(order + word, elf, header + (48 if wide else 28))
            # PT_LOAD: a segment the loader maps.
            if kind == 1 and align < page:
                misaligned.append(f"{name}: a segment aligned to {align} bytes")
                break
    if misaligned:
        sys.exit("Not aligned to 16 KB pages:\n  " + "\n  ".join(misaligned))
    print(f"Native libraries aligned to 16 KB pages: {', '.join(libraries)}")
PY
}
