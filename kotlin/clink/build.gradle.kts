// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

plugins {
    alias(libs.plugins.kotlin.jvm)
    `java-test-fixtures`
}
kotlin {
    jvmToolchain(17)
    explicitApi()
}
dependencies {
    implementation(project(":core"))
    implementation(libs.kotlinx.serialization.json)
    testFixturesApi(project(":core"))
    testImplementation(testFixtures(project(":core")))
    testImplementation(libs.kotlin.test.junit5)
    testRuntimeOnly(libs.junit.jupiter.engine)
    testRuntimeOnly(libs.junit.platform.launcher)
}
tasks.named<org.gradle.language.jvm.tasks.ProcessResources>("processTestFixturesResources") {
    from("src/test/resources") { include("head-unit-wfd.txt") }
}
tasks.test {
    useJUnitPlatform()
    systemProperty("clink.coreProjectDir", project(":core").projectDir.absolutePath)
}
