#!/usr/bin/env bash
# Build the Android application, native library first.
#
# Two steps in one place, because doing them in the wrong order produces an APK
# with a stale `.so` inside it and no error anywhere: Gradle packages whatever
# is in `jniLibs`, and has no idea the Rust changed.
#
# Needs a JDK, the Android SDK, an NDK and `cargo-ndk`. Point ANDROID_HOME and
# ANDROID_NDK_HOME at them, or accept the defaults this project was set up with.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
export ANDROID_HOME="${ANDROID_HOME:-D:/Android/Sdk}"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/27.3.13750724}"
export JAVA_HOME="${JAVA_HOME:-D:/Android/jdk}"
gradle="${GRADLE:-D:/Android/gradle-8.10.2/bin/gradle}"

variant="${1:-release}"

if ! command -v cargo-ndk >/dev/null 2>&1; then
    echo "cargo-ndk is not installed: cargo install cargo-ndk" >&2
    exit 1
fi

echo "== the core, for three ABIs"
cd "$here"
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 \
    -o android/app/src/main/jniLibs \
    build --release -p itsanas-android

# cargo-ndk copies every shared object it finds beside the one that matters.
# The application links none of them -- checked with `llvm-readelf -d`, which
# lists only libdl and libc -- so they are weight in the APK and nothing else.
find android/app/src/main/jniLibs -name 'lib*-*.so' -delete

echo "== the application"
cd "$here/android"
"$gradle" --no-daemon ":app:assemble${variant^}"

echo
find "$here/android/app/build/outputs/apk" -name '*.apk' -newermt '-10 minutes' -print
