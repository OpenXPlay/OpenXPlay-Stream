// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-02

plugins { alias(libs.plugins.kotlin.jvm) }
kotlin {
    jvmToolchain(17)
    explicitApi()
}
dependencies {
    api(project(":core"))
    testImplementation(testFixtures(project(":core")))
    testImplementation(libs.kotlin.test.junit5)
    testRuntimeOnly(libs.junit.jupiter.engine)
    testRuntimeOnly(libs.junit.platform.launcher)
}
tasks.test { useJUnitPlatform() }
sourceSets.test { resources.srcDir(rootProject.file("../spec")) }
