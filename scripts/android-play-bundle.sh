#!/usr/bin/env bash
# Build a Play Store Android App Bundle and sign it with the upload key.
#
# Prerequisites:
#   JDK 17 or 21 with javac, writable Android SDK, NDK 27,
#   rustup target aarch64-linux-android, dx 0.7.10
#   Upload keystore at ~/.android/dust-play-upload.jks
#   Passwords in ~/.android/dust-play-upload.env (mode 600):
#     KEYSTORE_PASSWORD=...
#     KEY_PASSWORD=...
#
# The first AAB you upload to Play Console with this key becomes the upload
# key if you let Google generate the app signing key.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

VERSION="$(awk '
  /^\[workspace.package\]/ { in_pkg = 1; next }
  in_pkg && /^\[/ { in_pkg = 0 }
  in_pkg && /^version *=/ { gsub(/[" ]/, "", $3); print $3; exit }
' Cargo.toml)"

KEYSTORE="${DUST_ANDROID_KEYSTORE:-$HOME/.android/dust-play-upload.jks}"
ENV_FILE="${DUST_ANDROID_KEYSTORE_ENV:-$HOME/.android/dust-play-upload.env}"
KEY_ALIAS="${DUST_ANDROID_KEY_ALIAS:-upload}"
NDK_VERSION="${NDK_VERSION:-27.2.12479018}"

pick_java_home() {
  local candidate
  for candidate in \
    "${JAVA_HOME:-}" \
    /usr/lib/jvm/java-17-openjdk-amd64 \
    /usr/lib/jvm/java-21-openjdk-amd64 \
    /usr/lib/jvm/java-21-openjdk \
    /usr/lib/jvm/java-17-openjdk
  do
    if [[ -n "$candidate" && -x "$candidate/bin/javac" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}
JAVA_HOME="$(pick_java_home)" || {
  echo "Need a JDK with javac (openjdk-17-jdk or openjdk-21-jdk)." >&2
  exit 1
}
export JAVA_HOME
export PATH="$JAVA_HOME/bin:$PATH"
if [[ -z "${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}" ]]; then
  if [[ -d "$HOME/Android/Sdk/build-tools" ]]; then
    export ANDROID_HOME="$HOME/Android/Sdk"
  else
    export ANDROID_HOME="/usr/lib/android-sdk"
  fi
else
  export ANDROID_HOME="${ANDROID_HOME:-$ANDROID_SDK_ROOT}"
fi
export ANDROID_SDK_ROOT="$ANDROID_HOME"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$HOME/Android/Sdk/ndk/${NDK_VERSION}}"
export ANDROID_NDK_ROOT="$ANDROID_NDK_HOME"
export NDK_HOME="$ANDROID_NDK_HOME"

if [[ ! -x "$JAVA_HOME/bin/java" ]]; then
  echo "JDK 17 not found at $JAVA_HOME" >&2
  exit 1
fi
if [[ ! -d "$ANDROID_HOME" ]]; then
  echo "Android SDK not found at $ANDROID_HOME" >&2
  exit 1
fi
if [[ ! -d "$ANDROID_NDK_HOME" ]]; then
  echo "Android NDK not found at $ANDROID_NDK_HOME" >&2
  exit 1
fi
if [[ ! -f "$KEYSTORE" ]]; then
  echo "Upload keystore not found at $KEYSTORE" >&2
  echo "Create one with keytool; see README Android / Play Store." >&2
  exit 1
fi
if [[ ! -f "$ENV_FILE" ]]; then
  echo "Keystore env file not found at $ENV_FILE" >&2
  exit 1
fi

# shellcheck disable=SC1090
source "$ENV_FILE"
: "${KEYSTORE_PASSWORD:?KEYSTORE_PASSWORD missing in $ENV_FILE}"
: "${KEY_PASSWORD:?KEY_PASSWORD missing in $ENV_FILE}"

bin="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin"
cc="$bin/aarch64-linux-android24-clang"
if [[ ! -x "$cc" ]]; then
  cc="$(find "$bin" -maxdepth 1 -name 'aarch64-linux-android*-clang' | sort -V | head -1 || true)"
fi
if [[ ! -x "$cc" ]]; then
  echo "No aarch64 clang wrapper in $bin" >&2
  exit 1
fi
export CC_aarch64_linux_android="$cc"
export CXX_aarch64_linux_android="${cc}++"
export AR_aarch64_linux_android="$bin/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$cc"

echo "dx bundle --platform android --release (AAB)"
dx bundle --release --locked \
  --platform android \
  --target aarch64-linux-android \
  --package-types aab

aab="$(find target/dx -name '*.aab' -type f -printf '%T@ %p\n' | sort -nr | head -1 | cut -d' ' -f2-)"
if [[ -z "$aab" ]]; then
  echo "No AAB was produced" >&2
  exit 1
fi

mkdir -p dist
out="dist/dust-${VERSION}-android-arm64.aab"
cp "$aab" "$out"

echo "Signing $out"
jarsigner -sigalg SHA256withRSA -digestalg SHA256 \
  -keystore "$KEYSTORE" \
  -storepass "$KEYSTORE_PASSWORD" \
  -keypass "$KEY_PASSWORD" \
  "$out" "$KEY_ALIAS"

jarsigner -verify -certs "$out"

cert_out="dist/dust-upload-certificate.pem"
keytool -exportcert -rfc \
  -keystore "$KEYSTORE" \
  -alias "$KEY_ALIAS" \
  -storepass "$KEYSTORE_PASSWORD" \
  -file "$cert_out" >/dev/null

ls -lh "$out" "$cert_out"
echo "Upload $out to Play Console (Release → Production / testing)."
echo "Public upload certificate: $cert_out"
