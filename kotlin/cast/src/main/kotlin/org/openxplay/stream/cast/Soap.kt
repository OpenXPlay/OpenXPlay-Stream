// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

package org.openxplay.stream.cast

import java.io.IOException
import java.net.InetAddress

internal class SoapFault(val action: String, val code: Int, val description: String) : IOException("SOAP fault") {
    fun detail(): String = "action=$action code=$code description=${description.replace(Regex("https?://\\S+", RegexOption.IGNORE_CASE), "[URL]").take(160)}"
}
internal data class SoapResult(val arguments: Map<String, String>, val localAddress: InetAddress)
internal class CastSoap(private val http: CastHttpClient) {
    fun action(service: CastService, action: String, arguments: Map<String, String> = mapOf("InstanceID" to "0"),
        budgetMs: Int = 7000): SoapResult {
        val response = http.request(service.control, "POST", mapOf("Content-Type" to "text/xml; charset=\"utf-8\"",
            "SOAPACTION" to "\"${service.type}#$action\""), envelope(service.type, action, arguments).toByteArray(Charsets.UTF_8), budgetMs = budgetMs)
        return SoapResult(parse(action, response.status, String(response.body, Charsets.UTF_8)), response.localAddress)
    }
    companion object {
        fun envelope(service: String, action: String, arguments: Map<String, String>): String =
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>" +
                "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body>" +
                "<u:$action xmlns:u=\"${xmlEscape(service)}\">" + arguments.entries.joinToString("") { (key, value) -> "<$key>${xmlEscape(value)}</$key>" } +
                "</u:$action></s:Body></s:Envelope>"
        fun parse(action: String, status: Int, xml: String): Map<String, String> {
            if (status != 200 && status != 500) throw IOException("SOAP HTTP status $status")
            val root = XmlReader(xml).read()
            val body = root.child("Body") ?: throw IOException("Missing SOAP body")
            if (status == 500 || body.child("Fault") != null) {
                val error = body.descendants("UPnPError").firstOrNull() ?: throw IOException("Missing UPnP fault")
                throw SoapFault(action, error.value("errorCode").toIntOrNull() ?: -1, error.value("errorDescription"))
            }
            val response = body.child("${action}Response") ?: throw IOException("Missing SOAP response")
            return response.children.associate { it.name to it.text.trim() }
        }
        fun metadata(title: String, url: String): String =
            "<DIDL-Lite xmlns=\"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:upnp=\"urn:schemas-upnp-org:metadata-1-0/upnp/\">" +
                "<item id=\"openxplay-cast\" parentID=\"-1\" restricted=\"1\"><dc:title>${xmlEscape(title)}</dc:title>" +
                "<upnp:class>object.item.videoItem</upnp:class><res protocolInfo=\"http-get:*:video/mp2t:$DLNA_FEATURES\">${xmlEscape(url)}</res></item></DIDL-Lite>"
    }
}
