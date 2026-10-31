import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// The release key, if this machine has one.
//
// `android/keystore.properties` is git-ignored and names a keystore that is
// also git-ignored. Nicolas generates and holds both; nothing in this
// repository, and no agent, ever sees them. Losing that file means losing the
// ability to update the application on Play for ever -- Google will not
// re-key a listing -- so it is backed up like the 24 words are.
val releaseKey = Properties().apply {
    val file = rootProject.file("keystore.properties")
    if (file.exists()) {
        file.inputStream().use { load(it) }
    }
}
val hasReleaseKey = releaseKey.getProperty("storeFile") != null

// The application's version is the workspace's (`Cargo.toml`), and its
// versionCode is derived from it as `itsanas_release::Version::android_code`
// derives it: MAJOR * 1_000_000 + MINOR * 1_000 + PATCH. Android installs an
// update only over a smaller code, and this said `versionCode = 1` for every
// build until 2026-10-31, so no update could ever have installed over it.
val workspaceVersion: String = rootProject.file("../Cargo.toml").readLines()
    .first { it.startsWith("version = ") }
    .substringAfter('"').substringBefore('"')
val workspaceCode: Int = workspaceVersion.split('.').map(String::toInt).let { (major, minor, patch) ->
    require(minor < 1000 && patch < 1000) { "version $workspaceVersion does not fit a versionCode" }
    major * 1_000_000 + minor * 1_000 + patch
}

android {
    namespace = "fr.ngas.itsanas"
    compileSdk = 35

    defaultConfig {
        applicationId = "fr.ngas.itsanas"

        // 26 rather than a lower number, and the reason is not fashion: below
        // Oreo there are no foreground service channels, and this application
        // is a sync service before it is anything else. It covers every phone
        // sold since 2017.
        minSdk = 26
        targetSdk = 35
        versionCode = workspaceCode
        versionName = workspaceVersion

        ndk {
            // The three the native library is built for. `arm64-v8a` is every
            // phone worth speaking of; `armeabi-v7a` is the old ones; `x86_64`
            // is the emulator, which is how this gets tested without a phone
            // in the room.
            abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64")
        }
    }

    signingConfigs {
        if (hasReleaseKey) {
            create("release") {
                storeFile = rootProject.file(releaseKey.getProperty("storeFile"))
                storePassword = releaseKey.getProperty("storePassword")
                keyAlias = releaseKey.getProperty("keyAlias")
                keyPassword = releaseKey.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        release {
            // Not shrunk. The Kotlin here is a few hundred lines and the weight
            // is the native library, which R8 does not touch — so minification
            // would buy nothing and would need a keep rule for every JNI entry
            // point, which is exactly the sort of list that goes stale silently
            // and fails at runtime on somebody's phone.
            isMinifyEnabled = false

            // Debug-signed when there is no release key, because a build that
            // fails on a developer machine for want of a secret is a build
            // nobody runs. The trade is that "release" then means two
            // different things, so the build says which one out loud rather
            // than leaving somebody to discover it when Play rejects the
            // upload.
            signingConfig = if (hasReleaseKey) {
                signingConfigs.getByName("release")
            } else {
                signingConfigs.getByName("debug")
            }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    buildFeatures {
        compose = true
        // BuildConfig.VERSION_NAME: the version the update check names.
        buildConfig = true
    }

    packaging {
        // The .so files are already stripped by the Rust release profile and
        // must be extractable: `System.loadLibrary` on API 26 cannot load from
        // a compressed APK entry on every device.
        jniLibs {
            useLegacyPackaging = false
        }
    }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2024.12.01"))
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.7")
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.security:security-crypto:1.1.0-alpha06")
    // Unlocking with a fingerprint or a face (Biometric.kt).
    implementation("androidx.biometric:biometric:1.1.0")
    debugImplementation("androidx.compose.ui:ui-tooling")
}
