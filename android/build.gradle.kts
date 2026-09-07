// The Android shell. The core is Rust, cross-compiled into
// `app/src/main/jniLibs` by `scripts/build-apk.sh`; nothing here reimplements
// anything the command line already does.
plugins {
    id("com.android.application") version "8.7.3" apply false
    id("org.jetbrains.kotlin.android") version "2.0.21" apply false
    id("org.jetbrains.kotlin.plugin.compose") version "2.0.21" apply false
}
