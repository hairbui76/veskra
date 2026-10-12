package org.veskra.kit

/**
 * Path access and copy-on-write updates for [JsonObject] trees. The brush editor edits the
 * engine's preset JSON this way, so fields the app does not know about survive a save unchanged.
 */
fun JsonObject.at(vararg path: String): JsonValue? {
    var cur: JsonValue = this
    for (key in path) cur = (cur as? JsonObject)?.get(key) ?: return null
    return cur
}

fun JsonObject.number(vararg path: String): Float? = (at(*path) as? JsonNumber)?.asDouble?.toFloat()

fun JsonObject.flag(vararg path: String): Boolean? = (at(*path) as? JsonBool)?.value

fun JsonObject.text(vararg path: String): String? = (at(*path) as? JsonString)?.value

/** `[[x, y], …]` control points, or null when the value is not such a list. */
fun JsonObject.points(vararg path: String): List<PointF>? =
    (at(*path) as? JsonArray)?.items?.map { p ->
        val xy = (p as? JsonArray)?.items ?: return null
        val x = (xy.getOrNull(0) as? JsonNumber)?.asDouble ?: return null
        val y = (xy.getOrNull(1) as? JsonNumber)?.asDouble ?: return null
        PointF(x.toFloat(), y.toFloat())
    }

/** A copy with [value] at [path]; missing (or non-object) intermediate nodes become objects. */
fun JsonObject.with(value: JsonValue, vararg path: String): JsonObject {
    require(path.isNotEmpty()) { "empty path" }
    val copy = LinkedHashMap(fields)
    val key = path[0]
    copy[key] = if (path.size == 1) {
        value
    } else {
        ((fields[key] as? JsonObject) ?: JsonObject(LinkedHashMap())).with(value, *path.copyOfRange(1, path.size))
    }
    return JsonObject(copy)
}

fun JsonObject.with(value: Float, vararg path: String): JsonObject = with(JsonNumber.of(value), *path)

fun JsonObject.with(value: Boolean, vararg path: String): JsonObject = with(JsonBool(value), *path)

fun JsonObject.with(value: String?, vararg path: String): JsonObject = with(value?.let(::JsonString) ?: JsonNull, *path)

fun JsonObject.withPoints(points: List<PointF>, vararg path: String): JsonObject =
    with(JsonArray(points.map { JsonArray(listOf(JsonNumber.of(it.x), JsonNumber.of(it.y))) }), *path)
