// SPDX-License-Identifier: AGPL-3.0-only
plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.kotlin.android) apply false
    alias(libs.plugins.kotlin.compose) apply false
    alias(libs.plugins.android.test) apply false
    alias(libs.plugins.androidx.baselineprofile) apply false
    alias(libs.plugins.androidx.baselineprofile.apptarget) apply false
    alias(libs.plugins.androidx.baselineprofile.producer) apply false
}
