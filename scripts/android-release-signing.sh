#!/usr/bin/env bash
# The release key for the APKs CI builds.
#
# An APK installs over an earlier one only if both carry the same signing
# key. dx 0.7 builds Android's release build type (minified, not
# debuggable) only when Dioxus.toml has a [bundle.android] table, and then
# signs it with the keystore that table names; dx 0.7.10 reads no
# [android.signing], whatever its docs say. Without the table it builds the
# debug type, signed with a debug key Gradle makes afresh on every runner,
# so no APK could update the one before it and every update meant an
# uninstall, which deletes the app's history.
#
# The table is written at build time from four repository secrets, so no
# key or password is ever committed:
#
#   ANDROID_KEYSTORE_BASE64    the keystore file, base64-encoded
#   ANDROID_KEYSTORE_PASSWORD  its password
#   ANDROID_KEY_ALIAS          the key's alias in it
#   ANDROID_KEY_PASSWORD       the key's password
#
# docs/platforms.md (Release signing) says how to make them. Source this,
# call install_android_release_signing before `dx bundle --release`, and
# check_android_apk_release on the APK it writes. The Gradle project dx
# writes under target/dx then holds the passwords: never cache it.

# Write the keystore to `keystore` and add a [bundle.android] table naming
# it to the Dioxus.toml under `root`. Sets ANDROID_RELEASE_CERT_SHA256 to
# the SHA-256 of the key's certificate, for check_android_apk_release.
install_android_release_signing() {
  local root=$1 keystore=$2
  local name missing=()
  for name in ANDROID_KEYSTORE_BASE64 ANDROID_KEYSTORE_PASSWORD ANDROID_KEY_ALIAS ANDROID_KEY_PASSWORD; do
    if [[ -z "${!name:-}" ]]; then
      missing+=("$name")
    fi
  done
  if ((${#missing[@]})); then
    echo "No Android release key: ${missing[*]} not set." >&2
    echo "docs/platforms.md, Release signing, says how to add them." >&2
    return 1
  fi
  # dx copies these into a Kotlin string through a template that escapes
  # HTML: anything else would reach Gradle changed, or end the string.
  for name in ANDROID_KEYSTORE_PASSWORD ANDROID_KEY_ALIAS ANDROID_KEY_PASSWORD; do
    if [[ ! "${!name}" =~ ^[A-Za-z0-9._-]+$ ]]; then
      echo "$name may hold only letters, digits, '.', '_' and '-'." >&2
      return 1
    fi
  done
  if grep -q '^\[bundle\.android\]' "$root/Dioxus.toml"; then
    echo "$root/Dioxus.toml already has a [bundle.android] table." >&2
    return 1
  fi

  if ! (umask 077 && printf '%s' "$ANDROID_KEYSTORE_BASE64" | tr -d '[:space:]' | base64 -d > "$keystore"); then
    echo "ANDROID_KEYSTORE_BASE64 is not base64." >&2
    return 1
  fi
  local der="${keystore}.der" said digest
  if ! said="$(keytool -exportcert -keystore "$keystore" -storepass:env ANDROID_KEYSTORE_PASSWORD \
    -alias "$ANDROID_KEY_ALIAS" -file "$der" 2>&1)"; then
    echo "$said" >&2
    echo "No key '$ANDROID_KEY_ALIAS' in ANDROID_KEYSTORE_BASE64 that ANDROID_KEYSTORE_PASSWORD opens." >&2
    return 1
  fi
  digest="$(sha256sum < "$der")"
  rm -f "$der"
  ANDROID_RELEASE_CERT_SHA256="${digest%% *}"

  # dx reads jks_file relative to the folder that holds target/.
  local jks
  jks="$(realpath -m --relative-to="$root" "$keystore")"
  if [[ ! "$jks" =~ ^[A-Za-z0-9._/-]+$ ]]; then
    echo "Keystore path $jks has characters dx would change." >&2
    return 1
  fi
  cat >> "$root/Dioxus.toml" << EOF

[bundle.android]
jks_file = "${jks}"
jks_password = "${ANDROID_KEYSTORE_PASSWORD}"
key_alias = "${ANDROID_KEY_ALIAS}"
key_password = "${ANDROID_KEY_PASSWORD}"
EOF
  echo "Android: release key certificate SHA-256 ${ANDROID_RELEASE_CERT_SHA256}"
}

# Fail unless the APK at `apk` is not debuggable and is signed with the key
# whose certificate has the SHA-256 `cert`: proof dx built the release type
# and signed it with the release key, not the debug one.
check_android_apk_release() {
  local apk=$1 cert=$2
  local aapt2 apksigner badging signers
  aapt2="$(find "${ANDROID_HOME:?ANDROID_HOME is not set}/build-tools" -name aapt2 -type f | sort -V | tail -1)"
  apksigner="$(find "$ANDROID_HOME/build-tools" -name apksigner -type f | sort -V | tail -1)"
  if [[ -z "$aapt2" || -z "$apksigner" ]]; then
    echo "No aapt2 or apksigner under $ANDROID_HOME/build-tools" >&2
    return 1
  fi
  badging="$("$aapt2" dump badging "$apk")" || return 1
  if [[ $'\n'"$badging"$'\n' == *$'\n'application-debuggable$'\n'* ]]; then
    echo "$apk is debuggable: dx built the debug build type." >&2
    return 1
  fi
  signers="$("$apksigner" verify --print-certs "$apk")" || return 1
  echo "$signers"
  if [[ "$signers" != *"certificate SHA-256 digest: ${cert}"* ]]; then
    echo "$apk is not signed with the release key (certificate SHA-256 ${cert})." >&2
    return 1
  fi
}
