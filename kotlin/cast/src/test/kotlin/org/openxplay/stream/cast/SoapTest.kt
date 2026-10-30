// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

package org.openxplay.stream.cast

import java.net.URI
import org.junit.jupiter.api.Test
import kotlin.test.*

class SoapTest {
    @Test fun missingOrInvalidFaultCodeBecomesMinusOne() {
        for (code in listOf("", "not-a-number", "999999999999999999999")) {
            val error = assertFailsWith<SoapFault> { CastSoap.parse("Play", 500, fault().replace("<errorCode>714</errorCode>", "<errorCode>$code</errorCode>")) }
            assertEquals(-1, error.code); assertEquals("Test rejected", error.description)
        }
        val missing = assertFailsWith<SoapFault> { CastSoap.parse("Play", 500, fault().replace("<errorCode>714</errorCode>", "")) }
        assertEquals("action=Play code=-1", missing.detail())
    }
    @Test fun didlIsEscapedAgainAsSoapArgumentAndResponseArgumentsAreParsed() {
        val url = "http://127.0.0.1:1234/cast/token/1.ts?a=1&b=2"
        val didl = CastSoap.metadata("Car <&> 'play'", url)
        assertContains(didl, "Car &lt;&amp;&gt; &apos;play&apos;")
        assertContains(didl, "http-get:*:video/mp2t:$DLNA_FEATURES")
        val request = CastSoap.envelope("${AVTRANSPORT_TYPE}3", "SetAVTransportURI", mapOf("InstanceID" to "0", "CurrentURI" to url, "CurrentURIMetaData" to didl))
        assertContains(request, "&lt;DIDL-Lite"); assertContains(request, "Car &amp;lt;&amp;amp;&amp;gt;")
        assertContains(request, "xmlns:u=\"${AVTRANSPORT_TYPE}3\"")
        assertContains(request, "s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"")
        val args = XmlReader(request).read().child("Body")!!.children.single()
        assertEquals(didl, args.value("CurrentURIMetaData")); assertEquals(url, args.value("CurrentURI"))
        assertEquals(mapOf("CurrentTransportState" to "PLAYING"), CastSoap.parse("GetTransportInfo", 200,
            soapResponse("GetTransportInfo", mapOf("CurrentTransportState" to "PLAYING"))))
        val error = assertFailsWith<SoapFault> { CastSoap.parse("Play", 500, fault(701)) }
        assertEquals("action=Play code=701", error.detail())
    }
    @Test fun soapHttpRequestUsesQuotedActionAndExactServiceVersion() {
        OneHttpServer { socket, head, body ->
            assertEquals("POST /control HTTP/1.1", head.start)
            assertEquals("\"${AVTRANSPORT_TYPE}3#Play\"", head.headers["soapaction"])
            assertEquals("text/xml; charset=\"utf-8\"", head.headers["content-type"])
            assertEquals("close", head.headers["connection"])
            assertEquals(CastSoap.envelope("${AVTRANSPORT_TYPE}3", "Play", mapOf("InstanceID" to "0", "Speed" to "1")), String(body))
            respond(socket, soapResponse("Play"))
        }.use { server ->
            val result = CastSoap(CastHttpClient(LOOPBACK_NETWORK)).action(CastService("${AVTRANSPORT_TYPE}3", server.uri.resolve("/control")),
                "Play", mapOf("InstanceID" to "0", "Speed" to "1"))
            assertEquals(LOOPBACK, result.localAddress); server.assertHealthy()
        }
    }
}
