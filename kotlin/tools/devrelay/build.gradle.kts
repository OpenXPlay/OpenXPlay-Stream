// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

plugins {
    alias(libs.plugins.kotlin.jvm)
    application
}

kotlin { jvmToolchain(17) }

dependencies {
    implementation(project(":core"))
    implementation(project(":replay"))
    implementation(project(":web-server"))
    implementation(project(":clink"))
    implementation(project(":cast"))
    testImplementation(libs.kotlin.test.junit5)
    testRuntimeOnly(libs.junit.jupiter.engine)
    testRuntimeOnly(libs.junit.platform.launcher)
}

application { mainClass.set("org.openxplay.stream.devrelay.DevRelayMain") }

tasks.named<JavaExec>("run") {
    workingDir(rootProject.projectDir)
    systemProperty("devrelay.repositoryRoot", rootProject.projectDir.absolutePath)
}

tasks.test {
    useJUnitPlatform()
    systemProperty("devrelay.repositoryRoot", rootProject.projectDir.absolutePath)
}
