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

# `release` and `debug` build an APK, which is what a phone installs directly.
# `bundle` builds an Android App Bundle, which is the only thing Google Play
# accepts for a new application and which a phone cannot install at all.
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

# Which key signed it, said before the build rather than discovered when Play
# rejects the upload. `release` means two different things depending on whether
# this machine holds the release key, and silence about that is how somebody
# ships a debug-signed build believing otherwise.
if [ -f "$here/android/keystore.properties" ]; then
    echo "== signing with the release key named in android/keystore.properties"
else
    echo "== NO release key on this machine: android/keystore.properties is absent."
    echo "   The result will be DEBUG-SIGNED -- fine for sideloading onto a phone,"
    echo "   rejected by Google Play. See docs/ANDROID-RELEASE.md to make one."
fi

echo "== the application"
cd "$here/android"
if [ "$variant" = bundle ]; then
    "$gradle" --no-daemon ":app:bundleRelease"
    echo
    find "$here/android/app/build/outputs/bundle" -name '*.aab' -newermt '-10 minutes' -print
else
    "$gradle" --no-daemon ":app:assemble${variant^}"
    echo
    find "$here/android/app/build/outputs/apk" -name '*.apk' -newermt '-10 minutes' -print
fi
