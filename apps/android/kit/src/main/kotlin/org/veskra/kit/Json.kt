package org.veskra.kit

import java.util.Locale

/**
 * Minimal, dependency-free JSON model, encoder and decoder for the engine command boundary
 * (CONTRACTS §8 `nativeExecute`). Deliberately small so :kit has no third-party runtime
 * dependency to audit.
 *
 * Decoding treats input as untrusted: bounded nesting depth and input length, strict grammar
 * (RFC 8259), no trailing data, and typed [JsonParseException] instead of crashes.
 */
sealed class JsonValue {
    /** Canonical compact JSON text. */
    fun encode(): String = StringBuilder().also { JsonWriter.write(this, it) }.toString()

    override fun toString(): String = encode()
}

object JsonNull : JsonValue()

data class JsonBool(val value: Boolean) : JsonValue()

data class JsonString(val value: String) : JsonValue()

/**
 * A JSON number. [raw] keeps the exact source text (or the canonical encoding for numbers built in
 * Kotlin) so integral values beyond 2^53 are not silently rounded.
 */
class JsonNumber private constructor(val raw: String) : JsonValue() {
    val asDouble: Double get() = raw.toDouble()

    /** Exact integer value, or null if the number is fractional or out of Long range. */
    val asLongOrNull: Long?
        get() = raw.toLongOrNull() ?: raw.toDoubleOrNull()?.let { d ->
            if (d.isFinite() && d == Math.floor(d) && d >= Long.MIN_VALUE.toDouble() && d <= Long.MAX_VALUE.toDouble()) {
                d.toLong()
            } else {
                null
            }
        }

    override fun equals(other: Any?): Boolean = other is JsonNumber && other.asDouble == asDouble

    override fun hashCode(): Int = asDouble.hashCode()

    companion object {
        fun of(v: Long): JsonNumber = JsonNumber(v.toString())

        fun of(v: Int): JsonNumber = JsonNumber(v.toString())

        fun of(v: Double): JsonNumber {
            require(v.isFinite()) { "JSON cannot encode non-finite number $v" }
            return if (v == Math.floor(v) && Math.abs(v) < 1e15) JsonNumber(v.toLong().toString()) else JsonNumber(v.toString())
        }

        fun of(v: Float): JsonNumber {
            require(v.isFinite()) { "JSON cannot encode non-finite number $v" }
            // Float.toString gives the shortest repr that round-trips the float, e.g. 0.1f -> "0.1".
            return if (v == Math.floor(v.toDouble()).toFloat() && Math.abs(v) < 1e7f) {
                JsonNumber(v.toLong().toString())
            } else {
                JsonNumber(v.toString())
            }
        }

        internal fun fromValidatedText(raw: String): JsonNumber = JsonNumber(raw)
    }
}

data class JsonArray(val items: List<JsonValue>) : JsonValue() {
    operator fun get(i: Int): JsonValue = items[i]
    val size: Int get() = items.size
}

/** Object with insertion-ordered keys (deterministic encoding). Duplicate keys are rejected on decode. */
data class JsonObject(val fields: LinkedHashMap<String, JsonValue>) : JsonValue() {
    operator fun get(key: String): JsonValue? = fields[key]

    fun string(key: String): String? = (fields[key] as? JsonString)?.value

    fun bool(key: String): Boolean? = (fields[key] as? JsonBool)?.value

    fun long(key: String): Long? = (fields[key] as? JsonNumber)?.asLongOrNull

    fun int(key: String): Int? = long(key)?.takeIf { it in Int.MIN_VALUE..Int.MAX_VALUE }?.toInt()

    fun double(key: String): Double? = (fields[key] as? JsonNumber)?.asDouble

    fun obj(key: String): JsonObject? = fields[key] as? JsonObject

    fun array(key: String): JsonArray? = fields[key] as? JsonArray
}

/** Builder DSL: `json { "cmd" to "layer.add"; "name" to "Ink" }`. */
class JsonObjectBuilder {
    private val map = LinkedHashMap<String, JsonValue>()

    infix fun String.to(v: String?) { map[this] = v?.let(::JsonString) ?: JsonNull }
    infix fun String.to(v: Boolean) { map[this] = JsonBool(v) }
    infix fun String.to(v: Int) { map[this] = JsonNumber.of(v) }
    infix fun String.to(v: Long) { map[this] = JsonNumber.of(v) }
    infix fun String.to(v: Float) { map[this] = JsonNumber.of(v) }
    infix fun String.to(v: Double) { map[this] = JsonNumber.of(v) }
    infix fun String.to(v: JsonValue) { map[this] = v }

    fun build(): JsonObject = JsonObject(LinkedHashMap(map))
}

fun json(block: JsonObjectBuilder.() -> Unit): JsonObject = JsonObjectBuilder().apply(block).build()

class JsonParseException(message: String, val offset: Int) : Exception("$message at offset $offset")

object Json {
    /** Upper bound on accepted input text (engine responses are small; layer lists included). */
    const val MAX_INPUT_CHARS: Int = 8 * 1024 * 1024
    const val MAX_DEPTH: Int = 64

    fun parse(text: String): JsonValue = JsonReader(text).parseDocument()

    fun parseObject(text: String): JsonObject =
        parse(text) as? JsonObject ?: throw JsonParseException("expected a JSON object", 0)
}

internal object JsonWriter {
    fun write(v: JsonValue, out: StringBuilder) {
        when (v) {
            JsonNull -> out.append("null")
            is JsonBool -> out.append(if (v.value) "true" else "false")
            is JsonNumber -> out.append(v.raw)
            is JsonString -> writeString(v.value, out)
            is JsonArray -> {
                out.append('[')
                v.items.forEachIndexed { i, item ->
                    if (i > 0) out.append(',')
                    write(item, out)
                }
                out.append(']')
            }
            is JsonObject -> {
                out.append('{')
                var first = true
                for ((k, item) in v.fields) {
                    if (!first) out.append(',')
                    first = false
                    writeString(k, out)
                    out.append(':')
                    write(item, out)
                }
                out.append('}')
            }
        }
    }

    fun writeString(s: String, out: StringBuilder) {
        out.append('"')
        for (c in s) {
            when (c) {
                '"' -> out.append("\\\"")
                '\\' -> out.append("\\\\")
                '\n' -> out.append("\\n")
                '\r' -> out.append("\\r")
                '\t' -> out.append("\\t")
                '\b' -> out.append("\\b")
                '\u000C' -> out.append("\\f")
                else -> if (c < ' ' || c == ' ' || c == ' ') {
                    out.append("\\u").append(String.format(Locale.ROOT, "%04x", c.code))
                } else {
                    out.append(c)
                }
            }
        }
        out.append('"')
    }
}

private class JsonReader(private val s: String) {
    private var pos = 0

    fun parseDocument(): JsonValue {
        if (s.length > Json.MAX_INPUT_CHARS) throw JsonParseException("input too large", 0)
        skipWs()
        val v = parseValue(0)
        skipWs()
        if (pos != s.length) fail("trailing data")
        return v
    }

    private fun fail(msg: String): Nothing = throw JsonParseException(msg, pos)

    private fun skipWs() {
        while (pos < s.length) {
            val c = s[pos]
            if (c == ' ' || c == '\t' || c == '\n' || c == '\r') pos++ else break
        }
    }

    private fun parseValue(depth: Int): JsonValue {
        if (depth > Json.MAX_DEPTH) fail("nesting too deep")
        if (pos >= s.length) fail("unexpected end of input")
        return when (val c = s[pos]) {
            '{' -> parseObject(depth)
            '[' -> parseArray(depth)
            '"' -> JsonString(parseString())
            't' -> literal("true", JsonBool(true))
            'f' -> literal("false", JsonBool(false))
            'n' -> literal("null", JsonNull)
            else -> if (c == '-' || c in '0'..'9') parseNumber() else fail("unexpected character '$c'")
        }
    }

    private fun literal(word: String, v: JsonValue): JsonValue {
        if (!s.startsWith(word, pos)) fail("invalid literal")
        pos += word.length
        return v
    }

    private fun parseObject(depth: Int): JsonObject {
        pos++ // {
        val map = LinkedHashMap<String, JsonValue>()
        skipWs()
        if (peek() == '}') {
            pos++
            return JsonObject(map)
        }
        while (true) {
            skipWs()
            if (peek() != '"') fail("expected object key")
            val key = parseString()
            skipWs()
            if (peek() != ':') fail("expected ':'")
            pos++
            skipWs()
            if (map.containsKey(key)) fail("duplicate key '$key'")
            map[key] = parseValue(depth + 1)
            skipWs()
            when (peek()) {
                ',' -> pos++
                '}' -> {
                    pos++
                    return JsonObject(map)
                }
                else -> fail("expected ',' or '}'")
            }
        }
    }

    private fun parseArray(depth: Int): JsonArray {
        pos++ // [
        val list = ArrayList<JsonValue>()
        skipWs()
        if (peek() == ']') {
            pos++
            return JsonArray(list)
        }
        while (true) {
            skipWs()
            list.add(parseValue(depth + 1))
            skipWs()
            when (peek()) {
                ',' -> pos++
                ']' -> {
                    pos++
                    return JsonArray(list)
                }
                else -> fail("expected ',' or ']'")
            }
        }
    }

    private fun peek(): Char = if (pos < s.length) s[pos] else fail("unexpected end of input")

    private fun parseString(): String {
        pos++ // opening quote
        val sb = StringBuilder()
        while (true) {
            if (pos >= s.length) fail("unterminated string")
            val c = s[pos++]
            when {
                c == '"' -> return sb.toString()
                c == '\\' -> {
                    if (pos >= s.length) fail("unterminated escape")
                    when (val e = s[pos++]) {
                        '"' -> sb.append('"')
                        '\\' -> sb.append('\\')
                        '/' -> sb.append('/')
                        'b' -> sb.append('\b')
                        'f' -> sb.append('\u000C')
                        'n' -> sb.append('\n')
                        'r' -> sb.append('\r')
                        't' -> sb.append('\t')
                        'u' -> sb.append(parseHex4())
                        else -> fail("invalid escape '\\$e'")
                    }
                }
                c < ' ' -> fail("unescaped control character in string")
                else -> sb.append(c)
            }
        }
    }

    private fun parseHex4(): Char {
        if (pos + 4 > s.length) fail("truncated \\u escape")
        var v = 0
        repeat(4) {
            val d = Character.digit(s[pos++], 16)
            if (d < 0) fail("invalid hex digit in \\u escape")
            v = v * 16 + d
        }
        return v.toChar()
    }

    private fun parseNumber(): JsonNumber {
        val start = pos
        if (s[pos] == '-') pos++
        if (pos >= s.length) fail("truncated number")
        if (s[pos] == '0') {
            pos++
        } else if (s[pos] in '1'..'9') {
            while (pos < s.length && s[pos].isAsciiDigit()) pos++
        } else {
            fail("invalid number")
        }
        if (pos < s.length && s[pos] == '.') {
            pos++
            if (pos >= s.length || !s[pos].isAsciiDigit()) fail("digit expected after '.'")
            while (pos < s.length && s[pos].isAsciiDigit()) pos++
        }
        if (pos < s.length && (s[pos] == 'e' || s[pos] == 'E')) {
            pos++
            if (pos < s.length && (s[pos] == '+' || s[pos] == '-')) pos++
            if (pos >= s.length || !s[pos].isAsciiDigit()) fail("digit expected in exponent")
            while (pos < s.length && s[pos].isAsciiDigit()) pos++
        }
        val raw = s.substring(start, pos)
        if (!raw.toDouble().isFinite()) fail("number out of range")
        return JsonNumber.fromValidatedText(raw)
    }

    private fun Char.isAsciiDigit(): Boolean = this in '0'..'9'
}
