plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
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
        versionCode = 1
        versionName = "0.1.0"

        ndk {
            // The three the native library is built for. `arm64-v8a` is every
            // phone worth speaking of; `armeabi-v7a` is the old ones; `x86_64`
            // is the emulator, which is how this gets tested without a phone
            // in the room.
            abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64")
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
            signingConfig = signingConfigs.getByName("debug")
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
    debugImplementation("androidx.compose.ui:ui-tooling")
}
