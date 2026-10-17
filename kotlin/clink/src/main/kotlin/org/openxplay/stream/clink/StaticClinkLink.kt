// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.clink

/** Already joined network. The caller owns network availability and teardown. */
public class StaticClinkLink(private val addresses: ClinkAddresses) : ClinkLink {
    override fun connect(onLinkLost: (Throwable?) -> Unit): ClinkAddresses = addresses
    override fun disconnect(): Unit = Unit
}
