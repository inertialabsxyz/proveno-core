# CLAUDE.md

Guidance for Claude Code working in **proveno-core**.

## What this repository is

The proveno runtime, and nothing else: parser, compiler, bytecode verifier, VM,
host, ISA, and the record/replay machinery. It is `no_std`-capable and knows
nothing about policy, HTTP, X.509, Ethereum or LLMs.

If a change touches proving, policy or provenance, it does not belong here.

| Repository | Scope |
|---|---|
| **proveno-core** (here) | Runtime: parser, compiler, bytecode, types, vm, host, isa |
| [proveno-zk](https://github.com/inertialabsxyz/proveno-zk) | Policy, commitments, Noir circuit, OpenVM guest, contracts |
| [proveno-agent](https://github.com/inertialabsxyz/proveno-agent) | LLM orchestrator, demo server, TLS provenance |
| [proveno](https://github.com/inertialabsxyz/proveno) | Umbrella: project overview, architecture, trust model |

Dependencies point inward, by git tag. Nothing here may depend on the other
three. The [architecture document](https://github.com/inertialabsxyz/proveno/blob/main/docs/architecture.md)
in the umbrella is the tie-breaker when documents disagree.

## Quality Gate

```bash
make check
```

Must pass before every commit. Runs `lint` (fmt + clippy `-D warnings`), `test`
and `test-nostd`.

`cargo test` alone is acceptable while iterating, but it does not cover the
`no_std` / no-poseidon configurations that `test-nostd` builds, and those are
what the zkVM guest compiles against.

Note `make lint` runs plain `cargo clippy`, not `--all-targets`, so lints inside
`tests/*.rs` are not gated.

There is no `test-prove` here. The Noir prove/verify pipeline lives in
proveno-zk, and it is **not** a hard gate on this repository (decided September
2026): OpenVM is the proving path, and its guest runs this interpreter as
ordinary Rust, so a new builtin or opcode needs no circuit work. The Noir
circuit executes bytecode in-circuit, so it lags until someone needs it.

Still run `make test-prove` in proveno-zk, and report the result, for a change
to canonical serialization, the oracle tape or the program hash: those are what
both proving paths recompute, and a break there is a break everywhere. A change
that only adds a builtin or an opcode may leave Noir behind; say so in the PR
body so nobody assumes the circuit kept up.

## Common Commands

```bash
cargo build                          # debug
cargo test                           # all tests
cargo test --lib                     # unit tests only (fast)
cargo test --lib engine              # filter by name within unit tests
cargo test --test integration        # one integration file
                                     # (also: builtins, compiler, json, tools, isa_trace)

cargo run -p proveno-compiler -- source.lua compiled.json
cargo run --example repl -- source.lua     # smoke-test REPL with a toy host

make test-nostd                      # the no_std / no-poseidon configurations
```

## Features

`default = ["std", "poseidon"]`, plus optional `serde`.

| Feature | Role |
|---|---|
| `std` | Standard library. Off = `no_std` + `alloc`. |
| `poseidon` | BN254 Poseidon2, byte-identical to the Noir circuit. |
| `serde` | Serialization of `CompiledProgram`, `VmOutput`, `OracleTape`. |

**`poseidon` must stay in the default set.** `compiler::codegen` selects the
program-hash scheme on it, so removing it would silently switch every committed
`program_hash` from Poseidon2 to SHA-256, with no compile error. Pinned by
`poseidon_hash_matches_recorded_fixture` in `src/compiler/program_hash.rs`.

**`poseidon` must be off for zkVM guest builds.** It pulls
`bn254_blackbox_solver` → wasmer → cranelift → target-lexicon, whose build
script hard-panics on custom RISC-V target triples, and cargo runs that build
script whether or not the code is linked. The tree goes from 245 crates to 22.

## Resource limits

`VmConfig::default()`, in `src/vm/engine.rs`:

| Limit | Field | Default |
|---|---|---|
| Gas | `gas_limit` | 200,000 |
| Memory | `memory_limit_bytes` | 16 MiB |
| Call depth | `max_call_depth` | 64 |
| Tool calls | `max_tool_calls` | 16 |
| Bytes in, all calls | `max_tool_bytes_in` | 64 KiB |
| Bytes out, all calls | `max_tool_bytes_out` | 1 MiB |
| Return value size | `max_output_bytes` | 256 KiB |

Constants, not configurable:

| Limit | Where | Value |
|---|---|---|
| String length | `builtins::MAX_STRING_LEN` | 64 KiB |
| String length, canonical JSON | `canonicalize::MAX_STRING_LEN` | 1 MiB |
| Table nesting depth | `MAX_TABLE_DEPTH` (both) | 32 |
| Table entries | `types::value::MAX_TABLE_ENTRIES` | 50,000 |

The byte quotas are cumulative across all tool calls in a run, not per call.

> Every number in this table was wrong in the pre-split CLAUDE.md, with
> bytes-in and bytes-out additionally transposed. Check `VmConfig::default()`
> before trusting a restatement of it.

## Execution pipeline

```
Lua source
    → parser/          (lexer + recursive-descent → AST)
    → compiler/        (AST → register-based bytecode + constants + program hash)
    → isa/             (opcode numbering + TraceStep: the VM's own encoding)
    → bytecode/verifier.rs  (validates stack depth, branch targets)
    → vm/engine.rs     (instruction dispatch loop, gas + memory metering)
    → host/            (tool calls, transcript, oracle tape)
```

## Key modules in `src/`

- **`parser/`** — Lexer and recursive-descent parser. Rejects `require`, `os`,
  `io` and other unsafe constructs at parse time. `tool.call()` is a first-class
  syntax node; indirect use is a compile error.
- **`compiler/`** — `codegen.rs` compiles AST to `Instruction` bytecode.
  `proto.rs` defines `FunctionProto`, `Instruction`, `Constant`.
  `program_hash.rs` is the canonical definition of the program hash, in both the
  Poseidon2 and SHA-256 schemes.
- **`isa/`** — `opcodes.rs` (opcode IDs) and `trace.rs` (`TraceStep`). The VM's
  instruction encoding, not a prover concern, despite the Noir circuit being its
  first consumer.
- **`bytecode/`** — `verifier.rs` performs a single-pass stack-depth check
  across all control-flow paths before any instruction executes.
- **`vm/engine.rs`** — `Vm` and the dispatch loop. Owns the `CallFrame` stack,
  resolves builtins, dispatches `ToolCall` through `ToolRegistry`.
- **`vm/builtins.rs`** — the standard library (`string.*`, `math.*`, `table.*`,
  `json.*`, `pcall`, `type`, `pairs_sorted`, `ipairs`, `log`, `print`).
  `string.find` accepts standard Lua's fourth `plain` argument: with
  `plain = true` the needle is literal, as in `string.find_literal`; without it
  a pattern metacharacter is still refused, and `match`/`gmatch`/`gsub` remain
  unsupported.
  `pcall` yields `ok` alone in every single-value position (a one-name `local`,
  an assignment, a `return`, an argument, a table field, parentheses, an
  operand), and `ok, result` only in a multi-name `local`, where names beyond
  the second get `nil`. Assigning both results to variables that already exist,
  `ok, err = pcall(f)`, is not supported: multiple assignment does not parse,
  and the parser says so by name.
  `string.format` takes `%d`, `%x` and `%s` with flags `-` and `0`, a width and
  a precision (two digits each), and refuses `%f`/`%e`/`%g`, pointing at
  decimal strings, because there are no floats.
- **`vm/gas.rs` + `vm/memory.rs`** — `GasMeter` and `MemoryMeter`. Exhaustion
  raises `VmError`, never panics.
- **`types/value.rs`** — `LuaValue` (`Nil | Boolean | Integer | LuaString |
  Table | Closure | Builtin`). No floats.
- **`types/table.rs`** — integer array section plus string/integer hash section.
  `rawset_tracked()` returns `RawsetResult` for memory accounting.
- **`host/`** — `ToolRegistry<H>` enforces per-call quotas and records a
  `Transcript`. `OracleTape` / `TapeHost` enable deterministic replay.
  `TapeHost::strict` reports the first call that differs from the tape via
  `divergence()`; `Vm::{transcript, gas_used, memory_used, host}` survive `Err`.
  `canonical_serialize()` is the one reproducible JSON encoding.
  **Knows nothing about policy**: enforcement is a host wrapper, supplied by
  proveno-zk.

## Calling convention (important for compiler + VM work)

- Parameters occupy local slots `0..param_count`. Register them with
  `register_param()`, not `declare_local()`.
- Non-parameter locals start at slot `param_count` via `declare_local()`.
- The verifier expects operand-stack depth 0 at function entry: params are in
  slots, not on the operand stack.
- Jump offsets are relative to the instruction **after** the jump; `pc` has
  already incremented when `jump_by()` is called.

## Determinism invariants

No floats. No randomized hash iteration. No time-dependent calls.
`pairs_sorted` / `IterInitSorted` iterate in canonical key order.
`canonical_serialize()` is the single JSON encoding path used for hashing.

These are load-bearing for ZK proof soundness, not style preferences. Any change
to the VM, compiler, host serialization or oracle tape **must** include a test
pinning the property it affects: identical hashes on replay, byte-identical
canonical JSON, sorted iteration order.

## Known gap: the Poseidon2 program hash does not cover constants

`compute_program_hash` hashes only the `(opcode, operand)` stream. `PushK`,
`GetField` and `SetField` carry a constant-pool *index*, so `return 1`,
`return 2` and `return "omega"` hash identically. A prover can swap every
literal in a program and still match a committed `program_hash`.

`compute_program_hash_sha256` does not have this gap: it covers the constant
pool, upvalue descriptors and prototype metadata. Closing it on the Noir side
means changing `assert_bytecode` in proveno-zk's `noir/src/main.nr` in lockstep,
which invalidates existing verification keys.

The gap is open and **not** pinned by a test. Earlier revisions of this file
claimed a `poseidon_program_hash_does_not_cover_constants_known_gap` test; it
has never existed.
