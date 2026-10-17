// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

plugins { alias(libs.plugins.kotlin.jvm) }
kotlin {
    jvmToolchain(17)
    explicitApi()
}
dependencies {
    api(project(":core"))
    implementation(libs.kotlinx.serialization.json)
    testImplementation(testFixtures(project(":core")))
    testImplementation(libs.kotlin.test.junit5)
    testRuntimeOnly(libs.junit.jupiter.engine)
    testRuntimeOnly(libs.junit.platform.launcher)
}
sourceSets.test { resources.srcDir(rootProject.file("../spec")) }
tasks.test { useJUnitPlatform() }
