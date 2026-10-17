// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.core

import kotlinx.serialization.json.*

public enum class ContextTopic(public val wireName: String) {
    SESSION("session"), NOW_PLAYING("nowPlaying"), CALL("call"), ACTIVITY("activity"), ROUTE("route"), AUDIO_SOURCE("audioSource")
}
public enum class CarPlayState { CONNECTED, DISCONNECTED }
public enum class CallState { IDLE, RINGING, DIALING, ACTIVE, HELD, ENDING }
public data class SessionContext(public val carplay: CarPlayState?)
public data class CallContext(public val state: CallState?)
public data class ActivityContext(public val siri: Boolean?, public val phoneCall: Boolean?, public val navigation: Boolean?)
public data class Artwork(public val url: String?, public val type: String?)
public data class NowPlayingContext(
    public val title: String?, public val artist: String?, public val album: String?,
    public val durationMs: Long?, public val elapsedMs: Long?, public val playing: Boolean?, public val rate: Double?,
    public val artwork: Artwork? = null,
)
public data class RouteManeuver(public val type: Int?, public val road: String?, public val distanceM: Double?)
public data class RouteContext(
    public val active: Boolean, public val destination: String?, public val etaMs: Double?,
    public val distanceRemainingM: Double?, public val maneuver: RouteManeuver?, public val appName: String?,
)
public data class AudioSourceContext(public val appName: String?)
/** Complete topic snapshots; a null value explicitly clears the topic. */
public sealed interface ContextUpdate {
    public val topic: ContextTopic
    public data class Session(public val value: SessionContext?) : ContextUpdate { override val topic: ContextTopic = ContextTopic.SESSION }
    public data class NowPlaying(public val value: NowPlayingContext?) : ContextUpdate { override val topic: ContextTopic = ContextTopic.NOW_PLAYING }
    public data class Call(public val value: CallContext?) : ContextUpdate { override val topic: ContextTopic = ContextTopic.CALL }
    public data class Activity(public val value: ActivityContext?) : ContextUpdate { override val topic: ContextTopic = ContextTopic.ACTIVITY }
    public data class Route(public val value: RouteContext?) : ContextUpdate { override val topic: ContextTopic = ContextTopic.ROUTE }
    public data class AudioSource(public val value: AudioSourceContext?) : ContextUpdate { override val topic: ContextTopic = ContextTopic.AUDIO_SOURCE }
    public companion object {
        public fun idle(): List<ContextUpdate> = listOf(Session(SessionContext(CarPlayState.DISCONNECTED)),
            NowPlaying(null), Call(CallContext(CallState.IDLE)), Activity(ActivityContext(false, false, false)), Route(null), AudioSource(null))
    }
}

/** JSON contract shared by the in-process model and the wire binding. Unknown extension fields are ignored. */
public object ContextJson {
    public fun encode(update: ContextUpdate): JsonObject {
        val value: JsonElement = when (update) {
            is ContextUpdate.AudioSource -> update.value?.let { buildJsonObject { put("appName", JsonPrimitive(it.appName)) } }
            is ContextUpdate.Route -> update.value?.let { r -> buildJsonObject {
                put("active", r.active); put("destination", r.destination)
                put("etaMs", r.etaMs); put("distanceRemainingM", r.distanceRemainingM)
                put("maneuver", r.maneuver?.let { m -> buildJsonObject {
                    put("type", m.type); put("road", m.road); put("distanceM", m.distanceM)
                } } ?: JsonNull)
                put("appName", r.appName)
            } }
            is ContextUpdate.Session -> update.value?.let { buildJsonObject { put("carplay", it.carplay?.name?.lowercase()?.let(::JsonPrimitive) ?: JsonNull) } }
            is ContextUpdate.Call -> update.value?.let { buildJsonObject { put("state", it.state?.name?.lowercase()?.let(::JsonPrimitive) ?: JsonNull) } }
            is ContextUpdate.Activity -> update.value?.let { buildJsonObject { put("siri", JsonPrimitive(it.siri)); put("phoneCall", JsonPrimitive(it.phoneCall)); put("navigation", JsonPrimitive(it.navigation)) } }
            is ContextUpdate.NowPlaying -> update.value?.let { n -> buildJsonObject {
                put("title", JsonPrimitive(n.title)); put("artist", JsonPrimitive(n.artist)); put("album", JsonPrimitive(n.album))
                put("durationMs", JsonPrimitive(n.durationMs)); put("elapsedMs", JsonPrimitive(n.elapsedMs))
                put("playing", JsonPrimitive(n.playing)); put("rate", JsonPrimitive(n.rate))
                n.artwork?.let { a -> put("artwork", buildJsonObject { put("url", JsonPrimitive(a.url)); put("type", JsonPrimitive(a.type)) }) }
            } }
        } ?: JsonNull
        return buildJsonObject { put("topic", update.topic.wireName); put("value", value) }.also { decode(it) }
    }
    public fun decode(update: JsonObject): ContextUpdate {
        val topic = ContextTopic.entries.single { it.wireName == update.getValue("topic").jsonPrimitive.let { p -> require(p.isString); p.content } }
        val value = update.getValue("value")
        if (value == JsonNull) return when (topic) {
            ContextTopic.ROUTE -> ContextUpdate.Route(null); ContextTopic.AUDIO_SOURCE -> ContextUpdate.AudioSource(null)
            ContextTopic.SESSION -> ContextUpdate.Session(null); ContextTopic.NOW_PLAYING -> ContextUpdate.NowPlaying(null)
            ContextTopic.CALL -> ContextUpdate.Call(null); ContextTopic.ACTIVITY -> ContextUpdate.Activity(null)
        }
        val o = value.jsonObject
        return when (topic) {
            ContextTopic.AUDIO_SOURCE -> ContextUpdate.AudioSource(AudioSourceContext(o.text("appName")))
            ContextTopic.ROUTE -> ContextUpdate.Route(RouteContext(
                requireNotNull(o.bool("active")), o.text("destination"),
                o.nonnegative("etaMs"), o.nonnegative("distanceRemainingM"),
                o.getValue("maneuver").takeUnless { it == JsonNull }?.jsonObject?.let { m ->
                    RouteManeuver(m.maneuverType(), m.text("road"), m.nonnegative("distanceM"))
                }, o.text("appName")))
            ContextTopic.SESSION -> ContextUpdate.Session(SessionContext(o.text("carplay")?.let { text -> CarPlayState.entries.single { it.name.lowercase() == text } }))
            ContextTopic.CALL -> ContextUpdate.Call(CallContext(o.text("state")?.let { text -> CallState.entries.single { it.name.lowercase() == text } }))
            ContextTopic.ACTIVITY -> ContextUpdate.Activity(ActivityContext(o.bool("siri"), o.bool("phoneCall"), o.bool("navigation")))
            ContextTopic.NOW_PLAYING -> ContextUpdate.NowPlaying(NowPlayingContext(o.text("title"), o.text("artist"), o.text("album"),
                o.milliseconds("durationMs"), o.milliseconds("elapsedMs"), o.bool("playing"), o.numeric("rate"),
                o["artwork"]?.takeUnless { it == JsonNull }?.jsonObject?.let { Artwork(it.text("url"), it.text("type")) }))
        }
    }
    private fun JsonObject.nonnegative(key: String): Double? = numeric(key)?.also { require(it >= 0.0) }
    private fun JsonObject.maneuverType(): Int? = primitive("type")?.let {
        require(!it.isString); requireNotNull(it.intOrNull).also { n -> require(n in 0..255) }
    }
    private fun JsonObject.primitive(key: String): JsonPrimitive? = getValue(key).takeUnless { it == JsonNull }?.jsonPrimitive
    private fun JsonObject.text(key: String): String? = primitive(key)?.let { require(it.isString); it.content }
    private fun JsonObject.bool(key: String): Boolean? = primitive(key)?.let { require(!it.isString); requireNotNull(it.booleanOrNull) }
    private fun JsonObject.milliseconds(key: String): Long? = primitive(key)?.let {
        require(!it.isString); requireNotNull(it.longOrNull).also { n -> require(n in 0..9007199254740991L) }
    }
    private fun JsonObject.numeric(key: String): Double? = primitive(key)?.let {
        require(!it.isString); requireNotNull(it.doubleOrNull).also { n -> require(n.isFinite()) }
    }
}
