// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

package org.openxplay.stream.cast

import org.junit.jupiter.api.Test
import kotlin.test.*

class XmlReaderTest {
    @Test fun nestingAttributesEntitiesCdataAndNamespaces() {
        val xml = "<?xml version='1.0'?><!--before--><x:root xmlns:x='urn:test' a='&quot;&apos;&amp;&lt;&gt;&#65;&#x1F600;'>" +
            "<x:item id=\"2\">one<![CDATA[ <&> ]]><!--skip--><?skip this?><b>two</b></x:item><empty/></x:root>"
        val root = XmlReader(xml).read()
        assertEquals("root", root.name)
        assertEquals("\"'&<>A😀", root.attributes["a"])
        assertEquals("2", root.child("item")!!.attributes["id"])
        assertEquals("one <&> two", root.child("item")!!.text)
        assertEquals("two", root.descendants("b").single().text)
        assertEquals("", root.child("empty")!!.text)
        assertEquals("&lt;&amp;&gt;&quot;&apos;", xmlEscape("<&>\"'"))
    }
    @Test fun rejectsDeclarationsAndMalformedInput() {
        for (xml in listOf("<!DOCTYPE r><r/>", "<!ENTITY x 'y'><r/>", "<!DOCTYPE r [<!ENTITY x SYSTEM 'file:///secret'>]><r>&x;</r>",
            "<r>", "<r></x>", "<r/ >", "<r a='1' a='2'/>", "<r a='1'b='2'/>", "<r>&;</r>", "<r>&bad name;</r>", "<r>&#0;</r>",
            "<r>&#xD800;</r>", "<r>&#+65;</r>", "<r><![CDATA[\u0000]]></r>", "<r>&#1114112;</r>", "<r a='<'/>", "<r><![CDATA[broken</r>", "<r/><!--unclosed", "<r/>garbage",
            "<r>bad]]></r>", "<r><!--a--b--></r>", "<r>".repeat(70) + "</r>".repeat(70))) {
            assertFailsWith<IllegalArgumentException>(xml) { XmlReader(xml).read() }
        }
    }
    @Test fun unknownNamedEntitiesRemainLiteralInTextAndAttributes() {
        val root = XmlReader("<r a='A&nbsp;&amp;'>B&vendor; &lt; &#65;</r>").read()
        assertEquals("A&nbsp;&", root.attributes["a"])
        assertEquals("B&vendor; < A", root.text)
    }
    @Test fun ignoresOnlyTrailingWhitespaceAndNulsAfterRoot() {
        for (tail in listOf("", " \t\r\n", "\u0000", "\u0000 \t\r\n\u0000"))
            assertEquals("value", XmlReader("<r>value</r>$tail").read().text)
        for (xml in listOf("<r>\u0000</r>", "<r/>\u0000garbage", "<r/>\u0000<!DOCTYPE r>", "<r/>\u0000<!ENTITY x 'y'>"))
            assertFailsWith<IllegalArgumentException> { XmlReader(xml).read() }
    }
}
