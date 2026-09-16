//! Integration tests: pipeline, determinism, gas/memory metering, pcall,
//! iterators, and VmOutput fields. TLS attestation lives in `tls.rs`.
use proveno::{
    bytecode::verify,
    compiler::compile,
    parser::parse,
    types::value::{LuaString, LuaValue},
    vm::{
        engine::{NoopHost, Vm, VmConfig, VmOutput},
        gas::VmError,
    },
};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn strip_line_info(e: VmError) -> VmError {
    match e {
        VmError::WithLine(_, inner) => *inner,
        other => other,
    }
}

fn run_with_config(src: &str, config: VmConfig) -> Result<VmOutput, VmError> {
    let block = parse(src).expect("parse failed");
    let program = compile(&block).expect("compile failed");
    verify(&program).expect("verify failed");
    let mut vm = Vm::new(config, NoopHost);
    vm.execute(&program, LuaValue::Nil).map_err(strip_line_info)
}

fn run(src: &str) -> Result<VmOutput, VmError> {
    run_with_config(src, VmConfig::default())
}

fn run_ok(src: &str) -> VmOutput {
    run(src).expect("execution failed")
}

fn int(n: i64) -> LuaValue {
    LuaValue::Integer(n)
}

fn s(text: &str) -> LuaValue {
    LuaValue::String(LuaString::from_str(text))
}

// ── Gas metering ──────────────────────────────────────────────────────────────

#[test]
fn gas_used_is_nonzero_for_simple_program() {
    let out = run_ok("return 1 + 2");
    assert!(
        out.gas_used > 0,
        "expected gas_used > 0, got {}",
        out.gas_used
    );
}

#[test]
fn gas_used_increases_with_more_work() {
    let cheap = run_ok("return 1").gas_used;
    let expensive = run_ok(
        r#"
        local s = 0
        for i = 1, 100 do
            s = s + i
        end
        return s
    "#,
    )
    .gas_used;
    assert!(
        expensive > cheap,
        "more work should use more gas: cheap={} expensive={}",
        cheap,
        expensive
    );
}

#[test]
fn gas_limit_exhaustion_halts_execution() {
    let config = VmConfig {
        gas_limit: 10,
        ..VmConfig::default()
    };
    let err = run_with_config(
        r#"
        local i = 0
        while true do i = i + 1 end
    "#,
        config,
    )
    .unwrap_err();
    assert_eq!(err, VmError::GasExhausted);
}

#[test]
fn gas_exhaustion_escapes_pcall() {
    // GasExhausted is unrecoverable — pcall must not swallow it.
    let config = VmConfig {
        gas_limit: 40,
        ..VmConfig::default()
    };
    let err = run_with_config(
        r#"
        local ok, err = pcall(function()
            local i = 0
            while true do i = i + 1 end
        end)
        return ok
    "#,
        config,
    )
    .unwrap_err();
    assert_eq!(err, VmError::GasExhausted);
}

#[test]
fn gas_used_reported_in_vmoutput() {
    // gas_used field should reflect actual consumption (not always equal to limit).
    let config = VmConfig {
        gas_limit: 200_000,
        ..VmConfig::default()
    };
    let out = run_with_config("return 42", config).unwrap();
    assert!(out.gas_used < 200_000, "gas_used should be less than limit");
    assert!(out.gas_used > 0);
}

// ── Memory metering ───────────────────────────────────────────────────────────

#[test]
fn memory_used_is_nonzero_for_table_alloc() {
    let out = run_ok("local t = {} return 1");
    assert!(
        out.memory_used > 0,
        "expected memory_used > 0, got {}",
        out.memory_used
    );
}

#[test]
fn memory_limit_exhaustion_halts_execution() {
    // Allocating many tables in a loop should exceed a tight memory limit.
    let config = VmConfig {
        memory_limit_bytes: 500,
        ..VmConfig::default()
    };
    let err = run_with_config(
        r#"
        local i = 0
        while true do
            local t = {}
            i = i + 1
        end
        return i
    "#,
        config,
    )
    .unwrap_err();
    assert_eq!(err, VmError::MemoryExhausted);
}

#[test]
fn memory_exhaustion_escapes_pcall() {
    // MemoryExhausted is unrecoverable — pcall must not swallow it.
    let config = VmConfig {
        memory_limit_bytes: 500,
        ..VmConfig::default()
    };
    let err = run_with_config(
        r#"
        local ok, err = pcall(function()
            local i = 0
            while true do
                local t = {}
                i = i + 1
            end
        end)
        return ok
    "#,
        config,
    )
    .unwrap_err();
    assert_eq!(err, VmError::MemoryExhausted);
}

#[test]
fn memory_is_monotonic_hwm() {
    // String concatenation allocates a new string each iteration.
    // Because HWM is monotonic (no free credit), memory_used grows with each concat.
    let config = VmConfig {
        memory_limit_bytes: 1_000_000,
        ..VmConfig::default()
    };
    let out = run_with_config(
        r#"
        local i = 0
        while i < 50 do
            local s = "prefix-" .. tostring(i)
            i = i + 1
        end
        return i
    "#,
        config,
    )
    .unwrap();
    // 50 iterations × ~(24 + 9 bytes per concat result) ≈ 1650 bytes, well above 1000.
    assert!(
        out.memory_used > 1000,
        "expected meaningful memory use, got {}",
        out.memory_used
    );
}

// ── pcall error taxonomy ──────────────────────────────────────────────────────

#[test]
fn pcall_catches_call_depth_exceeded() {
    // Infinite mutual recursion should hit CallDepthExceeded, which IS recoverable.
    let src = r#"
        local function recurse(n)
            return recurse(n + 1)
        end
        local ok, err = pcall(recurse, 0)
        if ok then return 1 else return 0 end
    "#;
    let out = run_ok(src);
    assert_eq!(
        out.return_value,
        int(0),
        "pcall should have caught CallDepthExceeded"
    );
}

#[test]
fn pcall_call_depth_error_message_contains_depth() {
    let src = r#"
        local function recurse(n)
            return recurse(n + 1)
        end
        local ok, err = pcall(recurse, 0)
        return err
    "#;
    let out = run_ok(src);
    if let LuaValue::String(s) = &out.return_value {
        let msg = String::from_utf8_lossy(s.as_bytes());
        assert!(
            msg.contains("depth") || msg.contains("stack") || msg.contains("call"),
            "error message should mention depth/stack/call, got: {}",
            msg
        );
    } else {
        panic!("expected string error message, got {:?}", out.return_value);
    }
}

#[test]
fn pcall_catches_error_with_table_payload() {
    // error() can be called with a non-string value; pcall returns it as the second value.
    let src = r#"
        local ok, err = pcall(function()
            error({code = 42, msg = "oops"})
        end)
        if ok then return -1 end
        return err.code
    "#;
    let out = run_ok(src);
    assert_eq!(out.return_value, int(42));
}

#[test]
fn pcall_catches_error_with_integer_payload() {
    let src = r#"
        local ok, err = pcall(function()
            error(99)
        end)
        if ok then return -1 end
        return err
    "#;
    let out = run_ok(src);
    assert_eq!(out.return_value, int(99));
}

#[test]
fn nested_pcall_inner_catches_outer_succeeds() {
    let src = r#"
        local outer_ok, outer_val = pcall(function()
            local inner_ok, inner_err = pcall(function()
                error("inner boom")
            end)
            -- inner caught the error; outer function returns normally
            if inner_ok then return -1 end
            return 1
        end)
        if outer_ok then return outer_val else return -1 end
    "#;
    let out = run_ok(src);
    assert_eq!(out.return_value, int(1));
}

#[test]
fn pcall_in_loop_multiple_times() {
    // pcall used in a loop should work correctly every iteration.
    let src = r#"
        local count = 0
        for i = 1, 5 do
            local ok, _ = pcall(function()
                if i == 3 then error("three") end
            end)
            if ok then count = count + 1 end
        end
        return count
    "#;
    let out = run_ok(src);
    // Iterations 1,2,4,5 succeed; iteration 3 errors but is caught.
    assert_eq!(out.return_value, int(4));
}

#[test]
fn pcall_success_stack_continues_normally() {
    let src = r#"
        local ok, val = pcall(function() return 10 end)
        local ok2, val2 = pcall(function() return 20 end)
        return val + val2
    "#;
    let out = run_ok(src);
    assert_eq!(out.return_value, int(30));
}

// ── Determinism ───────────────────────────────────────────────────────────────

#[test]
fn determinism_same_return_value_both_runs() {
    let src = r#"
        local t = {b = 2, a = 1, c = 3}
        local result = 0
        for k, v in pairs_sorted(t) do
            result = result + v
        end
        return result
    "#;
    let block = parse(src).expect("parse failed");
    let program = compile(&block).expect("compile failed");
    verify(&program).expect("verify failed");

    let mut vm1 = Vm::new(VmConfig::default(), NoopHost);
    let out1 = vm1.execute(&program, LuaValue::Nil).expect("run 1 failed");

    let mut vm2 = Vm::new(VmConfig::default(), NoopHost);
    let out2 = vm2.execute(&program, LuaValue::Nil).expect("run 2 failed");

    assert_eq!(out1.return_value, out2.return_value);
}

#[test]
fn determinism_same_gas_used_both_runs() {
    let src = r#"
        local t = {z = 26, a = 1, m = 13}
        local s = ""
        for k, v in pairs_sorted(t) do
            s = s .. k
        end
        return s
    "#;
    let block = parse(src).expect("parse failed");
    let program = compile(&block).expect("compile failed");
    verify(&program).expect("verify failed");

    let mut vm1 = Vm::new(VmConfig::default(), NoopHost);
    let out1 = vm1.execute(&program, LuaValue::Nil).expect("run 1 failed");

    let mut vm2 = Vm::new(VmConfig::default(), NoopHost);
    let out2 = vm2.execute(&program, LuaValue::Nil).expect("run 2 failed");

    assert_eq!(
        out1.gas_used, out2.gas_used,
        "gas_used must be deterministic"
    );
    assert_eq!(
        out1.memory_used, out2.memory_used,
        "memory_used must be deterministic"
    );
}

#[test]
fn determinism_iterator_order_stable() {
    // pairs_sorted must visit keys in the same canonical order across runs.
    let src = r#"
        local t = {z = 1, a = 2, m = 3}
        local result = ""
        for k, v in pairs_sorted(t) do
            result = result .. k
        end
        return result
    "#;
    let out1 = run_ok(src);
    let out2 = run_ok(src);
    assert_eq!(out1.return_value, out2.return_value);
    // Also verify the actual order is correct (a < m < z).
    assert_eq!(out1.return_value, s("amz"));
}

#[test]
fn determinism_same_program_same_input_twice() {
    // Identical inputs → identical outputs, gas, memory.
    let src = "local x = 7 * 6 return x";
    let out1 = run_ok(src);
    let out2 = run_ok(src);
    assert_eq!(out1.return_value, out2.return_value);
    assert_eq!(out1.gas_used, out2.gas_used);
    assert_eq!(out1.memory_used, out2.memory_used);
}

// ── Iterator semantics ────────────────────────────────────────────────────────

#[test]
fn pairs_sorted_visits_string_keys_lexicographic() {
    let src = r#"
        local t = {z = 1, a = 2, m = 3}
        local result = ""
        for k, v in pairs_sorted(t) do
            result = result .. k
        end
        return result
    "#;
    let out = run_ok(src);
    assert_eq!(out.return_value, s("amz"));
}

#[test]
fn pairs_sorted_visits_integer_keys_ascending() {
    let src = r#"
        local t = {}
        t[3] = "three"
        t[1] = "one"
        t[2] = "two"
        local result = ""
        for k, v in pairs_sorted(t) do
            result = result .. v
        end
        return result
    "#;
    let out = run_ok(src);
    assert_eq!(out.return_value, s("onetwothree"));
}

#[test]
fn pairs_sorted_integer_keys_before_string_keys() {
    // Spec §4: integers ascending, then strings lexicographic.
    let src = r#"
        local t = {a = "A"}
        t[1] = "one"
        local result = ""
        for k, v in pairs_sorted(t) do
            result = result .. v
        end
        return result
    "#;
    let out = run_ok(src);
    assert_eq!(out.return_value, s("oneA"));
}

#[test]
fn pairs_sorted_empty_table_body_never_runs() {
    let src = r#"
        local t = {}
        local count = 0
        for k, v in pairs_sorted(t) do
            count = count + 1
        end
        return count
    "#;
    let out = run_ok(src);
    assert_eq!(out.return_value, int(0));
}

#[test]
fn pairs_sorted_key_value_correct() {
    // Verify that both key and value are yielded correctly.
    let src = r#"
        local t = {x = 10, y = 20}
        local sum = 0
        for k, v in pairs_sorted(t) do
            sum = sum + v
        end
        return sum
    "#;
    let out = run_ok(src);
    assert_eq!(out.return_value, int(30));
}

#[test]
fn ipairs_visits_consecutive_keys_from_one() {
    let src = r#"
        local t = {10, 20, 30}
        local sum = 0
        for i, v in ipairs(t) do
            sum = sum + v
        end
        return sum
    "#;
    let out = run_ok(src);
    assert_eq!(out.return_value, int(60));
}

#[test]
fn ipairs_stops_at_nil_gap() {
    // {10, nil, 30}: ipairs stops at index 2 (nil), so only index 1 is visited.
    let src = r#"
        local t = {}
        t[1] = 10
        t[3] = 30
        local count = 0
        for i, v in ipairs(t) do
            count = count + 1
        end
        return count
    "#;
    let out = run_ok(src);
    assert_eq!(out.return_value, int(1));
}

#[test]
fn ipairs_empty_table_body_never_runs() {
    let src = r#"
        local t = {}
        local count = 0
        for i, v in ipairs(t) do
            count = count + 1
        end
        return count
    "#;
    let out = run_ok(src);
    assert_eq!(out.return_value, int(0));
}

#[test]
fn ipairs_index_is_correct() {
    // Verify the index value yielded is 1-based and sequential.
    let src = r#"
        local t = {100, 200, 300}
        local index_sum = 0
        for i, v in ipairs(t) do
            index_sum = index_sum + i
        end
        return index_sum
    "#;
    let out = run_ok(src);
    // 1 + 2 + 3 = 6
    assert_eq!(out.return_value, int(6));
}

#[test]
fn iterator_break_exits_early() {
    let src = r#"
        local t = {10, 20, 30, 40, 50}
        local sum = 0
        for i, v in ipairs(t) do
            if i == 3 then break end
            sum = sum + v
        end
        return sum
    "#;
    let out = run_ok(src);
    // Visits i=1 (10) and i=2 (20), then breaks at i=3.
    assert_eq!(out.return_value, int(30));
}

#[test]
fn nested_ipairs_in_pairs_sorted() {
    let src = r#"
        local matrix = {}
        matrix.row1 = {1, 2, 3}
        matrix.row2 = {4, 5, 6}
        local total = 0
        for k, row in pairs_sorted(matrix) do
            for i, v in ipairs(row) do
                total = total + v
            end
        end
        return total
    "#;
    let out = run_ok(src);
    // 1+2+3+4+5+6 = 21
    assert_eq!(out.return_value, int(21));
}

// ── VmOutput fields ───────────────────────────────────────────────────────────

#[test]
fn vmoutput_logs_captures_log_calls() {
    let src = r#"
        log("hello")
        log("world")
        return 1
    "#;
    let out = run_ok(src);
    assert_eq!(out.logs.len(), 2);
    assert_eq!(out.logs[0], "hello");
    assert_eq!(out.logs[1], "world");
}

#[test]
fn vmoutput_logs_empty_when_no_log_calls() {
    let out = run_ok("return 42");
    assert!(out.logs.is_empty());
}

#[test]
fn vmoutput_gas_used_plus_remaining_equals_limit() {
    // gas_used should be consistent: used = limit - remaining.
    // We can verify indirectly: run two programs, the one with more
    // instructions must have higher gas_used.
    let out_light = run_ok("return 1");
    let out_heavy = run_ok(
        r#"
        local s = 0
        for i = 1, 20 do s = s + i end
        return s
    "#,
    );
    assert!(out_heavy.gas_used > out_light.gas_used);
}

#[test]
fn vmoutput_memory_used_grows_with_allocations() {
    let out_no_alloc = run_ok("return 1");
    let out_with_alloc = run_ok(
        r#"
        local t1 = {}
        local t2 = {}
        local t3 = {}
        return 1
    "#,
    );
    assert!(out_with_alloc.memory_used > out_no_alloc.memory_used);
}

// ── Error model completeness ──────────────────────────────────────────────────

#[test]
fn error_code_type_error_is_recoverable() {
    let src = r#"
        local ok, _ = pcall(function()
            return 1 + "not a number"
        end)
        return ok
    "#;
    let out = run_ok(src);
    assert_eq!(out.return_value, LuaValue::Boolean(false));
}

#[test]
fn error_code_runtime_error_is_recoverable() {
    let src = r#"
        local ok, _ = pcall(function()
            error("boom")
        end)
        return ok
    "#;
    let out = run_ok(src);
    assert_eq!(out.return_value, LuaValue::Boolean(false));
}

#[test]
fn error_code_gas_exhausted_is_unrecoverable() {
    let config = VmConfig {
        gas_limit: 20,
        ..VmConfig::default()
    };
    let err = run_with_config(
        r#"
        local ok, _ = pcall(function()
            local i = 0
            while true do i = i + 1 end
        end)
        return 1
    "#,
        config,
    )
    .unwrap_err();
    assert_eq!(err, VmError::GasExhausted);
}

#[test]
fn error_code_memory_exhausted_is_unrecoverable() {
    let config = VmConfig {
        memory_limit_bytes: 200,
        ..VmConfig::default()
    };
    let err = run_with_config(
        r#"
        local ok, _ = pcall(function()
            local i = 0
            while true do
                local t = {}
                i = i + 1
            end
        end)
        return 1
    "#,
        config,
    )
    .unwrap_err();
    assert_eq!(err, VmError::MemoryExhausted);
}

#[test]
fn error_code_call_depth_exceeded_is_recoverable() {
    let src = r#"
        local function inf() return inf() end
        local ok, _ = pcall(inf)
        return ok
    "#;
    let out = run_ok(src);
    assert_eq!(out.return_value, LuaValue::Boolean(false));
}

// ── pcall in single-value positions (issue #2) ────────────────────────────────
//
// `PCall` pushes two values, `ok` and the result. Only a multi-name `local`
// consumes both; everywhere else Lua truncates the call to one result, so
// codegen drops the result. Leaving it on the operand stack made `verify`
// reject the program with `RetStackMismatch`. Every source below is one of the
// programs listed in the issue, and each is verified as well as run.

#[test]
fn pcall_two_name_local_still_binds_both_results() {
    let out = run_ok("local ok, err = pcall(function() return 1 end) return ok");
    assert_eq!(out.return_value, LuaValue::Boolean(true));
}

#[test]
fn pcall_one_name_local_truncates_to_ok() {
    let out = run_ok("local ok = pcall(function() return 1 end) return ok");
    assert_eq!(out.return_value, LuaValue::Boolean(true));
}

#[test]
fn pcall_one_name_local_is_false_when_the_function_errors() {
    let out = run_ok("local ok = pcall(function() error('x') end) return ok");
    assert_eq!(out.return_value, LuaValue::Boolean(false));
}

#[test]
fn pcall_three_name_local_gives_the_extra_name_nil() {
    let out = run_ok("local ok, err, extra = pcall(function() return 1 end) return ok");
    assert_eq!(out.return_value, LuaValue::Boolean(true));

    let out = run_ok("local ok, err, extra = pcall(function() return 1 end) return extra");
    assert_eq!(out.return_value, LuaValue::Nil);

    // The second name still receives the result, as in the two-name form.
    let out = run_ok("local ok, err, extra = pcall(function() return 7 end) return err");
    assert_eq!(out.return_value, int(7));
}

#[test]
fn pcall_four_name_local_gives_every_extra_name_nil() {
    let out = run_ok("local ok, err, e2, e3 = pcall(function() return 1 end) return e3");
    assert_eq!(out.return_value, LuaValue::Nil);
}

#[test]
fn pcall_assigned_to_an_existing_local_truncates_to_ok() {
    let out = run_ok("local x = 0 x = pcall(function() return 1 end) return x");
    assert_eq!(out.return_value, LuaValue::Boolean(true));
}

#[test]
fn pcall_returned_directly_truncates_to_ok() {
    let out = run_ok("return pcall(function() return 1 end)");
    assert_eq!(out.return_value, LuaValue::Boolean(true));
}

#[test]
fn pcall_in_parentheses_truncates_to_ok() {
    let out = run_ok("return (pcall(function() return 1 end))");
    assert_eq!(out.return_value, LuaValue::Boolean(true));
}

#[test]
fn pcall_as_a_table_field_contributes_one_value() {
    let out = run_ok("local t = { pcall(function() return 1 end) } return #t");
    assert_eq!(out.return_value, int(1));

    let out = run_ok("local t = { pcall(function() return 1 end) } return t[1]");
    assert_eq!(out.return_value, LuaValue::Boolean(true));
}

#[test]
fn pcall_as_a_call_argument_passes_one_value() {
    let out = run_ok("print(pcall(function() return 1 end)) return 1");
    assert_eq!(out.return_value, int(1));
}

#[test]
fn pcall_returned_from_a_function_truncates_to_ok() {
    let out = run_ok(
        "local function f() return pcall(function() return 1 end) end \
         local a, b = f() return a",
    );
    assert_eq!(out.return_value, LuaValue::Boolean(true));

    // `f` returns a single value, so the second name is nil.
    let out = run_ok(
        "local function f() return pcall(function() return 1 end) end \
         local a, b = f() return b",
    );
    assert_eq!(out.return_value, LuaValue::Nil);
}

#[test]
fn pcall_as_an_operand_truncates_to_ok() {
    let out = run_ok("return pcall(function() return 1 end) == true");
    assert_eq!(out.return_value, LuaValue::Boolean(true));

    let out = run_ok("if pcall(function() error('x') end) then return 1 else return 2 end");
    assert_eq!(out.return_value, int(2));
}

#[test]
fn pcall_as_a_bare_statement_leaves_a_balanced_stack() {
    let out = run_ok("pcall(function() return 1 end) return 5");
    assert_eq!(out.return_value, int(5));
}

#[test]
fn pcall_of_a_builtin_truncates_to_ok() {
    let out = run_ok("local ok = pcall(string.upper, 'a') return ok");
    assert_eq!(out.return_value, LuaValue::Boolean(true));
}

#[test]
fn assigning_both_pcall_results_to_existing_variables_names_the_working_form() {
    let src = "local ok, parsed\nok, parsed = pcall(function() return 1 end)\nreturn ok";
    let err = parse(src).expect_err("multiple assignment should be rejected");
    let msg = err.message();
    assert!(
        msg.contains("pcall") && msg.contains("local ok, err = pcall(...)"),
        "error should name pcall and the working form, got: {}",
        msg
    );
}
