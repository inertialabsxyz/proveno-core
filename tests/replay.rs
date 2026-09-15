//! Integration tests for replay: post-failure VM accessors and strict tape
//! replay.

use proveno::{
    bytecode::verify,
    compiler::{CompiledProgram, compile},
    host::transcript::ToolCallStatus,
    parser::parse,
    types::{
        table::{LuaKey, LuaTable},
        value::{LuaString, LuaValue},
    },
    vm::{
        engine::{HostInterface, Vm, VmConfig},
        gas::VmError,
    },
};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Answers each tool by name with a fixed response; unknown tools fail.
struct NamedHost {
    responses: Vec<(&'static str, Result<LuaTable, String>)>,
}

impl HostInterface for NamedHost {
    fn call_tool(&mut self, name: &str, _args: &LuaTable) -> Result<LuaTable, String> {
        self.responses
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, r)| r.clone())
            .unwrap_or_else(|| Err(format!("unknown tool '{name}'")))
    }
}

fn program(src: &str) -> CompiledProgram {
    let program = compile(&parse(src).expect("parse failed")).expect("compile failed");
    verify(&program).expect("verify failed");
    program
}

fn strip_line(e: VmError) -> VmError {
    match e {
        VmError::WithLine(_, inner) => *inner,
        other => other,
    }
}

fn response(key: &str, val: i64) -> LuaTable {
    let mut t = LuaTable::new();
    t.rawset(
        LuaKey::String(LuaString::from_str(key)),
        LuaValue::Integer(val),
    )
    .unwrap();
    t
}

fn host() -> NamedHost {
    NamedHost {
        responses: vec![
            ("price", Ok(response("price", 100))),
            ("transfer", Err("denied: amount over limit".to_owned())),
        ],
    }
}

// ── Post-failure accessors ────────────────────────────────────────────────────

#[test]
fn accessors_match_output_after_successful_run() {
    let prog = program(r#"local r = tool.call("price", {asset = "eth"}) return r.price"#);
    let mut vm = Vm::new(VmConfig::default(), host());
    let out = vm.execute(&prog, LuaValue::Nil).unwrap();
    assert_eq!(vm.transcript().len(), out.transcript.len());
    assert_eq!(vm.transcript()[0].args_canonical, br#"{"asset":"eth"}"#);
    assert_eq!(vm.gas_used(), out.gas_used);
    assert_eq!(vm.memory_used(), out.memory_used);
    assert_eq!(vm.host().responses.len(), 2);
}

#[test]
fn transcript_and_meters_readable_after_uncaught_tool_error() {
    let prog = program(
        r#"
        local p = tool.call("price", {asset = "eth"})
        tool.call("transfer", {amount = p.price})
        return 1
    "#,
    );
    let mut vm = Vm::new(VmConfig::default(), host());
    let err = strip_line(vm.execute(&prog, LuaValue::Nil).unwrap_err());
    assert_eq!(
        err,
        VmError::ToolError("denied: amount over limit".to_owned())
    );

    let transcript = vm.transcript();
    assert_eq!(transcript.len(), 2);
    assert_eq!(transcript[1].seq, 1);
    assert_eq!(transcript[1].tool_name, "transfer");
    assert_eq!(transcript[1].args_canonical, br#"{"amount":100}"#);
    assert_eq!(transcript[1].status, ToolCallStatus::Error);
    assert_eq!(transcript[1].error_message, "denied: amount over limit");
    assert!(vm.gas_used() > 0);
    assert!(vm.memory_used() > 0);
}

#[test]
fn transcript_and_meters_readable_after_gas_exhaustion() {
    let prog = program(
        r#"
        local p = tool.call("price", {asset = "eth"})
        local i = 0
        while true do i = i + 1 end
    "#,
    );
    let config = VmConfig {
        gas_limit: 5_000,
        ..VmConfig::default()
    };
    let mut vm = Vm::new(config, host());
    let err = strip_line(vm.execute(&prog, LuaValue::Nil).unwrap_err());
    assert_eq!(err, VmError::GasExhausted);
    assert_eq!(vm.transcript().len(), 1);
    assert_eq!(vm.transcript()[0].tool_name, "price");
    assert_eq!(vm.gas_used(), 5_000);
    assert!(vm.memory_used() > 0);
}
