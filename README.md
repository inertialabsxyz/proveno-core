# proveno-core

The proveno runtime: a deterministic, sandboxed, metered Lua interpreter whose
every execution can be replayed bit-for-bit from its own transcript.

This crate is the foundation the rest of proveno is built on. It has no notion
of proving, policy, HTTP, X.509, Ethereum or LLMs — those live in
[proveno-zk](https://github.com/inertialabsxyz/proveno-zk) and
[proveno-agent](https://github.com/inertialabsxyz/proveno-agent). For the
project overview, start at the [umbrella](https://github.com/inertialabsxyz/proveno).

```rust
use proveno::{bytecode, compiler, parser, vm::engine::{NoopHost, Vm, VmConfig}};
use proveno::types::value::LuaValue;

let ast     = parser::parse("local function f(n) if n <= 1 then return 1 end return n * f(n-1) end return f(5)")?;
let program = compiler::compile(&ast)?;
bytecode::verify(&program)?;

let mut vm = Vm::new(VmConfig::default(), NoopHost);
let out    = vm.execute(&program, LuaValue::Nil)?;
// out.return_value == 120, out.gas_used, out.memory_used, out.transcript
```

## What it gives you

- **Determinism.** No floats, no randomized iteration, no clocks, no ambient
  I/O. `require`, `os` and `io` are rejected at parse time. Given the same
  program and the same sequence of host responses, execution is byte-identical.
- **Metering.** Gas and memory are charged on every instruction and allocation.
  Exhaustion is a `VmError`, never a panic, and never a hang.
- **A recorded transcript.** Every tool call is captured with its canonical
  arguments and response, so a run can be replayed with the network switched
  off — which is what makes proving possible, and what makes an execution
  auditable even without a proof.
- **One canonical encoding.** `canonical_serialize()` produces byte-for-byte
  reproducible JSON, and it is the same path `json.encode` uses, so what you
  hash is what the program saw.

Tool calls are the only side-effecting primitive. You supply the host:

```rust
pub trait HostInterface {
    fn call_tool(&mut self, name: &str, args: &LuaTable) -> Result<LuaTable, String>;
    fn take_attestation(&mut self) -> Option<Vec<u8>> { None }
}
```

## `no_std`

`default = ["std", "poseidon"]`. With no default features the crate is
`no_std` + `alloc` and its dependency tree is 22 crates, which is what the zkVM
guest compiles against. Turning `poseidon` off is load-bearing there: it pulls
cranelift, whose build script panics on custom RISC-V target triples.

## Development

```bash
make check      # the gate: fmt, clippy -D warnings, tests, no_std builds
make test-unit  # fast inner loop
make help
```

See [CLAUDE.md](CLAUDE.md) for the module map, the calling convention, the
determinism invariants and the known gaps.

## Licence

See [LICENSE](LICENSE).
