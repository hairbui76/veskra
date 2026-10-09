package org.veskra.kit

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertThrows
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.params.ParameterizedTest
import org.junit.jupiter.params.provider.ValueSource

class JsonTest {
    @Test
    fun `encodes the document create command exactly as documented`() {
        val cmd = Commands.documentCreate(2048, 2048, 300, "#FFFFFF")
        assertEquals("""{"cmd":"document.create","width":2048,"height":2048,"dpi":300,"background":"#FFFFFF"}""", cmd.encode())
    }

    @Test
    fun `escapes strings and round trips unicode`() {
        val s = "quote\" back\\ nl\n tab\t ctl\u0001 snow\u2603 emoji\uD83C\uDFA8 ls\u2028"
        val encoded = json { "s" to s }.encode()
        assertTrue(encoded.contains("\\u0001"))
        assertTrue(encoded.contains("\\u2028"))
        assertEquals(s, Json.parseObject(encoded).string("s"))
    }

    @Test
    fun `parses nested structures and numbers`() {
        val o = Json.parseObject("""{"a":[1,-2.5,3e2,true,false,null,{"b":"c"}],"big":9007199254740993}""")
        val a = o.array("a")!!
        assertEquals(1L, (a[0] as JsonNumber).asLongOrNull)
        assertEquals(-2.5, (a[1] as JsonNumber).asDouble)
        assertEquals(300L, (a[2] as JsonNumber).asLongOrNull)
        assertEquals(JsonBool(true), a[3])
        assertEquals(JsonNull, a[5])
        assertEquals("c", (a[6] as JsonObject).string("b"))
        // Exact beyond 2^53: raw text preserved.
        assertEquals(9007199254740993L, o.long("big"))
    }

    @Test
    fun `fractional numbers are not longs and ints reject overflow`() {
        val o = Json.parseObject("""{"f":1.5,"l":4294967296}""")
        assertNull(o.long("f"))
        assertNull(o.int("l"))
        assertEquals(4294967296L, o.long("l"))
    }

    @Test
    fun `float encoding is shortest round trip`() {
        assertEquals("""{"o":0.1,"i":3}""", json { "o" to 0.1f; "i" to 3.0f }.encode())
    }

    @Test
    fun `non finite numbers cannot be encoded`() {
        assertThrows(IllegalArgumentException::class.java) { json { "x" to Float.NaN } }
        assertThrows(IllegalArgumentException::class.java) { json { "x" to Double.POSITIVE_INFINITY } }
    }

    @ParameterizedTest
    @ValueSource(
        strings = [
            "", "{", "}", "[1,]", "{\"a\":1,}", "{\"a\" 1}", "01", "1.", "-", "1e", "+1", "tru", "nul",
            "\"unterminated", "\"bad \\x escape\"", "\"\\u12\"", "{\"a\":1} trailing", "{\"a\":1,\"a\":2}",
            "\"raw\u0001control\"", "NaN", "1e999",
        ],
    )
    fun `rejects malformed input with a typed exception`(text: String) {
        assertThrows(JsonParseException::class.java) { Json.parse(text) }
    }

    @Test
    fun `rejects excessive nesting`() {
        val deep = "[".repeat(Json.MAX_DEPTH + 2) + "]".repeat(Json.MAX_DEPTH + 2)
        assertThrows(JsonParseException::class.java) { Json.parse(deep) }
        val ok = "[".repeat(Json.MAX_DEPTH) + "]".repeat(Json.MAX_DEPTH)
        Json.parse(ok)
    }

    @Test
    fun `whitespace and empty containers`() {
        val o = Json.parseObject(" \n{ \"a\" : [ ] , \"b\" : { } }\t")
        assertEquals(0, o.array("a")!!.size)
        assertEquals(0, o.obj("b")!!.fields.size)
    }
}
