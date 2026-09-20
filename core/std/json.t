# JSON (STDLIB-SERIALIZE §1-§4).
#
# Two entrances, and the writer is the one to reach for first:
# `JsonWriter` appends to a string as you go, so writing a struct out
# costs one allocation that grows, not a tree of `Json` values built
# only to be walked once. The tree exists for the reader's sake, and
# `Json::to_string` writes through the same writer so there is one
# implementation of the spelling.
#
# Reading is `json::parse(s) -> Result<Json, JsonError>`, or
# `Json::read` when the document to read into is one you already
# hold. Every step of the reader answers a `Result`, so a failure
# says where it stopped without a second call to ask.
#
# **Numbers are `Int(i64)` or `Num(f64)`, never both.** JSON has one
# number type, but a `u64` identifier put through an `f64` stops
# being itself above 2^53 -- `1234567890123456789` reads back as
# `...768`. So an integer that fits stays an integer. Values at or
# above 2^63 do fall to `Num` and lose precision; adding a third case
# for them would mean every `match` has three arms forever.
#
# **No exponent form on output.** It is legal JSON, but a writer with
# two spellings writes the same value two ways, so this one always
# writes the shortest form that reads back as the same f64. A very
# large or very small number is therefore long: 1e30 comes out as
# 31 digits.
#
# Names are qualified with `json::` throughout, because a bare name
# in a stdlib body can resolve to a user function
# (STDLIB-FN-SHADOWED-BY-USER-FN).

# How far the reader will nest before giving up.
#
# A recursive-descent reader recurses as deep as its input nests, and
# without a limit a deeply nested document is a stack overflow -- a
# crash, from inside a function whose whole job is to return `Result`
# instead of crashing.
#
# **32, not the 128 the design asked for**, because the limit has to
# be one this runtime can actually enforce: a nested document runs
# out of host stack somewhere between 40 and 60 levels (measured on
# the IR VM -- fine at 40, `fatal runtime error: stack overflow` at
# 60), and a limit above that is a limit that never fires. Real
# configuration files nest a handful of levels; a document 32 deep is
# generated, and generated documents are exactly the ones that arrive
# 10000 deep.
#
# **On a small stack even 32 is too many.** A walk on a thread with a
# 2 MiB stack (a test thread, say) runs out at about ten levels, so
# the limit protects the program a user runs and nothing here can
# protect that thread. That is a property of the tree-walker -- one
# large host frame per toylang call -- not of this reader.
pub fn max_depth() -> u64 { 32u64 }

pub enum JsonError {
    # There was nothing but whitespace.
    Empty,
    # Byte offset of the first character that cannot be there.
    Invalid(u64),
    # A complete value was read, and then something followed it.
    Trailing(u64),
    # Nesting passed `max_depth()`, at this offset.
    TooDeep(u64),
}

impl Display for JsonError {
    fn to_str(&self) -> str {
        match self {
            JsonError::Empty => "empty input",
            JsonError::Invalid(at) => "invalid JSON at byte {at}",
            JsonError::Trailing(at) => "trailing content at byte {at}",
            JsonError::TooDeep(at) => "nested too deeply at byte {at}",
        }
    }
}

# ---------------------------------------------------------------------
# The writer (S1).

# Appends JSON to a string.
#
# **It does not check that the calls make a document.** `key` inside
# an array, or `end_object` closing an array, produces text that is
# not JSON; the state kept here is only what is needed to place
# commas. Checking the shape would mean a stack of open containers,
# which is the tree this type exists to avoid.
pub struct JsonWriter {
    out: String,
    # Whether a comma is owed before the next thing written.
    need_comma: bool,
}

impl JsonWriter {
    fn new() -> Self {
        JsonWriter { out: String::new(), need_comma: false }
    }

    # `"` and `\` and everything below 0x20, and nothing else.
    #
    # The rest of the UTF-8 goes through as it is: escaping it to
    # `\uXXXX` is legal but makes a Japanese document about six times
    # larger, for no reader's benefit.
    #
    # A method rather than a free function taking `&mut String`,
    # because `&mut self.out` is not a borrow this language allows.
    fn write_escaped(&mut self, s: str) {
    val text: String = String::from_str(s)
    val n: u64 = text.size()
    self.out.push('"')
    var i: u64 = 0u64
    while i < n {
        val b: u8 = text.get(i)
        val c: u64 = b as u64
        # Written as byte pushes rather than as literals: a string
        # literal in this language cannot yet contain an escaped
        # backslash or quote, which is exactly what this function
        # emits.
        #
        # `'\x08'` / `'\x0c'` are backspace and form feed, which JSON
        # spells `\b` and `\f`. The language's char escapes are
        # `\n \t \r \0 \\ \' \"`, so those two arrive by code point --
        # the letter pushed next to each says which is which.
        if c == '"' {
            self.out.push('\\')
            self.out.push('"')
        } elif c == '\\' {
            self.out.push('\\')
            self.out.push('\\')
        } elif c == '\n' {
            self.out.push('\\')
            self.out.push('n')
        } elif c == '\r' {
            self.out.push('\\')
            self.out.push('r')
        } elif c == '\t' {
            self.out.push('\\')
            self.out.push('t')
        } elif c == '\x08' {
            self.out.push('\\')
            self.out.push('b')
        } elif c == '\x0c' {
            self.out.push('\\')
            self.out.push('f')
        } elif c < 32u64 {
            # Every other control character as `\u00XX`.
            self.out.push('\\')
            self.out.push('u')
            self.out.push('0')
            self.out.push('0')
            self.out.push(json::hex_digit(c / 16u64))
            self.out.push(json::hex_digit(c % 16u64))
        } else {
            self.out.push(b)
        }
        i = i + 1u64
    }
    self.out.push('"')
}

    fn separate(&mut self) {
        if self.need_comma { self.out.push(',') }
    }

    fn begin_object(&mut self) {
        self.separate()
        self.out.push('{')
        self.need_comma = false
    }

    fn end_object(&mut self) {
        self.out.push('}')
        self.need_comma = true
    }

    fn begin_array(&mut self) {
        self.separate()
        self.out.push('[')
        self.need_comma = false
    }

    fn end_array(&mut self) {
        self.out.push(']')
        self.need_comma = true
    }

    # A member name. The value that follows it is written by the next
    # call, and owes no comma of its own.
    fn key(&mut self, k: str) {
        self.separate()
        self.write_escaped(k)
        self.out.push(':')
        self.need_comma = false
    }

    fn str_value(&mut self, v: str) {
        self.separate()
        self.write_escaped(v)
        self.need_comma = true
    }

    fn u64_value(&mut self, v: u64) {
        self.separate()
        self.out.push_str(__builtin_to_string(v))
        self.need_comma = true
    }

    fn i64_value(&mut self, v: i64) {
        self.separate()
        self.out.push_str(__builtin_to_string(v))
        self.need_comma = true
    }

    # **Panics on NaN and on either infinity.** JSON has no spelling
    # for them, and the alternatives are worse: writing `null` puts a
    # different value in the document than the one that was passed,
    # and writing `NaN` produces something no other reader accepts.
    # A caller who wants `null` there branches on `math::is_finite`.
    fn f64_value(&mut self, v: f64) {
        if math::is_nan(v) { panic("JSON has no NaN") }
        if math::is_infinite(v) { panic("JSON has no infinity") }
        self.separate()
        self.out.push_str(__builtin_to_string(v))
        self.need_comma = true
    }

    fn bool_value(&mut self, v: bool) {
        self.separate()
        if v { self.out.push_str("true") } else { self.out.push_str("false") }
        self.need_comma = true
    }

    fn null_value(&mut self) {
        self.separate()
        self.out.push_str("null")
        self.need_comma = true
    }

    # The document, taking the writer with it.
    fn finish(self: Self) -> String { self.out }
}

fn hex_digit(v: u64) -> u8 {
    if v < 10u64 { ('0' + v) as u8 } else { ('a' + (v - 10u64)) as u8 }
}

# ---------------------------------------------------------------------
# The tree (S3).

# What a node is.
pub enum JsonKind {
    Null,
    Bool,
    Int,
    Num,
    Text,
    Array,
    Object,
}

# One node of a document.
#
# **The tree is flat.** The obvious spelling -- an `enum Json` whose
# `Array` variant holds `Vec<Json>` -- type-checks and runs on the
# tree-walker, but a value that size cannot be passed to a function
# in the compiled lanes at all (`compiler MVP cannot lower parameter
# `t: Json``, and the recursive walk that reads it is nothing but
# parameter passing). So a document is one `Vec` of nodes in
# pre-order, and a child is an index.
#
# `next` is the index just past this node's subtree, which is how a
# walk finds the following sibling without a child list. An object's
# children alternate: key node, value node, key node, ... so a member
# needs no field of its own.
struct JsonNode {
    kind: u64,
    next: u64,
    # The `Int` value, or 0/1 for `Bool`.
    int: i64,
    num: f64,
    text: String,
}

pub struct Json {
    nodes: Vec<JsonNode>,
}

fn kind_null() -> u64 { 0u64 }
fn kind_bool() -> u64 { 1u64 }
fn kind_int() -> u64 { 2u64 }
fn kind_num() -> u64 { 3u64 }
fn kind_text() -> u64 { 4u64 }
fn kind_array() -> u64 { 5u64 }
fn kind_object() -> u64 { 6u64 }

impl Json {
    fn new() -> Self { Json { nodes: Vec::new() } }

    # The index of the whole document's value. Always 0 for a
    # document that parsed.
    fn root(&self) -> u64 { 0u64 }

    fn size(&self) -> u64 { self.nodes.size() }

    fn kind(&self, id: u64) -> JsonKind {
        val n = self.nodes.borrow(id)
        if n.kind == 0u64 {
            JsonKind::Null
        } elif n.kind == 1u64 {
            JsonKind::Bool
        } elif n.kind == 2u64 {
            JsonKind::Int
        } elif n.kind == 3u64 {
            JsonKind::Num
        } elif n.kind == 4u64 {
            JsonKind::Text
        } elif n.kind == 5u64 {
            JsonKind::Array
        } else {
            JsonKind::Object
        }
    }

    fn as_bool(&self, id: u64) -> bool {
        val n = self.nodes.borrow(id)
        n.int != 0i64
    }

    fn as_int(&self, id: u64) -> i64 {
        val n = self.nodes.borrow(id)
        n.int
    }

    # An `Int` read as an `f64`, so a caller that wants a number need
    # not care which of the two it got. Above 2^53 this is lossy, and
    # that is the reason the two kinds exist.
    fn as_num(&self, id: u64) -> f64 {
        val n = self.nodes.borrow(id)
        if n.kind == 2u64 { n.int as f64 } else { n.num }
    }

    fn as_text(&self, id: u64) -> str {
        val n = self.nodes.borrow(id)
        val t: str = n.text.to_str()
        t
    }

    # Elements of an array, or members of an object.
    fn len(&self, id: u64) -> u64 {
        val n = self.nodes.borrow(id)
        var count: u64 = 0u64
        var child: u64 = id + 1u64
        while child < n.next {
            val c = self.nodes.borrow(child)
            child = c.next
            count = count + 1u64
        }
        if n.kind == 6u64 { count / 2u64 } else { count }
    }

    # The `i`th child node of an array, or the `i`th key node of an
    # object. Out of range is a panic, like `Vec::get`.
    fn child(&self, id: u64, i: u64) -> u64 {
        val n = self.nodes.borrow(id)
        val step: u64 = if n.kind == 6u64 { 2u64 } else { 1u64 }
        var seen: u64 = 0u64
        var child: u64 = id + 1u64
        while child < n.next {
            if seen == i * step { return child }
            val c = self.nodes.borrow(child)
            child = c.next
            seen = seen + 1u64
        }
        panic("Json::child index out of bounds")
    }

    fn key_at(&self, id: u64, i: u64) -> str {
        val k: u64 = self.child(id, i)
        val t: str = self.as_text(k)
        t
    }

    fn value_at(&self, id: u64, i: u64) -> u64 {
        val k: u64 = self.child(id, i)
        val n = self.nodes.borrow(k)
        n.next
    }

    # The value for a member name, or `None`.
    #
    # A duplicate name is the last one written, which is what the
    # reader stores and what a `Dict` would have done.
    fn get(&self, id: u64, key: str) -> Option<u64> {
        val count: u64 = self.len(id)
        var found: Option<u64> = Option::None
        var i: u64 = 0u64
        while i < count {
            val k: str = self.key_at(id, i)
            if k == key { found = Option::Some(self.value_at(id, i)) }
            i = i + 1u64
        }
        found
    }

    # The document as JSON text.
    #
    # Written through `JsonWriter` -- including one throwaway writer
    # per string, to escape it -- so there is exactly one place that
    # decides how a value is spelled.
    fn to_string(&self) -> String {
        val s: String = self.node_to_string(0u64)
        s
    }

    fn node_to_string(&self, id: u64) -> String {
        val n = self.nodes.borrow(id)
        var out: String = String::new()
        if n.kind == 0u64 {
            out.push_str("null")
        } elif n.kind == 1u64 {
            if n.int != 0i64 { out.push_str("true") } else { out.push_str("false") }
        } elif n.kind == 2u64 {
            out.push_str(__builtin_to_string(n.int))
        } elif n.kind == 3u64 {
            var w: JsonWriter = JsonWriter::new()
            w.f64_value(n.num)
            val part: String = w.finish()
            out.push_string(&part)
        } elif n.kind == 4u64 {
            var w: JsonWriter = JsonWriter::new()
            val t: str = n.text.to_str()
            w.str_value(t)
            val part: String = w.finish()
            out.push_string(&part)
        } else {
            val is_object: bool = n.kind == 6u64
            # Char literals, not string literals: `"{"` alone
            # would start an interpolation rather than name a brace.
            if is_object { out.push('{') } else { out.push('[') }
            var child: u64 = id + 1u64
            var index: u64 = 0u64
            while child < n.next {
                if index > 0u64 {
                    # In an object the separator alternates: a colon
                    # after a key, a comma after a value.
                    if is_object && index % 2u64 == 1u64 {
                        out.push_str(":")
                    } else {
                        out.push_str(",")
                    }
                }
                val part: String = self.node_to_string(child)
                out.push_string(&part)
                val c = self.nodes.borrow(child)
                child = c.next
                index = index + 1u64
            }
            if is_object { out.push('}') } else { out.push(']') }
        }
        out
    }
}

# ---------------------------------------------------------------------
# The reader (S4 / S5).
#
# RFC 8259 and no extensions: no trailing comma, no comment, no
# `NaN`, no leading zero, no single quote. Each of those is a place
# where a document that means something to one reader means nothing
# to another, and this one is on the strict side by design -- the
# same choice `parse::to_u64` makes.
#
# **Every step answers `Result<u64, JsonError>`** -- the position
# just past what it read, or why it stopped -- so a failure travels
# out on the return, and `?` carries a nested one up without the
# caller restating it.

fn is_digit(b: u64) -> bool { b >= '0' && b <= '9' }

impl Json {
    # Two constructors rather than one, because a compound-returning
    # call cannot sit in an argument in the compiled lanes: the empty
    # `String` a non-text node carries has to be bound first, and
    # binding it here keeps that out of every call site.
    fn push_node(&mut self, kind: u64, int: i64, num: f64) -> u64 {
        val id: u64 = self.nodes.size()
        val empty: String = String::new()
        self.nodes.push(JsonNode { kind: kind, next: 0u64, int: int, num: num, text: empty })
        id
    }

    fn push_text_node(&mut self, text: String) -> u64 {
        val id: u64 = self.nodes.size()
        self.nodes.push(JsonNode { kind: json::kind_text(), next: 0u64, int: 0i64, num: 0f64, text: text })
        id
    }

    # A node with no children ends right after itself.
    fn close_leaf(&mut self, id: u64) {
        var n: JsonNode = self.nodes.get(id)
        n.next = id + 1u64
        self.nodes.set(id, n)
    }

    # A container ends wherever the document has grown to.
    fn close_node(&mut self, id: u64) {
        var n: JsonNode = self.nodes.get(id)
        n.next = self.nodes.size()
        self.nodes.set(id, n)
    }

    # These take the input by reference and answer with a scalar, and
    # they were made methods because a module-level free function
    # with that shape -- a `&`-compound parameter and a scalar
    # return -- did not lower in the compiled lanes at the time
    # (`call argument produced no value`; MODULE-FN-REF-ARG). **That
    # is fixed** (the module-call path passes the callee, so a `&T`
    # parameter receives an address); they stay methods because
    # nothing is gained by moving them back.

    # Whitespace is the four characters JSON names, and no others.
    fn skip_ws(&self, text: &String, pos: u64) -> u64 {
        val n: u64 = text.size()
        var p: u64 = pos
        var scanning: bool = true
        while scanning && p < n {
            val b: u64 = text.get(p) as u64
            if b == ' ' || b == '\t' || b == '\n' || b == '\r' {
                p = p + 1u64
            } else {
                scanning = false
            }
        }
        p
    }

    # The byte at `pos`, or 256 past the end -- a value no byte has,
    # so callers can compare without a separate bounds test.
    fn byte_at(&self, text: &String, pos: u64) -> u64 {
        if pos >= text.size() { 256u64 } else { text.get(pos) as u64 }
    }

    # The value of four hex digits at `pos`, or -1 if they are not
    # four hex digits.
    fn hex4(&self, text: &String, pos: u64) -> i64 {
        if pos + 4u64 > text.size() { return -1i64 }
        var v: i64 = 0i64
        var i: u64 = 0u64
        while i < 4u64 {
            val b: u64 = text.get(pos + i) as u64
            var d: i64 = -1i64
            if b >= '0' && b <= '9' {
                d = (b - '0') as i64
            } elif b >= 'a' && b <= 'f' {
                d = (b - 'a' + 10u64) as i64
            } elif b >= 'A' && b <= 'F' {
                d = (b - 'A' + 10u64) as i64
            }
            if d < 0i64 { return -1i64 }
            v = v * 16i64 + d
            i = i + 1u64
        }
        v
    }

    # Does this literal word sit at `pos`?
    fn word_at(&self, text: &String, pos: u64, word: str) -> bool {
        val w: String = String::from_str(word)
        val n: u64 = w.size()
        if pos + n > text.size() { return false }
        var i: u64 = 0u64
        var same: bool = true
        while same && i < n {
            if text.get(pos + i) != w.get(i) { same = false }
            i = i + 1u64
        }
        same
    }

    # Read one value at `pos`, appending its nodes, and answer with
    # the position just after it.
    fn read_value(&mut self, text: &String, pos: u64, depth: u64) -> Result<u64, JsonError> {
        if depth > json::max_depth() {
            return Result::Err(JsonError::TooDeep(pos))
        }
        val p: u64 = self.skip_ws(text, pos)
        val b: u64 = self.byte_at(text, p)
        if b == '{' {
            val r = self.read_object(text, p, depth)
            r
        } elif b == '[' {
            val r = self.read_array(text, p, depth)
            r
        } elif b == '"' {
            val r = self.read_text(text, p)
            r
        } elif b == 't' {
            if self.word_at(text, p, "true") {
                val id: u64 = self.push_node(json::kind_bool(), 1i64, 0f64)
                self.close_leaf(id)
                Result::Ok(p + 4u64)
            } else {
                Result::Err(JsonError::Invalid(p))
            }
        } elif b == 'f' {
            if self.word_at(text, p, "false") {
                val id: u64 = self.push_node(json::kind_bool(), 0i64, 0f64)
                self.close_leaf(id)
                Result::Ok(p + 5u64)
            } else {
                Result::Err(JsonError::Invalid(p))
            }
        } elif b == 'n' {
            if self.word_at(text, p, "null") {
                val id: u64 = self.push_node(json::kind_null(), 0i64, 0f64)
                self.close_leaf(id)
                Result::Ok(p + 4u64)
            } else {
                Result::Err(JsonError::Invalid(p))
            }
        } elif b == '-' || json::is_digit(b) {
            val r = self.read_number(text, p)
            r
        } else {
            Result::Err(JsonError::Invalid(p))
        }
    }

    fn read_array(&mut self, text: &String, pos: u64, depth: u64) -> Result<u64, JsonError> {
        val id: u64 = self.push_node(json::kind_array(), 0i64, 0f64)
        var p: u64 = self.skip_ws(text, pos + 1u64)
        if self.byte_at(text, p) == ']' {
            self.close_node(id)
            return Result::Ok(p + 1u64)
        }
        var done: bool = false
        while !done {
            val after: u64 = self.read_value(text, p, depth + 1u64)?
            p = self.skip_ws(text, after)
            val b: u64 = self.byte_at(text, p)
            if b == ',' {
                p = p + 1u64
            } elif b == ']' {
                p = p + 1u64
                done = true
            } else {
                return Result::Err(JsonError::Invalid(p))
            }
        }
        self.close_node(id)
        Result::Ok(p)
    }

    fn read_object(&mut self, text: &String, pos: u64, depth: u64) -> Result<u64, JsonError> {
        val id: u64 = self.push_node(json::kind_object(), 0i64, 0f64)
        var p: u64 = self.skip_ws(text, pos + 1u64)
        if self.byte_at(text, p) == '}' {
            self.close_node(id)
            return Result::Ok(p + 1u64)
        }
        var done: bool = false
        while !done {
            # The key is a text node like any other: an object's
            # children alternate key, value, key, value.
            if self.byte_at(text, p) != '"' {
                return Result::Err(JsonError::Invalid(p))
            }
            val after_key: u64 = self.read_text(text, p)?
            p = self.skip_ws(text, after_key)
            if self.byte_at(text, p) != ':' {
                return Result::Err(JsonError::Invalid(p))
            }
            p = p + 1u64
            val after_value: u64 = self.read_value(text, p, depth + 1u64)?
            p = self.skip_ws(text, after_value)
            val b: u64 = self.byte_at(text, p)
            if b == ',' {
                p = self.skip_ws(text, p + 1u64)
            } elif b == '}' {
                p = p + 1u64
                done = true
            } else {
                return Result::Err(JsonError::Invalid(p))
            }
        }
        self.close_node(id)
        Result::Ok(p)
    }

    # A quoted string, escapes decoded.
    #
    # `\uXXXX` is accepted, and a surrogate pair is put back together
    # into one codepoint before it is encoded -- so an emoji written
    # as two escapes is one character, and a lone half is `Invalid`.
    fn read_text(&mut self, text: &String, pos: u64) -> Result<u64, JsonError> {
        var out: String = String::new()
        var p: u64 = pos + 1u64
        var done: bool = false
        while !done {
            val b: u64 = self.byte_at(text, p)
            if b == 256u64 {
                return Result::Err(JsonError::Invalid(p))
            } elif b == '"' {
                p = p + 1u64
                done = true
            } elif b < 32u64 {
                # A raw control character has to be escaped; only its
                # escape is legal here.
                return Result::Err(JsonError::Invalid(p))
            } elif b == '\\' {
                val e: u64 = self.byte_at(text, p + 1u64)
                if e == '"' {
                    out.push('"')
                    p = p + 2u64
                } elif e == '\\' {
                    out.push('\\')
                    p = p + 2u64
                } elif e == '/' {
                    out.push('/')
                    p = p + 2u64
                } elif e == 'b' {
                    out.push('\x08')
                    p = p + 2u64
                } elif e == 'f' {
                    out.push('\x0c')
                    p = p + 2u64
                } elif e == 'n' {
                    out.push('\n')
                    p = p + 2u64
                } elif e == 'r' {
                    out.push('\r')
                    p = p + 2u64
                } elif e == 't' {
                    out.push('\t')
                    p = p + 2u64
                } elif e == 'u' {
                    val hi: i64 = self.hex4(text, p + 2u64)
                    if hi < 0i64 {
                        return Result::Err(JsonError::Invalid(p))
                    } elif hi >= 55296i64 && hi <= 56319i64 {
                        # A high surrogate is half of a codepoint; the
                        # other half has to follow it.
                        val lo: i64 = self.hex4(text, p + 8u64)
                        val slash: u64 = self.byte_at(text, p + 6u64)
                        val u: u64 = self.byte_at(text, p + 7u64)
                        if slash != '\\' || u != 'u' || lo < 56320i64 || lo > 57343i64 {
                            return Result::Err(JsonError::Invalid(p))
                        } else {
                            val c: i64 = 65536i64 + (hi - 55296i64) * 1024i64 + (lo - 56320i64)
                            out.push_char(c as u32)
                            p = p + 12u64
                        }
                    } elif hi >= 56320i64 && hi <= 57343i64 {
                        # A low surrogate with nothing before it.
                        return Result::Err(JsonError::Invalid(p))
                    } else {
                        out.push_char(hi as u32)
                        p = p + 6u64
                    }
                } else {
                    return Result::Err(JsonError::Invalid(p))
                }
            } else {
                out.push(b as u8)
                p = p + 1u64
            }
        }
        val id: u64 = self.push_text_node(out)
        self.close_leaf(id)
        Result::Ok(p)
    }

    # A number, in JSON's grammar and no wider: no leading `+`, no
    # leading zero, no bare `.5`, no hex.
    fn read_number(&mut self, text: &String, pos: u64) -> Result<u64, JsonError> {
        var p: u64 = pos
        if self.byte_at(text, p) == '-' { p = p + 1u64 }
        val first: u64 = self.byte_at(text, p)
        if first == '0' {
            p = p + 1u64
            # `01` is two tokens to a lenient reader and one mistake
            # to everyone else. Rejected here rather than left to the
            # caller as trailing content, so it reads the same at the
            # top level as it does inside an array.
            if json::is_digit(self.byte_at(text, p)) {
                return Result::Err(JsonError::Invalid(p))
            }
        } elif json::is_digit(first) {
            while json::is_digit(self.byte_at(text, p)) { p = p + 1u64 }
        } else {
            return Result::Err(JsonError::Invalid(p))
        }
        var floating: bool = false
        if self.byte_at(text, p) == '.' {
            floating = true
            p = p + 1u64
            if !json::is_digit(self.byte_at(text, p)) {
                return Result::Err(JsonError::Invalid(p))
            }
            while json::is_digit(self.byte_at(text, p)) { p = p + 1u64 }
        }
        val e: u64 = self.byte_at(text, p)
        if e == 'e' || e == 'E' {
            floating = true
            p = p + 1u64
            val sign: u64 = self.byte_at(text, p)
            if sign == '+' || sign == '-' { p = p + 1u64 }
            if !json::is_digit(self.byte_at(text, p)) {
                return Result::Err(JsonError::Invalid(p))
            }
            while json::is_digit(self.byte_at(text, p)) { p = p + 1u64 }
        }
        val token: String = text.substring(pos, p)
        val spelling: str = token.to_str()
        var placed: bool = false
        if !floating {
            val as_int = parse::to_i64(spelling)
            match as_int {
                Result::Ok(v) => {
                    val id: u64 = self.push_node(json::kind_int(), v, 0f64)
                    self.close_leaf(id)
                    placed = true
                }
                Result::Err(_) => {
                    # Too wide for an i64. It becomes a `Num` and
                    # loses precision, which is the documented edge.
                }
            }
        }
        if placed { return Result::Ok(p) }
        val as_num = parse::to_f64(spelling)
        var ok: bool = false
        match as_num {
            Result::Ok(v) => {
                val id: u64 = self.push_node(json::kind_num(), 0i64, v)
                self.close_leaf(id)
                ok = true
            }
            Result::Err(_) => {}
        }
        if !ok {
            return Result::Err(JsonError::Invalid(pos))
        }
        Result::Ok(p)
    }

    # Read a document into `self`, answering with the root index
    # (always 0) or why it stopped.
    #
    #     var doc: Json = Json::new()
    #     val r = doc.read(text)
    #     match r {
    #         Result::Ok(root) => { ... }
    #         Result::Err(e) => { println(e) }
    #     }
    #
    # `json::parse` is the same thing without the two lines; reach for
    # this one when you want to read into a document you already hold.
    #
    # The whole input has to be one value: anything after it is
    # `Trailing`, and whitespace alone is `Empty`. A failed read
    # leaves whatever was read before the failure in the document, so
    # read into a fresh `Json` rather than reusing one.
    fn read(&mut self, s: str) -> Result<u64, JsonError> {
        val text: String = String::from_str(s)
        val start: u64 = self.skip_ws(&text, 0u64)
        if start >= text.size() {
            return Result::Err(JsonError::Empty)
        }
        val after: u64 = self.read_value(&text, start, 0u64)?
        val end: u64 = self.skip_ws(&text, after)
        if end < text.size() {
            return Result::Err(JsonError::Trailing(end))
        }
        Result::Ok(0u64)
    }
}

# Read a whole document, the way §2 of the design asked for it.
#
#     val r = json::parse(text)
#     match r {
#         Result::Ok(doc) => { ... }
#         Result::Err(e) => { println(e) }
#     }
#
# A free function rather than an associated one so the call reads as
# the entrance it is; `Json::read` is the same reader with the
# document supplied by the caller.
pub fn parse(s: str) -> Result<Json, JsonError> {
    var doc: Json = Json::new()
    # `?` rather than a `match`: handing the document back from an
    # arm would be a move inside a branch, which is `[E0014]`.
    val root: u64 = doc.read(s)?
    Result::Ok(doc)
}
