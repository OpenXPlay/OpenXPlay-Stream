// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

plugins {
    alias(libs.plugins.kotlin.jvm)
    application
}
kotlin { jvmToolchain(17); explicitApi() }
dependencies {
    implementation(testFixtures(project(":clink")))
    implementation(libs.kotlinx.serialization.json)
    testImplementation(project(":wire"))
    testImplementation(testFixtures(project(":core")))
    testImplementation(libs.kotlin.test.junit5)
    testRuntimeOnly(libs.junit.jupiter.engine)
    testRuntimeOnly(libs.junit.platform.launcher)
}
application { mainClass.set("org.openxplay.stream.simheadunit.SimHeadUnitMain") }
tasks.named<JavaExec>("run") { standardInput = System.`in` }
tasks.test {
    useJUnitPlatform()
    systemProperty("sim.repository", rootProject.projectDir.absolutePath)
}
