// LLM-LOOP P2 — every diagnostic carries a position, and that position
// is one the reader can trust.
//
// Three separate failures were in play before P2:
//   1. the message body was prefixed with `line:column:offset:`, which
//      duplicated the header and leaked a byte offset
//   2. the caret was guessed by string-matching the message against the
//      source line, so it landed on the wrong token or a fixed `^^`
//   3. plenty of diagnostics carried no location at all, and errors
//      raised inside an imported module carried one that pointed into a
//      *different file* while being rendered against this one


use crate::common::test_program;

/// Run a program expected to fail *at runtime* and return what the
/// binary would have printed.
fn runtime_diagnostics(source: &str) -> String {
    match test_program(source) {
        Ok(v) => panic!("expected a runtime failure, got {v:?}"),
        Err(e) => e,
    }
}

fn diagnostics(source: &str) -> String {
    match test_program(source) {
        Ok(_) => panic!("expected the program to fail type checking:\n{source}"),
        Err(e) => e,
    }
}

/// Every reported diagnostic must carry a file:line:column header.
fn assert_all_located(diags: &str) {
    assert!(
        !diags.contains("Error: ["),
        "a diagnostic was reported with no location:\n{diags}"
    );
    assert!(diags.contains("Error at"), "no diagnostics at all:\n{diags}");
}

#[test]
fn message_body_carries_no_internal_coordinates() {
    let diags = diagnostics(
        "fn main() -> u64 {
            val a: bool = 1u64
            0u64
        }",
    );
    // The header is `test.t:2:27:`; the *message* must not repeat a
    // position, and must never contain a raw byte offset.
    let message = diags
        .split("Type mismatch")
        .nth(1)
        .expect("expected a type mismatch diagnostic");
    assert!(
        !message.contains("offset"),
        "byte offset leaked into the message:\n{diags}"
    );
    // `2:27:41:` style prefixes: three colon-separated numbers in a row.
    assert!(
        !diags.contains(":41:") && !diags.contains(":40:"),
        "internal coordinates still in the message:\n{diags}"
    );
}

#[test]
fn caret_width_matches_the_offending_token() {
    let diags = diagnostics(
        "fn main() -> u64 {
            val a: bool = 1u64
            0u64
        }",
    );
    // `1u64` is four characters, so four carets -- not the old fixed
    // two, and not the whole line.
    assert!(
        diags.contains("^^^^ [E0001] Type mismatch"),
        "expected a 4-wide caret over `1u64`:\n{diags}"
    );
}

#[test]
fn undefined_call_is_anchored_at_the_callee_name() {
    let diags = diagnostics(
        "fn main() -> u64 {
            val z = not_a_function(1u64)
            0u64
        }",
    );
    // 14 characters of `not_a_function`, and the column must be the
    // start of the name rather than the `(` that follows it.
    assert!(
        diags.contains("^^^^^^^^^^^^^^ [E0003] Function 'not_a_function' not found"),
        "caret should cover the callee name:\n{diags}"
    );
}

#[test]
fn argument_mismatch_is_anchored_at_the_argument() {
    let diags = diagnostics(
        "fn takes_i64(a: i64) -> i64 { a }
        fn main() -> u64 {
            val c = takes_i64(1u64)
            0u64
        }",
    );
    // The offending value is the argument, not the callee: pointing at
    // `takes_i64` would not say which argument to change. E0001 rather
    // than the E0010 catch-all -- a wrong argument type is a type
    // mismatch, and one of the most common errors in the language.
    assert!(
        diags.contains("^^^^ [E0001] Type mismatch"),
        "caret should cover the argument `1u64`:\n{diags}"
    );
    assert!(
        diags.contains("argument 1 of function 'takes_i64'"),
        "the message should say which argument:\n{diags}"
    );
}

#[test]
fn binding_mismatch_is_anchored_at_the_initializer() {
    let diags = diagnostics(
        "fn main() -> u64 {
            val flag: bool = 7u64
            0u64
        }",
    );
    assert!(
        diags.contains("^^^^ [E0001] Type mismatch"),
        "caret should cover the initializer `7u64`:\n{diags}"
    );
}

// --- location coverage ------------------------------------------------
//
// One test per diagnostic shape that used to escape with no position.

#[test]
fn bare_call_statement_error_is_located() {
    assert_all_located(&diagnostics("fn main() -> u64 { no_such_function() }"));
}

#[test]
fn impl_method_return_mismatch_is_located() {
    assert_all_located(&diagnostics(
        "struct P { x: u64 }
        impl P {
            fn get(self: Self) -> bool { self.x }
        }
        fn main() -> u64 { 0u64 }",
    ));
}

#[test]
fn array_index_type_error_is_located() {
    assert_all_located(&diagnostics(
        "fn main() -> u64 {
            val a = [1u64, 2u64]
            a[true]
        }",
    ));
}

#[test]
fn non_exhaustive_match_is_located() {
    assert_all_located(&diagnostics(
        "enum E { A, B }
        fn main() -> u64 {
            val e = E::A
            match e { E::A => 0u64 }
        }",
    ));
}

#[test]
fn non_bool_requires_clause_is_located() {
    assert_all_located(&diagnostics(
        "fn g(n: u64) -> u64
            requires n
        { n }
        fn main() -> u64 { g(1u64) }",
    ));
}

#[test]
fn while_condition_type_error_is_located() {
    assert_all_located(&diagnostics(
        "fn main() -> u64 {
            while 1u64 { }
            0u64
        }",
    ));
}

#[test]
fn struct_literal_field_mismatch_is_located() {
    assert_all_located(&diagnostics(
        "struct P { x: u64 }
        fn main() -> u64 {
            val p = P { x: true }
            0u64
        }",
    ));
}

/// Every expression form's caret covers the expression, not the token
/// that happens to name it.
///
/// A node used to be located wherever the parser's cursor sat when it
/// was built: a field access at the token on the *next line*, an index
/// at the `[`, a unary at its `-`, a cast at its `as`, an `if` at the
/// first token of the following statement. Two consequences, one bad
/// and one worse — a one-character caret says nothing about which
/// subexpression is wrong, and a caret on an unrelated line points the
/// reader confidently at innocent code, which is precisely the failure
/// P2 exists to prevent.
#[test]
fn every_expression_form_underlines_itself() {
    // `val q: bool = <expr>` anchors the mismatch at the initializer,
    // so the caret width is the expression node's own span.
    let cases: &[(&str, &str, &str)] = &[
        ("field access", "struct P { x: u64 }\nfn main() -> u64 {\n    val p = P { x: 1u64 }\n    val q: bool = p.x\n    0u64\n}", "^^^"),
        ("tuple access", "fn main() -> u64 {\n    val t = (1u64, 2u64)\n    val q: bool = t.0\n    0u64\n}", "^^^"),
        ("index", "fn main() -> u64 {\n    val a: [u64; 2] = [1u64, 2u64]\n    val q: bool = a[0]\n    0u64\n}", "^^^^"),
        ("slice", "fn main() -> u64 {\n    val a: [u64; 3] = [1u64, 2u64, 3u64]\n    val q: bool = a[0..2]\n    0u64\n}", "^^^^^^"),
        ("unary", "fn main() -> u64 {\n    val n: i64 = 1i64\n    val q: bool = -n\n    0u64\n}", "^^"),
        ("cast", "fn main() -> u64 {\n    val n: u64 = 1u64\n    val q: bool = n as i64\n    0u64\n}", "^^^^^^^^"),
        ("method call", "fn main() -> u64 {\n    val n: i64 = -3i64\n    val q: bool = n.abs()\n    0u64\n}", "^^^^^^^"),
        ("struct literal", "struct P { x: u64 }\nfn main() -> u64 {\n    val q: bool = P { x: 1u64 }\n    0u64\n}", "^^^^^^^^^^^^^"),
        ("tuple literal", "fn main() -> u64 {\n    val q: bool = (1u64, 2u64)\n    0u64\n}", "^^^^^^^^^^^^"),
        ("array literal", "fn main() -> u64 {\n    val q: bool = [1u64, 2u64]\n    0u64\n}", "^^^^^^^^^^^^"),
        ("binary", "fn main() -> u64 {\n    val q: bool = 1u64 + 2u64\n    0u64\n}", "^^^^^^^^^^^"),
        // `if` and `match` span several lines and the caret is clamped
        // to the line it annotates, so these are anchored at the
        // keyword — precise, and on the right line.
        ("if", "fn main() -> u64 {\n    val q: bool = if true { 1u64 } else { 2u64 }\n    0u64\n}", "^^ "),
        ("match", "fn main() -> u64 {\n    val q: bool = match 1u64 { _ => 2u64 }\n    0u64\n}", "^^^^^ "),
    ];
    for (shape, source, caret) in cases {
        let diags = diagnostics(source);
        assert!(
            diags.contains(caret),
            "{shape}: expected a caret of `{caret}`:\n{diags}"
        );
        // The caret has to be on the line holding the expression, not
        // on a later statement that happens to sit at that offset.
        assert!(
            diags.contains("val q: bool"),
            "{shape}: the diagnostic points at the wrong line:\n{diags}"
        );
    }
}

/// A call keeps its narrower anchor: "function not found" has to point
/// at the name, not at the whole call.
#[test]
fn a_call_stays_anchored_at_its_callee() {
    let diags = diagnostics(
        "fn main() -> u64 {
            val z = no_such_thing(1u64, 2u64)
            0u64
        }",
    );
    assert!(
        diags.contains("^^^^^^^^^^^^^ [E0003]"),
        "caret should cover just the callee name:\n{diags}"
    );
}

// INTERP-DIAG-SPAN: a diagnostic raised inside a string interpolation.
//
// The desugaring re-lexes each `{...}` segment with a fresh lexer whose
// positions start at zero, and used to insert the resulting tokens with
// no span of their own — so every node built from them claimed
// whatever the cursor happened to hold, and the error surfaced at line
// 1 of the file. The lexer now records each segment's absolute offset
// and the parser shifts the sub-lexer's positions by it.
//
// Note: `diagnostics()` returns the errors through a `Debug` format, so
// the quoted source line arrives with its `"` escaped. These tests
// match on quote-free fragments and read the column out of the header
// rather than counting characters in the rendered line.

/// `line:column` from the first `Error at <file>:<line>:<column>:`
/// header. `diagnostics()` hands back the rendered text through a
/// `Debug` format, so the whole report arrives as one line with `\n`
/// spelled out — hence scanning for the first two runs of digits
/// after the header rather than splitting on lines.
fn first_error_line_col(diags: &str) -> (usize, usize) {
    let at = diags.find("Error at").expect("a located diagnostic");
    let nums: Vec<usize> = diags[at..]
        .split(|c: char| !c.is_ascii_digit())
        .filter(|t| !t.is_empty())
        .take(2)
        .map(|t| t.parse().expect("a line / column number"))
        .collect();
    (nums[0], nums[1])
}

#[test]
fn an_error_inside_an_interpolation_points_at_the_sub_expression() {
    let diags = diagnostics(
        "fn main() -> u64 {
            val a: u64 = 1u64
            println(\"{a + true}\")
            0u64
        }",
    );
    assert_all_located(&diags);
    let (line, _) = first_error_line_col(&diags);
    assert_eq!(
        line, 3,
        "the interpolating line is 3, not the file's first line:\n{diags}"
    );
    assert!(
        diags.contains("{a + true}"),
        "the diagnostic should quote the interpolating line:\n{diags}"
    );
}

#[test]
fn the_caret_picks_the_right_segment_of_a_multi_part_literal() {
    // Three segments, only the middle one is ill-typed: the column has
    // to land inside it rather than at the start of the literal.
    let source = "fn main() -> u64 {
            val a: u64 = 1u64
            val b: bool = true
            println(\"first={a} second={a + b} third={a}\")
            0u64
        }";
    let diags = diagnostics(source);
    assert_all_located(&diags);
    let (line, column) = first_error_line_col(&diags);
    assert_eq!(line, 4, "the literal is on line 4:\n{diags}");
    let src_line = source.lines().nth(3).expect("the interpolating line");
    // Columns are 1-based; `find` is 0-based.
    let second = src_line.find("second=").expect("the second segment") + 1;
    let third = src_line.find("third=").expect("the third segment") + 1;
    assert!(
        column > second && column < third,
        "column {column} should sit inside the `second=` segment \
         ({second}..{third}):\n{diags}"
    );
}

#[test]
fn a_malformed_format_spec_points_at_its_literal() {
    // STR-INTERP-FMT: the spec is rejected at parse time, before any
    // token from the segment exists, so it reports against the literal
    // it was written in.
    let diags = diagnostics(
        "fn main() -> u64 {
            val x: f64 = 1.5f64
            println(\"{x:.q}\")
            0u64
        }",
    );
    // A parse error, so it arrives as a `ParserError` rather than the
    // rendered `Error at` form the type checker produces — assert on
    // the location it carries.
    assert!(
        diags.contains("invalid format spec"),
        "the diagnostic should name the problem:\n{diags}"
    );
    assert!(
        diags.contains("line: 3"),
        "the spec is written on line 3:\n{diags}"
    );
}

// --- DEBUG-OBS D2: a position knows which file it is in -------------
//
// Item 3 of this file's header — "errors raised inside an imported
// module carried a location that pointed into a *different file* while
// being rendered against this one" — was papered over rather than
// fixed: `module_integration` dropped module positions on the floor,
// so there was nothing to render wrongly. Now positions survive
// integration, and each one says which file it belongs to.

#[test]
fn a_failure_inside_the_stdlib_quotes_the_stdlib() {
    let diags = runtime_diagnostics(
        "fn main() -> u64 {
            val o: Option<u64> = Option::None
            val v: u64 = o.unwrap()
            v
        }",
    );
    assert!(
        diags.contains("core/std/option.t:"),
        "the panic is in the stdlib and should say so:\n{diags}"
    );
    assert!(
        !diags.contains("test.t:"),
        "the user's file is not where this failed:\n{diags}"
    );
    // The excerpt has to come from option.t's text. Before D2 the
    // formatter had one source string and would have drawn whatever
    // sat at that line number in the user's program.
    assert!(
        diags.contains("=> panic("),
        "the quoted line should be the module's own:\n{diags}"
    );
}

#[test]
fn a_failure_in_the_users_file_is_still_named_by_the_driver() {
    // The other direction of the same rule: the entry file is named by
    // whoever ran it, not by anything the map recorded.
    let diags = runtime_diagnostics(
        "fn main() -> u64 { panic(\"mine\") }",
    );
    assert!(diags.contains("test.t:1:"), "{diags}");
}

#[test]
fn integrated_positions_name_a_file_the_map_can_resolve() {
    // The structural half: every position copied out of a module is
    // re-anchored, and the id it carries resolves to a real file. An
    // id that resolved to nothing would put us back to drawing module
    // lines against the entry source.
    let source = "fn main() -> u64 {\n    val o: Option<u64> = Option::None\n    0u64\n}\n";
    let mut parser = frontend::ParserWithInterner::new(source);
    let mut program = parser.parse_program().expect("parse");
    interpreter::check_typing_with_core_modules(
        &mut program,
        parser.get_string_interner(),
        Some(source),
        Some("test.t"),
        std::slice::from_ref(&crate::common::core_modules_dir()),
    )
    .expect("type check");

    assert!(
        program.source_map.len() > 1,
        "the stdlib modules should be registered, got {} file(s)",
        program.source_map.len()
    );
    let foreign: Vec<_> = program
        .location_pool
        .expr_locations
        .iter()
        .flatten()
        .filter(|loc| loc.file != frontend::source_map::FileId::ENTRY)
        .collect();
    assert!(
        !foreign.is_empty(),
        "integration copied module expressions but no positions with them"
    );
    for loc in foreign {
        let file = program
            .source_map
            .get(loc.file)
            .unwrap_or_else(|| panic!("location names {:?}, which the map does not have", loc.file));
        assert!(!file.path.is_empty(), "an integrated file with no name");
        // `+ 1`: the parser anchors an end-of-input location one line
        // past the last, which is a position in that file even though
        // no text sits there.
        assert!(
            (loc.line as usize) <= file.source.lines().count() + 1,
            "line {} is past the end of {}",
            loc.line,
            file.path
        );
    }
}
