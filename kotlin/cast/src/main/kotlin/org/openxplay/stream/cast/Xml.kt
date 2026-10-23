// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

package org.openxplay.stream.cast

internal data class XmlNode(val name: String, val attributes: Map<String, String>, val children: List<XmlNode>, val text: String) {
    fun child(name: String): XmlNode? = children.firstOrNull { it.name == name }
    fun value(name: String): String = child(name)?.text?.trim().orEmpty()
    fun descendants(name: String): List<XmlNode> = children.flatMap { (if (it.name == name) listOf(it) else emptyList()) + it.descendants(name) }
}

internal fun xmlEscape(value: String): String = buildString {
    for (c in value) append(when (c) { '&' -> "&amp;"; '<' -> "&lt;"; '>' -> "&gt;"; '"' -> "&quot;"; '\'' -> "&apos;"; else -> c.toString() })
}

/** Deliberately has no DTD, external resources, validation or entity declarations. */
internal class XmlReader(source: String) {
    private val source = source.trimEnd { it == '\u0000' || it in " \r\n\t" }
    private var at = 0
    fun read(): XmlNode {
        require(!source.contains("<!DOCTYPE", true) && !source.contains("<!ENTITY", true)) { "XML declarations forbidden" }
        var index = 0
        while (index < source.length) {
            val code = source.codePointAt(index)
            require(character(code)) { "Invalid XML character" }; index += Character.charCount(code)
        }
        if (source.startsWith('\uFEFF')) at++
        misc()
        val node = element(0)
        misc()
        require(at == source.length) { "Trailing XML" }
        return node
    }
    private fun starts(value: String): Boolean = source.startsWith(value, at)
    private fun space() { while (at < source.length && source[at] in " \r\n\t") at++ }
    private fun skip(end: String) {
        val to = source.indexOf(end, at)
        require(to >= 0) { "Unterminated XML" }; at = to + end.length
    }
    private fun misc() {
        while (true) {
            space()
            when { starts("<!--") -> { at += 4; comment() }; starts("<?") -> { at += 2; skip("?>") }; else -> return }
        }
    }
    private fun comment() {
        val to = source.indexOf("-->", at)
        require(to >= 0 && !source.substring(at, to).contains("--")) { "Invalid XML comment" }
        at = to + 3
    }
    private fun expect(value: String) { require(starts(value)) { "Expected XML $value" }; at += value.length }
    private fun name(): String {
        val start = at
        require(at < source.length && (source[at].isLetter() || source[at] in "_:")) { "Invalid XML name" }
        at++
        while (at < source.length && (source[at].isLetterOrDigit() || source[at] in "_:.-")) at++
        return source.substring(start, at)
    }
    private fun element(depth: Int): XmlNode {
        require(depth < 64) { "XML nesting limit" }
        expect("<"); val tag = name()
        val attributes = linkedMapOf<String, String>()
        val names = mutableSetOf<String>()
        while (true) {
            val before = at; space()
            if (starts("/>") || starts(">")) break
            require(at > before) { "Missing XML attribute separator" }
            val key = name(); require(names.add(key)) { "Duplicate XML attribute" }
            space(); expect("="); space()
            require(at < source.length && source[at] in "\"'") { "Unquoted XML attribute" }
            val quote = source[at++]; val end = source.indexOf(quote, at)
            require(end >= 0 && '<' !in source.substring(at, end)) { "Invalid XML attribute" }
            attributes[key.substringAfter(':')] = entities(source.substring(at, end)); at = end + 1
        }
        if (starts("/>")) { at += 2; return XmlNode(tag.substringAfter(':'), attributes, emptyList(), "") }
        expect(">")
        val children = mutableListOf<XmlNode>(); val text = StringBuilder()
        while (!starts("</")) {
            require(at < source.length) { "Unclosed XML element" }
            when {
                starts("<!--") -> { at += 4; comment() }
                starts("<?") -> { at += 2; skip("?>") }
                starts("<![CDATA[") -> {
                    at += 9; val end = source.indexOf("]]>", at); require(end >= 0) { "Unclosed CDATA" }
                    text.append(source.substring(at, end)); at = end + 3
                }
                starts("<") -> { val child = element(depth + 1); children += child; text.append(child.text) }
                else -> {
                    val end = source.indexOf('<', at).let { if (it < 0) source.length else it }
                    val raw = source.substring(at, end); require(!raw.contains("]]>")) { "Invalid XML text" }
                    text.append(entities(raw)); at = end
                }
            }
        }
        expect("</"); require(name() == tag) { "Mismatched XML end tag" }; space(); expect(">")
        return XmlNode(tag.substringAfter(':'), attributes, children, text.toString())
    }
    private fun character(code: Int): Boolean = code in 0x20..0xd7ff || code in 0xe000..0xfffd || code in 0x10000..0x10ffff || code in listOf(9, 10, 13)
    private fun entities(value: String): String = buildString {
        var i = 0
        while (i < value.length) {
            if (value[i] != '&') { val c = value[i++]; require(c.code >= 32 || c in "\r\n\t"); append(c); continue }
            val end = value.indexOf(';', i); require(end >= 0) { "Unclosed XML entity" }
            val entity = value.substring(i + 1, end)
            when (entity) {
                "amp" -> append('&'); "lt" -> append('<'); "gt" -> append('>'); "quot" -> append('"'); "apos" -> append('\'')
                else -> {
                    if (entity.startsWith('#')) {
                        val digits = entity.drop(if (entity.startsWith("#x")) 2 else 1)
                        val radix = if (entity.startsWith("#x")) 16 else 10
                        require(digits.isNotEmpty() && digits.all { it.digitToIntOrNull(radix) != null }) { "Invalid XML reference" }
                        val code = digits.toIntOrNull(radix)
                        require(code != null && character(code)) { "Invalid XML character" }
                        append(String(Character.toChars(code)))
                    } else {
                        require(entity.isNotEmpty() && (entity.first().isLetter() || entity.first() in "_:") &&
                            entity.all { it.isLetterOrDigit() || it in "_:.-" }) { "Invalid XML entity name" }
                        append('&').append(entity).append(';')
                    }
                }
            }
            i = end + 1
        }
    }
}
