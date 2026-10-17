// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

pluginManagement {
    repositories {
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositories {
        mavenCentral()
        gradlePluginPortal()
    }
}

rootProject.name = "OpenXPlay-Stream"
include(":core")
include(":clink")

include(":web-server")
project(":web-server").projectDir = file("web-server")

include(":replay")
project(":replay").projectDir = file("tools/replay")

include(":devrelay")
project(":devrelay").projectDir = file("tools/devrelay")

include(":wire")

include(":sim-headunit")
project(":sim-headunit").projectDir = file("tools/sim-headunit")
