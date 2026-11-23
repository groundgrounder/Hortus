// SPDX-License-Identifier: GPL-3.0-only
import org.jetbrains.kotlin.gradle.dsl.JvmTarget
import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

val hortusVersionCode: Int =
    (project.findProperty("hortusVersionCode") as String?)?.toIntOrNull() ?: 1

android {
    namespace = "io.github.groundgrounder.hortus"
    compileSdk = 36
    providers.gradleProperty("hortusNdkPath").orNull?.let { root ->
        val properties = Properties()
        file("$root/source.properties").inputStream().use(properties::load)
        ndkPath = root
        ndkVersion = properties.getProperty("Pkg.Revision")
    }

    defaultConfig {
        applicationId = "io.github.groundgrounder.hortus"
        minSdk = 29
        targetSdk = 36
        versionCode = hortusVersionCode
        versionName = "0.5.0"

        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    buildTypes.configureEach {
        // 两种 APK 均裁除 AndroidX 里 API 29 不可达的旧平台代码。
        isMinifyEnabled = true
        proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }

    sourceSets {
        getByName("main") {
            java.srcDir(layout.buildDirectory.dir("generated/uniffi/kotlin"))
            assets.srcDir(layout.buildDirectory.dir("generated/hortusAssets"))
        }
    }

    packaging {
        jniLibs {
            useLegacyPackaging = false
            // JNA 的预打包库已经剥离，无需由 NDK 再处理。
            keepDebugSymbols += "**/libjnidispatch.so"
        }
    }
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.foundation)
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.material3)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.jna) {
        artifact {
            type = "aar"
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}
