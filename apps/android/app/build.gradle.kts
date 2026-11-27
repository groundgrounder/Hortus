// SPDX-License-Identifier: GPL-3.0-only
import org.jetbrains.kotlin.gradle.dsl.JvmTarget
import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.androidx.baselineprofile)
    alias(libs.plugins.androidx.baselineprofile.apptarget)
}

val hortusVersionCode: Int =
    (project.findProperty("hortusVersionCode") as String?)?.toIntOrNull() ?: 2

val releaseKeystorePath = System.getenv("HORTUS_ANDROID_KEYSTORE")?.takeIf { it.isNotBlank() }
val releaseKeystorePassword = System.getenv("HORTUS_ANDROID_KEYSTORE_PASSWORD")?.takeIf { it.isNotBlank() }
val releaseKeyAlias = System.getenv("HORTUS_ANDROID_KEY_ALIAS")?.takeIf { it.isNotBlank() }
val releaseKeyPassword = System.getenv("HORTUS_ANDROID_KEY_PASSWORD")?.takeIf { it.isNotBlank() }
val releaseKeystoreFile = releaseKeystorePath?.let(::file)?.takeIf { it.isFile }
val hasReleaseSigning = releaseKeystoreFile != null &&
    releaseKeystorePassword != null &&
    releaseKeyAlias != null &&
    releaseKeyPassword != null

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
        versionName = "1.0.0"

        ndk {
            abiFilters += "arm64-v8a"
        }
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    if (hasReleaseSigning) {
        signingConfigs {
            create("release") {
                storeFile = releaseKeystoreFile
                storePassword = releaseKeystorePassword
                keyAlias = releaseKeyAlias
                keyPassword = releaseKeyPassword
            }
        }
    }

    buildTypes {
        getByName("debug") {
            // 与 Release 一样裁除不可达平台代码；调试构建仍保持可调试。
            isMinifyEnabled = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
                "proguard-debug-rules.pro",
            )
            testProguardFiles("proguard-test-rules.pro")
        }
        getByName("release") {
            isMinifyEnabled = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            if (hasReleaseSigning) {
                signingConfig = signingConfigs.getByName("release")
            }
            matchingFallbacks += listOf("benchmark")
        }
        create("benchmark") {
            initWith(getByName("release"))
            signingConfig = signingConfigs.getByName("debug")
            matchingFallbacks += listOf("release")
            isDebuggable = false
            isProfileable = true
            versionNameSuffix = "-benchmark"
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }

    sourceSets {
        getByName("benchmark") {
            java.srcDir("src/release/java")
        }
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
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.foundation)
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.material.icons.core)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.androidsvg)
    implementation(libs.jna) {
        artifact {
            type = "aar"
        }
    }
    implementation(libs.androidx.profileinstaller)

    androidTestImplementation(libs.androidx.test.ext.junit)
    androidTestImplementation(libs.androidx.test.runner)
    androidTestImplementation(libs.androidx.test.core)
    androidTestImplementation(libs.androidx.test.uiautomator)
    androidTestImplementation(libs.errorprone.annotations)
    androidTestImplementation("org.jetbrains.kotlin:kotlin-stdlib:2.2.21")
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

baselineProfile {
    saveInSrc = true
    variants {
        create("release") {
            saveInSrc = true
            from(project(":macrobenchmark"))
        }
    }
}
