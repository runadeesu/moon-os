//! A real, from-scratch WebAssembly interpreter -- a genuine, bounded MVP
//! subset: parses the actual WASM binary format (magic/version, the
//! Type/Function/Export/Code sections, real LEB128 varints) and executes
//! a real stack-machine instruction set (i32 arithmetic/comparisons,
//! locals, and structured control flow -- block/loop/if/else/br/br_if/
//! return) well enough to run a real, unmodified `.wasm` module produced
//! by a genuine toolchain. This module's self-test uses one actually
//! compiled by `clang --target=wasm32 -nostdlib`, and its expected
//! results were cross-checked against Node.js's native WebAssembly engine
//! running those exact same bytes before being trusted here.
//!
//! Deliberately not attempted, and it matters: any numeric type besides
//! i32 (no i64/f32/f64 -- this kernel has no float support at all, see
//! `gui::desktop`'s doc comment on why), `call`/`call_indirect` (so an
//! exported function that calls another function in the same module
//! won't run -- a real next slice, not attempted here), linear memory
//! (`memory.load`/`memory.store`, so nothing that touches `memory`
//! works), multi-value blocks, or anything resembling WASI or a host
//! import ABI. That's not "nearly a complete engine" -- it's a small
//! fraction of one; this is a first, honest slice, the same way
//! `pe.rs`/`elf.rs` started with a small import/relocation subset instead
//! of a complete loader.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

const MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];

fn read_uleb128(data: &[u8], pos: &mut usize) -> Option<u64> {
    let mut result = 0u64;
    let mut shift = 0u32;
    loop {
        let byte = *data.get(*pos)?;
        *pos += 1;
        result |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some(result);
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

fn read_sleb128(data: &[u8], pos: &mut usize) -> Option<i64> {
    let mut result: i64 = 0;
    let mut shift = 0u32;
    loop {
        let byte = *data.get(*pos)?;
        *pos += 1;
        result |= i64::from(byte & 0x7F) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            if shift < 64 && (byte & 0x40) != 0 {
                result |= -(1i64 << shift);
            }
            return Some(result);
        }
        if shift >= 64 {
            return None;
        }
    }
}

/// A function type's arity -- only param/result *counts* are kept since
/// every value in this interpreter is i32 (the one numeric type it
/// supports), so the types themselves would all be identical anyway.
struct FuncType {
    params: usize,
    results: usize,
}

pub struct Module {
    types: Vec<FuncType>,
    func_type_indices: Vec<u32>,
    /// One entry per defined function, in module order: its declared
    /// (non-parameter) locals and its raw instruction bytes.
    codes: Vec<(Vec<()>, Vec<u8>)>,
    exports: BTreeMap<String, u32>,
}

/// Parses a WASM binary module. Only the sections this interpreter needs
/// are actually decoded (Type/Function/Export/Code); anything else
/// (Import, Table, Memory, Global, Start, Element, Data, Custom) is
/// skipped over using the section's own declared length -- this parser
/// doesn't need to understand them to still execute an import-free,
/// memory-free exported function correctly.
pub fn parse(data: &[u8]) -> Result<Module, &'static str> {
    if data.len() < 8 || data[0..4] != MAGIC {
        return Err("not a WASM module (bad magic)");
    }
    let version = u32::from_le_bytes(data[4..8].try_into().unwrap());
    if version != 1 {
        return Err("unsupported WASM version (only the MVP binary version 1)");
    }

    let mut pos = 8usize;
    let mut types = Vec::new();
    let mut func_type_indices = Vec::new();
    let mut exports = BTreeMap::new();
    let mut codes: Vec<(Vec<()>, Vec<u8>)> = Vec::new();

    while pos < data.len() {
        let section_id = data[pos];
        pos += 1;
        let section_len =
            read_uleb128(data, &mut pos).ok_or("truncated section header")? as usize;
        let section_end = pos
            .checked_add(section_len)
            .filter(|&e| e <= data.len())
            .ok_or("section length out of bounds")?;

        match section_id {
            1 => {
                // Type section.
                let count = read_uleb128(data, &mut pos).ok_or("truncated type section")?;
                for _ in 0..count {
                    let form = *data.get(pos).ok_or("truncated functype")?;
                    pos += 1;
                    if form != 0x60 {
                        return Err("unsupported type form (only `func` types)");
                    }
                    let param_count =
                        read_uleb128(data, &mut pos).ok_or("truncated functype params")? as usize;
                    for _ in 0..param_count {
                        let vt = *data.get(pos).ok_or("truncated functype param type")?;
                        pos += 1;
                        if vt != 0x7F {
                            return Err("only i32-typed params/results are supported");
                        }
                    }
                    let result_count =
                        read_uleb128(data, &mut pos).ok_or("truncated functype results")? as usize;
                    for _ in 0..result_count {
                        let vt = *data.get(pos).ok_or("truncated functype result type")?;
                        pos += 1;
                        if vt != 0x7F {
                            return Err("only i32-typed params/results are supported");
                        }
                    }
                    types.push(FuncType {
                        params: param_count,
                        results: result_count,
                    });
                }
            }
            3 => {
                // Function section.
                let count = read_uleb128(data, &mut pos).ok_or("truncated function section")?;
                for _ in 0..count {
                    let idx = read_uleb128(data, &mut pos).ok_or("truncated function index")?;
                    func_type_indices.push(idx as u32);
                }
            }
            7 => {
                // Export section.
                let count = read_uleb128(data, &mut pos).ok_or("truncated export section")?;
                for _ in 0..count {
                    let name_len =
                        read_uleb128(data, &mut pos).ok_or("truncated export name")? as usize;
                    let name_bytes = data
                        .get(pos..pos + name_len)
                        .ok_or("export name out of bounds")?;
                    let name =
                        core::str::from_utf8(name_bytes).map_err(|_| "export name is not UTF-8")?;
                    pos += name_len;
                    let kind = *data.get(pos).ok_or("truncated export")?;
                    pos += 1;
                    let idx = read_uleb128(data, &mut pos).ok_or("truncated export index")?;
                    if kind == 0 {
                        exports.insert(String::from(name), idx as u32);
                    }
                }
            }
            10 => {
                // Code section.
                let count = read_uleb128(data, &mut pos).ok_or("truncated code section")?;
                for _ in 0..count {
                    let body_len =
                        read_uleb128(data, &mut pos).ok_or("truncated function body")? as usize;
                    let body_end = pos + body_len;
                    if body_end > data.len() {
                        return Err("function body out of bounds");
                    }
                    let mut p = pos;
                    let local_decl_count =
                        read_uleb128(data, &mut p).ok_or("truncated locals declaration")?;
                    let mut local_count = 0usize;
                    for _ in 0..local_decl_count {
                        let n = read_uleb128(data, &mut p).ok_or("truncated locals declaration")?;
                        let vt = *data.get(p).ok_or("truncated locals declaration")?;
                        p += 1;
                        if vt != 0x7F {
                            return Err("only i32 locals are supported");
                        }
                        local_count += n as usize;
                    }
                    codes.push((alloc::vec![(); local_count], data[p..body_end].to_vec()));
                    pos = body_end;
                }
            }
            _ => {} // Skipped: this interpreter's scope doesn't need it.
        }
        pos = section_end;
    }

    Ok(Module {
        types,
        func_type_indices,
        codes,
        exports,
    })
}

impl Module {
    /// Calls exported function `name` with `args`, requiring it take
    /// exactly `args.len()` i32 parameters and return exactly one i32 --
    /// the one calling convention this interpreter supports.
    pub fn call(&self, name: &str, args: &[i32]) -> Result<i32, &'static str> {
        let func_idx = *self.exports.get(name).ok_or("no such export")?;
        let type_idx = *self
            .func_type_indices
            .get(func_idx as usize)
            .ok_or("bad function index")?;
        let func_type = self.types.get(type_idx as usize).ok_or("bad type index")?;
        if args.len() != func_type.params {
            return Err("wrong argument count for this export");
        }
        if func_type.results != 1 {
            return Err("only single-i32-result exports are supported by this entry point");
        }
        run_function(self, func_idx, args)
    }
}

#[derive(Clone, Copy)]
enum Instr {
    Unreachable,
    Nop,
    Block,
    Loop,
    If,
    Else,
    End,
    Br(u32),
    BrIf(u32),
    Return,
    Drop,
    LocalGet(u32),
    LocalSet(u32),
    LocalTee(u32),
    I32Const(i32),
    I32Eqz,
    I32Eq,
    I32Ne,
    I32LtS,
    I32LtU,
    I32GtS,
    I32GtU,
    I32LeS,
    I32LeU,
    I32GeS,
    I32GeU,
    I32Add,
    I32Sub,
    I32Mul,
    I32DivS,
    I32DivU,
    I32RemS,
    I32RemU,
    I32And,
    I32Or,
    I32Xor,
    I32Shl,
    I32ShrS,
    I32ShrU,
}

/// Decodes one instruction at `*pc`, advancing it past the opcode and any
/// immediate operand. Shared by both the control-flow pre-pass
/// (`build_control_map`) and actual execution (`run_function`) so the two
/// can never disagree about how long an instruction is.
fn decode(code: &[u8], pc: &mut usize) -> Result<Instr, &'static str> {
    let op = *code.get(*pc).ok_or("truncated instruction stream")?;
    *pc += 1;
    Ok(match op {
        0x00 => Instr::Unreachable,
        0x01 => Instr::Nop,
        0x02 => {
            *pc += 1; // block type byte (only the single-byte 0x40/valtype form is supported)
            Instr::Block
        }
        0x03 => {
            *pc += 1;
            Instr::Loop
        }
        0x04 => {
            *pc += 1;
            Instr::If
        }
        0x05 => Instr::Else,
        0x0B => Instr::End,
        0x0C => Instr::Br(read_uleb128(code, pc).ok_or("truncated br")? as u32),
        0x0D => Instr::BrIf(read_uleb128(code, pc).ok_or("truncated br_if")? as u32),
        0x0F => Instr::Return,
        0x1A => Instr::Drop,
        0x20 => Instr::LocalGet(read_uleb128(code, pc).ok_or("truncated local.get")? as u32),
        0x21 => Instr::LocalSet(read_uleb128(code, pc).ok_or("truncated local.set")? as u32),
        0x22 => Instr::LocalTee(read_uleb128(code, pc).ok_or("truncated local.tee")? as u32),
        0x41 => Instr::I32Const(read_sleb128(code, pc).ok_or("truncated i32.const")? as i32),
        0x45 => Instr::I32Eqz,
        0x46 => Instr::I32Eq,
        0x47 => Instr::I32Ne,
        0x48 => Instr::I32LtS,
        0x49 => Instr::I32LtU,
        0x4A => Instr::I32GtS,
        0x4B => Instr::I32GtU,
        0x4C => Instr::I32LeS,
        0x4D => Instr::I32LeU,
        0x4E => Instr::I32GeS,
        0x4F => Instr::I32GeU,
        0x6A => Instr::I32Add,
        0x6B => Instr::I32Sub,
        0x6C => Instr::I32Mul,
        0x6D => Instr::I32DivS,
        0x6E => Instr::I32DivU,
        0x6F => Instr::I32RemS,
        0x70 => Instr::I32RemU,
        0x71 => Instr::I32And,
        0x72 => Instr::I32Or,
        0x73 => Instr::I32Xor,
        0x74 => Instr::I32Shl,
        0x75 => Instr::I32ShrS,
        0x76 => Instr::I32ShrU,
        _ => return Err("unsupported WASM opcode (this is a bounded MVP subset, not a full engine)"),
    })
}

struct ControlInfo {
    end_pc: usize,
    else_pc: Option<usize>,
}

/// Pre-scans `code`, matching every `block`/`loop`/`if` with its `end`
/// (and `if` with its `else`, if any) -- WASM branches always target one
/// of these enclosing structured constructs by relative depth, never an
/// arbitrary address, so resolving them all up front once turns `br`/
/// `br_if` into cheap table lookups instead of a scan at branch time.
fn build_control_map(code: &[u8]) -> Result<BTreeMap<usize, ControlInfo>, &'static str> {
    let mut map = BTreeMap::new();
    let mut open: Vec<usize> = Vec::new();
    let mut pc = 0usize;
    while pc < code.len() {
        let op_pc = pc;
        match decode(code, &mut pc)? {
            Instr::Block | Instr::Loop | Instr::If => open.push(op_pc),
            Instr::Else => {
                let if_pc = *open.last().ok_or("`else` without a matching `if`")?;
                map.entry(if_pc)
                    .or_insert(ControlInfo {
                        end_pc: 0,
                        else_pc: None,
                    })
                    .else_pc = Some(op_pc);
            }
            Instr::End => {
                if let Some(start_pc) = open.pop() {
                    let else_pc = map.get(&start_pc).and_then(|i| i.else_pc);
                    map.insert(
                        start_pc,
                        ControlInfo {
                            end_pc: op_pc,
                            else_pc,
                        },
                    );
                    if let Some(else_pc) = else_pc {
                        // So `Instr::Else` reached by falling through a
                        // taken `if` branch can look up where to skip to.
                        map.insert(
                            else_pc,
                            ControlInfo {
                                end_pc: op_pc,
                                else_pc: None,
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }
    Ok(map)
}

enum Frame {
    Block { end_pc: usize },
    Loop { start_pc: usize },
    If { end_pc: usize },
}

/// Resolves a `br`/`br_if` of relative `depth`. Returns `Ok(true)` when
/// the branch targets past every open block (i.e. depth equals the
/// number of open frames) -- WASM treats branching out of the function's
/// own implicit outermost block as a `return`.
fn branch(control_stack: &mut Vec<Frame>, depth: usize, pc: &mut usize) -> Result<bool, &'static str> {
    if depth == control_stack.len() {
        return Ok(true);
    }
    if depth > control_stack.len() {
        return Err("branch depth exceeds the open control-flow stack (malformed module)");
    }
    let target_idx = control_stack.len() - 1 - depth;
    match control_stack[target_idx] {
        Frame::Loop { start_pc } => {
            *pc = start_pc;
            control_stack.truncate(target_idx + 1);
        }
        Frame::Block { end_pc } | Frame::If { end_pc } => {
            *pc = end_pc + 1;
            control_stack.truncate(target_idx);
        }
    }
    Ok(false)
}

fn pop(stack: &mut Vec<i32>) -> Result<i32, &'static str> {
    stack.pop().ok_or("value stack underflow (malformed module)")
}

fn binop(stack: &mut Vec<i32>, f: impl FnOnce(i32, i32) -> i32) -> Result<(), &'static str> {
    let b = pop(stack)?;
    let a = pop(stack)?;
    stack.push(f(a, b));
    Ok(())
}

fn run_function(module: &Module, func_idx: u32, args: &[i32]) -> Result<i32, &'static str> {
    let type_idx = module.func_type_indices[func_idx as usize];
    let param_count = module.types[type_idx as usize].params;
    let (declared_locals, code) = &module.codes[func_idx as usize];

    let mut locals: Vec<i32> = Vec::with_capacity(param_count + declared_locals.len());
    locals.extend_from_slice(args);
    locals.extend(core::iter::repeat_n(0, declared_locals.len()));

    let control_map = build_control_map(code)?;
    let mut value_stack: Vec<i32> = Vec::new();
    let mut control_stack: Vec<Frame> = Vec::new();
    let mut pc = 0usize;

    while pc < code.len() {
        let op_pc = pc;
        let instr = decode(code, &mut pc)?;
        match instr {
            Instr::Unreachable => return Err("hit an `unreachable` instruction (a real trap)"),
            Instr::Nop => {}
            Instr::Drop => {
                pop(&mut value_stack)?;
            }
            Instr::Block => {
                let end_pc = control_map
                    .get(&op_pc)
                    .ok_or("internal: missing control info for block")?
                    .end_pc;
                control_stack.push(Frame::Block { end_pc });
            }
            Instr::Loop => control_stack.push(Frame::Loop { start_pc: pc }),
            Instr::If => {
                let cond = pop(&mut value_stack)?;
                let info = control_map
                    .get(&op_pc)
                    .ok_or("internal: missing control info for if")?;
                if cond != 0 {
                    control_stack.push(Frame::If { end_pc: info.end_pc });
                } else if let Some(else_pc) = info.else_pc {
                    control_stack.push(Frame::If { end_pc: info.end_pc });
                    pc = else_pc + 1;
                } else {
                    pc = info.end_pc + 1;
                }
            }
            Instr::Else => {
                let end_pc = control_map
                    .get(&op_pc)
                    .ok_or("internal: missing control info for else")?
                    .end_pc;
                pc = end_pc + 1;
                control_stack.pop();
            }
            Instr::End => {
                control_stack.pop();
            }
            Instr::Br(depth) => {
                if branch(&mut control_stack, depth as usize, &mut pc)? {
                    break;
                }
            }
            Instr::BrIf(depth) => {
                let cond = pop(&mut value_stack)?;
                if cond != 0 && branch(&mut control_stack, depth as usize, &mut pc)? {
                    break;
                }
            }
            Instr::Return => break,
            Instr::LocalGet(idx) => {
                value_stack.push(*locals.get(idx as usize).ok_or("bad local index")?);
            }
            Instr::LocalSet(idx) => {
                let v = pop(&mut value_stack)?;
                *locals.get_mut(idx as usize).ok_or("bad local index")? = v;
            }
            Instr::LocalTee(idx) => {
                let v = *value_stack.last().ok_or("local.tee with empty stack")?;
                *locals.get_mut(idx as usize).ok_or("bad local index")? = v;
            }
            Instr::I32Const(v) => value_stack.push(v),
            Instr::I32Eqz => {
                let a = pop(&mut value_stack)?;
                value_stack.push(i32::from(a == 0));
            }
            Instr::I32Eq => binop(&mut value_stack, |a, b| i32::from(a == b))?,
            Instr::I32Ne => binop(&mut value_stack, |a, b| i32::from(a != b))?,
            Instr::I32LtS => binop(&mut value_stack, |a, b| i32::from(a < b))?,
            Instr::I32LtU => binop(&mut value_stack, |a, b| i32::from((a as u32) < (b as u32)))?,
            Instr::I32GtS => binop(&mut value_stack, |a, b| i32::from(a > b))?,
            Instr::I32GtU => binop(&mut value_stack, |a, b| i32::from((a as u32) > (b as u32)))?,
            Instr::I32LeS => binop(&mut value_stack, |a, b| i32::from(a <= b))?,
            Instr::I32LeU => binop(&mut value_stack, |a, b| i32::from((a as u32) <= (b as u32)))?,
            Instr::I32GeS => binop(&mut value_stack, |a, b| i32::from(a >= b))?,
            Instr::I32GeU => binop(&mut value_stack, |a, b| i32::from((a as u32) >= (b as u32)))?,
            Instr::I32Add => binop(&mut value_stack, |a, b| a.wrapping_add(b))?,
            Instr::I32Sub => binop(&mut value_stack, |a, b| a.wrapping_sub(b))?,
            Instr::I32Mul => binop(&mut value_stack, |a, b| a.wrapping_mul(b))?,
            Instr::I32DivS => {
                let b = pop(&mut value_stack)?;
                let a = pop(&mut value_stack)?;
                if b == 0 || (a == i32::MIN && b == -1) {
                    return Err("integer divide trap (division by zero or overflow)");
                }
                value_stack.push(a / b);
            }
            Instr::I32DivU => {
                let b = pop(&mut value_stack)?;
                let a = pop(&mut value_stack)?;
                if b == 0 {
                    return Err("integer divide trap (division by zero)");
                }
                value_stack.push(((a as u32) / (b as u32)) as i32);
            }
            Instr::I32RemS => {
                let b = pop(&mut value_stack)?;
                let a = pop(&mut value_stack)?;
                if b == 0 {
                    return Err("integer divide trap (remainder by zero)");
                }
                value_stack.push(if b == -1 { 0 } else { a % b });
            }
            Instr::I32RemU => {
                let b = pop(&mut value_stack)?;
                let a = pop(&mut value_stack)?;
                if b == 0 {
                    return Err("integer divide trap (remainder by zero)");
                }
                value_stack.push(((a as u32) % (b as u32)) as i32);
            }
            Instr::I32And => binop(&mut value_stack, |a, b| a & b)?,
            Instr::I32Or => binop(&mut value_stack, |a, b| a | b)?,
            Instr::I32Xor => binop(&mut value_stack, |a, b| a ^ b)?,
            Instr::I32Shl => binop(&mut value_stack, |a, b| a.wrapping_shl(b as u32 & 31))?,
            Instr::I32ShrS => binop(&mut value_stack, |a, b| a.wrapping_shr(b as u32 & 31))?,
            Instr::I32ShrU => {
                binop(&mut value_stack, |a, b| {
                    ((a as u32).wrapping_shr(b as u32 & 31)) as i32
                })?;
            }
        }
    }

    pop(&mut value_stack)
}

/// Cross-verified: `wasmtest.wasm` was compiled with a genuine toolchain
/// (`clang --target=wasm32 -O1 -nostdlib -Wl,--no-entry
/// -Wl,--export=add -Wl,--export=fib`) from real C source (`add`, and an
/// iterative `fib` with an early-return `if` -- exercising locals,
/// `block`/`br_if`, `loop`/`br_if`, and `local.tee`), and every expected
/// result below was captured by running those *exact* bytes through
/// Node.js's native WebAssembly engine before this test was written, an
/// independent implementation this interpreter has no code in common
/// with.
pub fn self_test() {
    let bytes = crate::crypto::testhex::hex_vec(concat!(
        "0061736d01000000010c0260027f7f017f60017f017f030302000105030100020608017f01418088040b071603066d65",
        "6d6f727902000361646400000366696200010a4f020700200120006a0b4501057f0240200041024e0d0020000f0b2000",
        "417f6a2100410121014100210203402000417f6a220321002001220420026a22052101200421022005210420030d000b",
        "20040b0036046e616d65000e0d7761736d746573742e7761736d010b0200036164640103666962071201000f5f5f7374",
        "61636b5f706f696e74657200380970726f647563657273010c70726f6365737365642d6279010c5562756e747520636c",
        "616e671131382e312e332028317562756e74753129002c0f7461726765745f6665617475726573022b0f6d757461626c",
        "652d676c6f62616c732b087369676e2d657874",
    ));

    let module = parse(&bytes).expect("wasm: self-test module failed to parse");

    assert_eq!(module.call("add", &[3, 4]), Ok(7), "wasm: add(3,4)");
    assert_eq!(module.call("add", &[100, 200]), Ok(300), "wasm: add(100,200)");
    assert_eq!(module.call("fib", &[0]), Ok(0), "wasm: fib(0)");
    assert_eq!(module.call("fib", &[1]), Ok(1), "wasm: fib(1)");
    assert_eq!(module.call("fib", &[10]), Ok(55), "wasm: fib(10)");
    assert_eq!(module.call("fib", &[20]), Ok(6765), "wasm: fib(20)");

    crate::serial_println!(
        "wasm: self-test passed (real clang-compiled module, results match Node.js's WebAssembly engine)"
    );
}
