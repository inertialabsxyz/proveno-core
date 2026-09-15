//! Integration tests for replay: post-failure VM accessors and strict tape
//! replay.

use proveno::{
    OracleTape, TapeCall, TapeHost,
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

// ── Strict replay ─────────────────────────────────────────────────────────────

const PRICE_THEN_TRANSFER: &str = r#"
    local p = tool.call("price", {asset = "eth"})
    local ok, err = pcall(function()
        tool.call("transfer", {amount = 20})
    end)
    return p.price
"#;

/// Record `src` against the live host, returning the finished VM.
fn record(src: &str, config: VmConfig) -> (Result<proveno::VmOutput, VmError>, Vm<NamedHost>) {
    let mut vm = Vm::new(config, host());
    let result = vm.execute(&program(src), LuaValue::Nil);
    (result, vm)
}

/// Strictly replay `src` over the tape built from `recorded`'s transcript.
fn strict_replay<H: HostInterface>(
    src: &str,
    recorded: &Vm<H>,
    config: VmConfig,
) -> (Result<proveno::VmOutput, VmError>, Vm<TapeHost>) {
    let tape = OracleTape::from_records(recorded.transcript());
    let mut vm = Vm::new(config, TapeHost::strict(tape));
    let result = vm.execute(&program(src), LuaValue::Nil);
    (result, vm)
}

#[test]
fn strict_replay_of_recorded_run_matches() {
    let (recorded, rec_vm) = record(PRICE_THEN_TRANSFER, VmConfig::default());
    let recorded = recorded.unwrap();
    assert_eq!(recorded.transcript.len(), 2);

    let (replayed, vm) = strict_replay(PRICE_THEN_TRANSFER, &rec_vm, VmConfig::default());
    let replayed = replayed.unwrap();
    assert!(vm.host().divergence().is_none());
    assert!(vm.host().is_exhausted());
    assert_eq!(replayed.return_value, recorded.return_value);
    assert_eq!(replayed.gas_used, recorded.gas_used);
    assert_eq!(replayed.memory_used, recorded.memory_used);
}

#[test]
fn strict_replay_reports_changed_argument() {
    let (_, rec_vm) = record(PRICE_THEN_TRANSFER, VmConfig::default());
    let changed = PRICE_THEN_TRANSFER.replace("amount = 20", "amount = 2000");

    let (result, vm) = strict_replay(&changed, &rec_vm, VmConfig::default());
    // The divergence is raised inside pcall, so the run itself completes.
    assert!(result.is_ok());
    let d = vm.host().divergence().expect("divergence");
    assert_eq!(d.seq, 1);
    assert_eq!(
        d.expected,
        Some(TapeCall {
            tool_name: "transfer".to_owned(),
            args_canonical: br#"{"amount":20}"#.to_vec(),
        })
    );
    assert_eq!(
        d.actual,
        TapeCall {
            tool_name: "transfer".to_owned(),
            args_canonical: br#"{"amount":2000}"#.to_vec(),
        }
    );
    assert_eq!(vm.transcript()[1].error_message, "replay diverged at seq 1");
}

#[test]
fn strict_replay_reports_extra_call_with_no_expected() {
    let src = r#"local p = tool.call("price", {asset = "eth"}) return p.price"#;
    let (_, rec_vm) = record(src, VmConfig::default());
    let extra = r#"
        local p = tool.call("price", {asset = "eth"})
        tool.call("price", {asset = "btc"})
        return p.price
    "#;

    let (result, vm) = strict_replay(extra, &rec_vm, VmConfig::default());
    assert_eq!(
        strip_line(result.unwrap_err()),
        VmError::ToolError("replay diverged at seq 1".to_owned())
    );
    let d = vm.host().divergence().expect("divergence");
    assert_eq!(d.seq, 1);
    assert_eq!(d.expected, None);
    assert_eq!(d.actual.tool_name, "price");
    assert_eq!(d.actual.args_canonical, br#"{"asset":"btc"}"#);
}

#[test]
fn strict_replay_of_uncaught_tool_error_ends_in_same_error() {
    let src = r#"
        local p = tool.call("price", {asset = "eth"})
        tool.call("transfer", {amount = p.price})
        return 1
    "#;
    let (recorded, rec_vm) = record(src, VmConfig::default());
    let recorded_err = strip_line(recorded.unwrap_err());
    assert_eq!(rec_vm.transcript()[1].status, ToolCallStatus::Error);

    let (replayed, vm) = strict_replay(src, &rec_vm, VmConfig::default());
    assert_eq!(strip_line(replayed.unwrap_err()), recorded_err);
    assert!(vm.host().divergence().is_none());
    assert_eq!(vm.transcript().len(), rec_vm.transcript().len());
    assert_eq!(vm.gas_used(), rec_vm.gas_used());
    assert_eq!(vm.memory_used(), rec_vm.memory_used());
}

#[test]
fn strict_replay_of_gas_exhausted_run_ends_in_same_error() {
    let src = r#"
        local p = tool.call("price", {asset = "eth"})
        local i = 0
        while true do i = i + p.price end
    "#;
    let config = VmConfig {
        gas_limit: 5_000,
        ..VmConfig::default()
    };
    let (recorded, rec_vm) = record(src, config.clone());
    assert_eq!(strip_line(recorded.unwrap_err()), VmError::GasExhausted);

    let (replayed, vm) = strict_replay(src, &rec_vm, config);
    assert_eq!(strip_line(replayed.unwrap_err()), VmError::GasExhausted);
    assert!(vm.host().divergence().is_none());
    assert_eq!(vm.transcript().len(), 1);
    assert_eq!(vm.gas_used(), rec_vm.gas_used());
    assert_eq!(vm.memory_used(), rec_vm.memory_used());
}
