#!/usr/bin/env bash
# Build a signed, installable ARM64 APK. Run after installing the prerequisites in docs/ANDROID.md.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export PATH="$ROOT/.tools/bin:$PATH"
if ! command -v cargo >/dev/null && [ -d /opt/homebrew/opt/rustup/bin ]; then
    export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
fi
if [ -z "${ANDROID_HOME:-}" ] && [ -d "$ROOT/.tools/android-sdk" ]; then
    export ANDROID_HOME="$ROOT/.tools/android-sdk"
fi
: "${ANDROID_HOME:?Set ANDROID_HOME to your Android SDK directory}"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/28.2.13676358}"
: "${JAVA_HOME:?Set JAVA_HOME to a JDK 17 installation}"
export GRADLE_USER_HOME="${GRADLE_USER_HOME:-$ROOT/.tools/gradle-home}"
if [ -z "${ANDROID_KEYSTORE:-}" ]; then
    mkdir -p "$ROOT/.tools/android-signing"
    if [ ! -f "$ROOT/.tools/android-signing/local-build.jks" ]; then
        "$JAVA_HOME/bin/keytool" -genkeypair \
            -keystore "$ROOT/.tools/android-signing/local-build.jks" \
            -storepass android -keypass android -alias garage-distiller-local \
            -keyalg RSA -keysize 3072 -validity 10000 -dname 'CN=Garage Distiller Local Build'
    fi
else
    : "${ANDROID_KEYSTORE_PASSWORD:?Set the custom keystore password}"
    : "${ANDROID_KEY_ALIAS:?Set the custom signing alias}"
    : "${ANDROID_KEY_PASSWORD:?Set the custom key password}"
fi
cargo ndk -t arm64-v8a --platform 26 -o android/app/src/main/jniLibs build --locked --release --lib
android/gradlew -p android --no-daemon assembleRelease
VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)"
mkdir -p dist/android
APK="$ROOT/dist/android/garage-distiller-$VERSION-android-arm64.apk"
cp android/app/build/outputs/apk/release/app-release.apk "$APK"
"$ANDROID_HOME/build-tools/35.0.0/apksigner" verify --verbose "$APK"
"$ANDROID_HOME/build-tools/35.0.0/zipalign" -c -P 16 -v 4 "$APK" > "$ROOT/dist/android/alignment.txt"
python3 - "$APK" <<'PY'
from pathlib import Path
import hashlib, sys
apk = Path(sys.argv[1])
apk.with_suffix('.apk.sha256').write_text(hashlib.sha256(apk.read_bytes()).hexdigest() + '  ' + apk.name + '\n')
print(f'APK: {apk}')
PY
