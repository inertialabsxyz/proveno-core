//! `s:method(...)` on a string dispatches to the string module with `s` as the
//! first argument, as Lua's string metatable does.

use proveno::types::value::{LuaString, LuaValue};
use proveno::{
    bytecode::verify,
    compiler::compile,
    parser::parse,
    vm::engine::{NoopHost, Vm, VmConfig},
    vm::gas::VmError,
};

fn run(src: &str) -> Result<LuaValue, VmError> {
    let block = parse(src).expect("parse failed");
    let program = compile(&block).expect("compile failed");
    verify(&program).expect("verify failed");
    let mut vm = Vm::new(VmConfig::default(), NoopHost);
    match vm.execute(&program, LuaValue::Nil) {
        Ok(out) => Ok(out.return_value),
        Err(VmError::WithLine(_, inner)) => Err(*inner),
        Err(e) => Err(e),
    }
}

fn s(text: &str) -> LuaValue {
    LuaValue::String(LuaString::from_str(text))
}

/// The colon form and the dot form must return the same value.
fn assert_same(colon: &str, dot: &str, expected: LuaValue) {
    assert_eq!(run(colon).expect(colon), expected, "source: {colon}");
    assert_eq!(run(dot).expect(dot), expected, "source: {dot}");
}

fn runtime_message(src: &str) -> String {
    match run(src).expect_err(src) {
        VmError::RuntimeError(LuaValue::String(m)) => {
            String::from_utf8(m.as_bytes().to_vec()).unwrap()
        }
        other => panic!("expected a runtime error from {src}, got {other:?}"),
    }
}

#[test]
fn method_len() {
    assert_same(
        r#"local x = "hello" return x:len()"#,
        r#"local x = "hello" return string.len(x)"#,
        LuaValue::Integer(5),
    );
}

#[test]
fn method_sub() {
    assert_same(
        r#"local x = "hello" return x:sub(2)"#,
        r#"local x = "hello" return string.sub(x, 2)"#,
        s("ello"),
    );
    assert_same(
        r#"local x = "hello" return x:sub(2, 3)"#,
        r#"local x = "hello" return string.sub(x, 2, 3)"#,
        s("el"),
    );
}

#[test]
fn method_find() {
    assert_same(
        r#"local x = "hello" local i, j = x:find("ll") return i"#,
        r#"local x = "hello" local i, j = string.find(x, "ll") return i"#,
        LuaValue::Integer(3),
    );
}

#[test]
fn method_find_plain_flag_matches_dot_form() {
    assert_same(
        r#"local x = "3245.67" local i, j = x:find(".", 1, true) return i"#,
        r#"local x = "3245.67" local i, j = string.find(x, ".", 1, true) return i"#,
        LuaValue::Integer(5),
    );
    assert_same(
        r#"local x = "3245" local i, j = x:find(".", 1, true) return i"#,
        r#"local x = "3245" local i, j = string.find(x, ".", 1, true) return i"#,
        LuaValue::Nil,
    );
    let colon = runtime_message(r#"local x = "3245.67" return x:find(".", 1, false)"#);
    let dot = runtime_message(r#"local x = "3245.67" return string.find(x, ".", 1, false)"#);
    assert_eq!(colon, dot);
}

#[test]
fn method_find_literal() {
    assert_same(
        r#"local x = "3245.67" local i, j = x:find_literal(".") return i"#,
        r#"local x = "3245.67" local i, j = string.find_literal(x, ".") return i"#,
        LuaValue::Integer(5),
    );
}

#[test]
fn method_upper_and_lower() {
    assert_same(
        r#"local x = "Hello" return x:upper()"#,
        r#"local x = "Hello" return string.upper(x)"#,
        s("HELLO"),
    );
    assert_same(
        r#"local x = "Hello" return x:lower()"#,
        r#"local x = "Hello" return string.lower(x)"#,
        s("hello"),
    );
}

#[test]
fn method_rep() {
    assert_same(
        r#"local x = "ab" return x:rep(3)"#,
        r#"local x = "ab" return string.rep(x, 3)"#,
        s("ababab"),
    );
}

#[test]
fn method_byte() {
    assert_same(
        r#"local x = "A" return x:byte()"#,
        r#"local x = "A" return string.byte(x)"#,
        LuaValue::Integer(65),
    );
}

#[test]
fn method_char_passes_receiver_first() {
    // Pointless in Lua too, but it must behave as string.char(x).
    let colon = run(r#"local x = "A" return x:char()"#);
    let dot = run(r#"local x = "A" return string.char(x)"#);
    assert_eq!(format!("{colon:?}"), format!("{dot:?}"));
}

#[test]
fn method_format() {
    assert_same(
        r#"local x = "%d + %d" return x:format(1, 2)"#,
        r#"local x = "%d + %d" return string.format(x, 1, 2)"#,
        s("1 + 2"),
    );
}

#[test]
fn method_on_literal_and_chained() {
    assert_eq!(
        run(r#"return ("hello"):sub(2):upper()"#).unwrap(),
        s("ELLO")
    );
}

#[test]
fn method_as_statement() {
    assert_eq!(
        run(r#"local x = "hello" x:upper() return x"#).unwrap(),
        s("hello")
    );
}

#[test]
fn method_match_gmatch_gsub_fail_as_the_dot_form_does() {
    for name in ["match", "gmatch", "gsub"] {
        let colon = runtime_message(&format!(r#"local x = "a" return x:{name}("a", "b")"#));
        let dot = runtime_message(&format!(
            r#"local x = "a" return string.{name}(x, "a", "b")"#
        ));
        assert_eq!(colon, dot, "{name}");
    }
}

#[test]
fn method_missing_from_string_module_is_named() {
    let msg = runtime_message(r#"local x = "hello" return x:reverse()"#);
    assert!(msg.contains("'reverse'"), "{msg}");
    assert!(msg.contains("string"), "{msg}");
}

#[test]
fn method_missing_is_catchable_by_pcall() {
    assert_eq!(
        run(r#"local ok = pcall(function() local x = "a" return x:nope() end) return ok"#).unwrap(),
        LuaValue::Boolean(false)
    );
}

#[test]
fn method_call_on_table_is_unaffected() {
    assert_eq!(
        run(r#"
local obj = { n = 40 }
obj.add = function(self, k) return self.n + k end
return obj:add(2)
"#)
        .unwrap(),
        LuaValue::Integer(42)
    );
    // A table field named like a string function is the table's, not string's.
    assert_eq!(
        run(r#"local t = { upper = function(self) return 7 end } return t:upper()"#).unwrap(),
        LuaValue::Integer(7)
    );
    // A missing method on a table is still a call of nil, not the string error.
    let err = run(r#"local t = {} return t:upper()"#).unwrap_err();
    assert!(
        !format!("{err:?}").contains("string has no method"),
        "{err:?}"
    );
}

#[test]
fn method_call_on_integer_is_still_a_type_error() {
    let err = run(r#"local n = 5 return n:upper()"#).unwrap_err();
    assert!(matches!(err, VmError::TypeError(_)), "{err:?}");
}

/// String dispatch is resolved in the VM, not the compiler: a colon call
/// compiles exactly as it did before, so no committed program moves.
#[test]
fn method_call_bytecode_is_unchanged() {
    use proveno::compiler::Instruction::*;
    let p = compile(&parse(r#"local x = "hello" return x:sub(2)"#).unwrap()).unwrap();
    assert_eq!(
        p.prototypes[0].code,
        vec![
            PushK(0),
            StoreLocal(0),
            LoadLocal(0),
            Dup,
            StoreLocal(1),
            GetField(1),
            LoadLocal(1),
            PushK(2),
            Call(2),
            Ret(1),
            Ret(0),
        ]
    );
    let p = compile(
        &parse(r#"local t = { upper = function(self) return 7 end } return t:upper()"#).unwrap(),
    )
    .unwrap();
    assert_eq!(
        p.prototypes[0].code,
        vec![
            NewTable,
            Dup,
            Closure(1),
            SetField(0),
            StoreLocal(0),
            LoadLocal(0),
            Dup,
            StoreLocal(1),
            GetField(0),
            LoadLocal(1),
            Call(1),
            Ret(1),
            Ret(0),
        ]
    );
}

/// Recorded on `2d0478c`, before string method dispatch. A change here means a
/// committed program hash has moved.
#[cfg(feature = "poseidon")]
#[test]
fn method_call_program_hash_is_unchanged() {
    let p = compile(&parse(r#"local x = "hello" return x:sub(2)"#).unwrap()).unwrap();
    assert_eq!(
        p.program_hash,
        [
            11, 62, 113, 252, 50, 113, 117, 70, 176, 71, 179, 54, 171, 34, 187, 198, 73, 242, 133,
            170, 229, 210, 174, 241, 105, 93, 220, 40, 76, 243, 247, 8
        ]
    );
    let p = compile(
        &parse(r#"local t = { upper = function(self) return 7 end } return t:upper()"#).unwrap(),
    )
    .unwrap();
    assert_eq!(
        p.program_hash,
        [
            13, 77, 67, 48, 174, 128, 170, 94, 86, 160, 99, 150, 112, 61, 60, 156, 83, 129, 62, 78,
            100, 248, 151, 69, 141, 43, 214, 95, 148, 193, 38, 225
        ]
    );
}
