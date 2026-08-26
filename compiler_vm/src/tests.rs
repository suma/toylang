//! Unit tests for the IR VM, moved verbatim from the interpreter's
//! `ir_vm` module (COMPILE-TIME-EVAL C6). The heap-dependent ones run
//! against [`TestHost`], a minimal bump-heap host that mirrors the
//! interpreter's byte layout — the VM's contract with its host.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use compiler_ir::{Const, Instruction, Linkage, Module, Terminator, Type, ValueId};
use string_interner::DefaultStringInterner;

use super::*;

/// A host for unit tests: a bump heap with the interpreter's byte
/// layout, output discarded.
struct TestHost {
    heap: RefCell<Vec<u8>>,
    next: Cell<usize>,
    blocks: RefCell<HashMap<usize, usize>>,
    typed: RefCell<HashMap<(usize, usize), (RawSlot, Type)>>,
}

impl TestHost {
    fn new() -> Self {
        Self {
            heap: RefCell::new(Vec::new()),
            next: Cell::new(1), // 0 is reserved for the null pointer
            blocks: RefCell::new(HashMap::new()),
            typed: RefCell::new(HashMap::new()),
        }
    }
}

fn scalar_byte_width(ty: Type) -> Option<usize> {
    match ty {
        Type::I8 | Type::U8 | Type::Bool => Some(1),
        Type::I16 | Type::U16 => Some(2),
        Type::I32 | Type::U32 => Some(4),
        Type::I64 | Type::U64 | Type::F64 => Some(8),
        _ => None,
    }
}

fn byte_value_to_slot(raw: u64, ty: Type) -> RawSlot {
    match ty {
        Type::I8 => RawSlot::from_i64(raw as u8 as i8 as i64),
        Type::I16 => RawSlot::from_i64(raw as u16 as i16 as i64),
        Type::I32 => RawSlot::from_i64(raw as u32 as i32 as i64),
        Type::I64 => RawSlot::from_i64(raw as i64),
        Type::U8 | Type::U16 | Type::U32 | Type::U64 => RawSlot::from_u64(raw),
        Type::F64 => RawSlot::from_f64(f64::from_bits(raw)),
        Type::Bool => RawSlot::from_bool(raw != 0),
        _ => RawSlot::from_u64(raw),
    }
}

impl VmHost for TestHost {
    fn print_text(&self, _text: &str) {}

    fn println_text(&self, _text: &str) {}

    fn alloc_push(&self, _handle: u64) {}

    fn alloc_pop(&self) {}

    fn alloc_current(&self) -> u64 {
        0
    }

    fn alloc_at(&self, size: u64, _site: u64) -> u64 {
        if size == 0 {
            return 0;
        }
        let addr = self.next.get();
        self.heap.borrow_mut().resize(addr + size as usize, 0);
        self.blocks.borrow_mut().insert(addr, size as usize);
        self.next.set(addr + size as usize);
        addr as u64
    }

    fn realloc(&self, ptr: u64, new_size: u64) -> u64 {
        if ptr == 0 {
            return self.alloc_at(new_size, 0);
        }
        if new_size == 0 {
            return 0;
        }
        let old = self.blocks.borrow().get(&(ptr as usize)).copied().unwrap_or(0);
        let addr = self.alloc_at(new_size, 0);
        let copy = old.min(new_size as usize);
        let mut h = self.heap.borrow_mut();
        let s = ptr as usize;
        let d = addr as usize;
        let tmp: Vec<u8> = h[s..s + copy].to_vec();
        h[d..d + copy].copy_from_slice(&tmp);
        addr
    }

    fn free(&self, _ptr: u64) {}

    fn ptr_read(&self, addr: u64, offset: u64, ty: Type) -> Option<RawSlot> {
        if let Some((slot, _)) = self.typed.borrow().get(&(addr as usize, offset as usize)) {
            return Some(*slot);
        }
        let w = scalar_byte_width(ty)?;
        let h = self.heap.borrow();
        let off = addr as usize + offset as usize;
        let slice = h.get(off..off + w)?;
        let mut buf = [0u8; 8];
        buf[..w].copy_from_slice(slice);
        Some(byte_value_to_slot(u64::from_le_bytes(buf), ty))
    }

    fn ptr_write(&self, addr: u64, offset: u64, value: RawSlot, ty: Type) {
        self.typed
            .borrow_mut()
            .insert((addr as usize, offset as usize), (value, ty));
        if let Some(w) = scalar_byte_width(ty) {
            let mut h = self.heap.borrow_mut();
            let off = addr as usize + offset as usize;
            if off + w <= h.len() {
                let bytes = unsafe { value.u64 }.to_le_bytes();
                h[off..off + w].copy_from_slice(&bytes[..w]);
            }
        }
    }

    fn alloc_str_bytes(&self, bytes: &[u8]) -> u64 {
        let len = bytes.len();
        let base = self.alloc_at((len + 1 + 8) as u64, 0) as usize;
        {
            let mut h = self.heap.borrow_mut();
            h[base..base + len].copy_from_slice(bytes); // [0..len]
            h[base + len] = 0; // NUL at [len]
            let len_bytes = (len as u64).to_le_bytes();
            h[base + len + 1..base + len + 1 + 8].copy_from_slice(&len_bytes);
        }
        (base + len + 1) as u64
    }

    fn read_str_bytes(&self, value: u64) -> Vec<u8> {
        if value == 0 {
            return Vec::new();
        }
        let len = self.string_len(value) as usize;
        let start = value as usize - len - 1;
        let h = self.heap.borrow();
        h.get(start..start + len).unwrap_or_default().to_vec()
    }

    fn string_len(&self, value: u64) -> u64 {
        let h = self.heap.borrow();
        let off = value as usize;
        if off + 8 <= h.len() {
            u64::from_le_bytes(h[off..off + 8].try_into().unwrap())
        } else {
            0
        }
    }

    fn mem_copy(&self, src: u64, dest: u64, size: u64) {
        let mut h = self.heap.borrow_mut();
        let s = src as usize;
        let d = dest as usize;
        let n = size as usize;
        if s + n <= h.len() && d + n <= h.len() {
            let tmp: Vec<u8> = h[s..s + n].to_vec();
            h[d..d + n].copy_from_slice(&tmp);
        }
    }

    fn read_byte_at(&self, addr: u64, offset: u64) -> u8 {
        if let Some((slot, _)) = self.typed.borrow().get(&(addr as usize, offset as usize)) {
            return unsafe { slot.u64 } as u8;
        }
        self.heap
            .borrow()
            .get(addr as usize + offset as usize)
            .copied()
            .unwrap_or(0)
    }

    fn mem_stat(&self, _stat: frontend::ast::MemStat) -> u64 {
        0
    }

    fn record_allocator_layout(&self, _name: &str, _managed: u64, _live: u64, _free: u64, _largest: u64) {}
}

fn run(module: &Module) -> Result<i64, String> {
    run_module(module, &TestHost::new())
}

#[test]
fn vm_returns_constant_u64() {
    let mut interner = DefaultStringInterner::default();
    let main_sym = interner.get_or_intern("main");

    let mut module = Module::new();
    let main_id = module.declare_function(
        main_sym,
        "main".to_string(),
        Linkage::Export,
        vec![],
        Type::U64,
    );
    let func = module.function_mut(main_id);
    let entry = func.add_block();
    func.entry = entry;
    let block = func.block_mut(entry);
    block.instructions.push(Instruction {
        result: Some((ValueId(0), Type::U64)),
        kind: compiler_ir::InstKind::Const(Const::U64(42)),
    });
    block.terminator = Some(Terminator::Return(vec![ValueId(0)]));

    let result = run(&module).unwrap();
    assert_eq!(result, 42);
}

#[test]
fn vm_returns_constant_i64() {
    let mut interner = DefaultStringInterner::default();
    let main_sym = interner.get_or_intern("main");

    let mut module = Module::new();
    let main_id = module.declare_function(
        main_sym,
        "main".to_string(),
        Linkage::Export,
        vec![],
        Type::I64,
    );
    let func = module.function_mut(main_id);
    let entry = func.add_block();
    func.entry = entry;
    let block = func.block_mut(entry);
    block.instructions.push(Instruction {
        result: Some((ValueId(0), Type::I64)),
        kind: compiler_ir::InstKind::Const(Const::I64(-7)),
    });
    block.terminator = Some(Terminator::Return(vec![ValueId(0)]));

    let result = run(&module).unwrap();
    assert_eq!(result, -7);
}

#[test]
fn vm_adds_two_constants() {
    let mut interner = DefaultStringInterner::default();
    let main_sym = interner.get_or_intern("main");

    let mut module = Module::new();
    let main_id = module.declare_function(
        main_sym,
        "main".to_string(),
        Linkage::Export,
        vec![],
        Type::U64,
    );
    let func = module.function_mut(main_id);
    let entry = func.add_block();
    func.entry = entry;
    let block = func.block_mut(entry);
    block.instructions.push(Instruction {
        result: Some((ValueId(0), Type::U64)),
        kind: compiler_ir::InstKind::Const(Const::U64(10)),
    });
    block.instructions.push(Instruction {
        result: Some((ValueId(1), Type::U64)),
        kind: compiler_ir::InstKind::Const(Const::U64(32)),
    });
    block.instructions.push(Instruction {
        result: Some((ValueId(2), Type::U64)),
        kind: compiler_ir::InstKind::BinOp {
            op: compiler_ir::BinOp::Add,
            lhs: ValueId(0),
            rhs: ValueId(1),
        },
    });
    block.terminator = Some(Terminator::Return(vec![ValueId(2)]));

    let result = run(&module).unwrap();
    assert_eq!(result, 42);
}

#[test]
fn vm_branch_takes_true_arm() {
    let mut interner = DefaultStringInterner::default();
    let main_sym = interner.get_or_intern("main");

    let mut module = Module::new();
    let main_id = module.declare_function(
        main_sym,
        "main".to_string(),
        Linkage::Export,
        vec![],
        Type::U64,
    );
    let func = module.function_mut(main_id);
    let entry = func.add_block();
    let then_blk = func.add_block();
    let else_blk = func.add_block();
    func.entry = entry;

    // entry: cond = true; br cond, then, else
    let entry_block = func.block_mut(entry);
    entry_block.instructions.push(Instruction {
        result: Some((ValueId(0), Type::Bool)),
        kind: compiler_ir::InstKind::Const(Const::Bool(true)),
    });
    entry_block.terminator = Some(Terminator::Branch {
        cond: ValueId(0),
        then_blk,
        else_blk,
    });

    // then: return 1
    let then_block = func.block_mut(then_blk);
    then_block.instructions.push(Instruction {
        result: Some((ValueId(1), Type::U64)),
        kind: compiler_ir::InstKind::Const(Const::U64(1)),
    });
    then_block.terminator = Some(Terminator::Return(vec![ValueId(1)]));

    // else: return 2
    let else_block = func.block_mut(else_blk);
    else_block.instructions.push(Instruction {
        result: Some((ValueId(2), Type::U64)),
        kind: compiler_ir::InstKind::Const(Const::U64(2)),
    });
    else_block.terminator = Some(Terminator::Return(vec![ValueId(2)]));

    let result = run(&module).unwrap();
    assert_eq!(result, 1);
}

#[test]
fn vm_calls_function() {
    let mut interner = DefaultStringInterner::default();
    let main_sym = interner.get_or_intern("main");
    let add_sym = interner.get_or_intern("add");

    let mut module = Module::new();
    // main must be FuncId(0) because Vm::run hardcodes it.
    let main_id = module.declare_function(
        main_sym,
        "main".to_string(),
        Linkage::Export,
        vec![],
        Type::U64,
    );
    let add_id = module.declare_function(
        add_sym,
        "add".to_string(),
        Linkage::Local,
        vec![Type::U64, Type::U64],
        Type::U64,
    );
    {
        let func = module.function_mut(add_id);
        let entry = func.add_block();
        func.entry = entry;
        let block = func.block_mut(entry);
        // params are @l0 and @l1
        block.instructions.push(Instruction {
            result: Some((ValueId(0), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(1), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(1)),
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(2), Type::U64)),
            kind: compiler_ir::InstKind::BinOp {
                op: compiler_ir::BinOp::Add,
                lhs: ValueId(0),
                rhs: ValueId(1),
            },
        });
        block.terminator = Some(Terminator::Return(vec![ValueId(2)]));
    }

    {
        let func = module.function_mut(main_id);
        let entry = func.add_block();
        func.entry = entry;
        let block = func.block_mut(entry);
        block.instructions.push(Instruction {
            result: Some((ValueId(0), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(10)),
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(1), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(32)),
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(2), Type::U64)),
            kind: compiler_ir::InstKind::Call {
                target: add_id,
                args: vec![ValueId(0), ValueId(1)],
            },
        });
        block.terminator = Some(Terminator::Return(vec![ValueId(2)]));
    }

    let result = run(&module).unwrap();
    assert_eq!(result, 42);
}

#[test]
fn vm_while_loop_counts_down() {
    let mut interner = DefaultStringInterner::default();
    let main_sym = interner.get_or_intern("main");

    let mut module = Module::new();
    let main_id = module.declare_function(
        main_sym,
        "main".to_string(),
        Linkage::Export,
        vec![],
        Type::U64,
    );
    let func = module.function_mut(main_id);
    // Register locals so CallFrame allocates slots for them.
    let _acc = func.add_local(Type::U64);
    let _n = func.add_local(Type::U64);

    let entry = func.add_block();
    let body = func.add_block();
    let _exit = func.add_block();
    func.entry = entry;

    // entry: acc = 0; n = 5; jump body
    let entry_block = func.block_mut(entry);
    entry_block.instructions.push(Instruction {
        result: Some((ValueId(0), Type::U64)),
        kind: compiler_ir::InstKind::Const(Const::U64(0)),
    });
    entry_block.instructions.push(Instruction {
        result: Some((ValueId(1), Type::U64)),
        kind: compiler_ir::InstKind::Const(Const::U64(5)),
    });
    entry_block.instructions.push(Instruction {
        result: None,
        kind: compiler_ir::InstKind::StoreLocal {
            dst: LocalId(0),
            src: ValueId(0),
        },
    });
    entry_block.instructions.push(Instruction {
        result: None,
        kind: compiler_ir::InstKind::StoreLocal {
            dst: LocalId(1),
            src: ValueId(1),
        },
    });
    entry_block.terminator = Some(Terminator::Jump(body));

    let loop_body = func.add_block();
    let after_loop = func.add_block();

    // body: load n; cond = n > 0; br cond, loop_body, after_loop
    let body_block = func.block_mut(body);
    body_block.instructions.push(Instruction {
        result: Some((ValueId(2), Type::U64)),
        kind: compiler_ir::InstKind::LoadLocal(LocalId(1)),
    });
    body_block.instructions.push(Instruction {
        result: Some((ValueId(3), Type::U64)),
        kind: compiler_ir::InstKind::Const(Const::U64(0)),
    });
    body_block.instructions.push(Instruction {
        result: Some((ValueId(4), Type::Bool)),
        kind: compiler_ir::InstKind::BinOp {
            op: compiler_ir::BinOp::Gt,
            lhs: ValueId(2),
            rhs: ValueId(3),
        },
    });
    body_block.terminator = Some(Terminator::Branch {
        cond: ValueId(4),
        then_blk: loop_body,
        else_blk: after_loop,
    });

    // loop_body: acc = acc + n; n = n - 1; jump body
    let lb = func.block_mut(loop_body);
    lb.instructions.push(Instruction {
        result: Some((ValueId(5), Type::U64)),
        kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
    });
    lb.instructions.push(Instruction {
        result: Some((ValueId(6), Type::U64)),
        kind: compiler_ir::InstKind::LoadLocal(LocalId(1)),
    });
    lb.instructions.push(Instruction {
        result: Some((ValueId(7), Type::U64)),
        kind: compiler_ir::InstKind::BinOp {
            op: compiler_ir::BinOp::Add,
            lhs: ValueId(5),
            rhs: ValueId(6),
        },
    });
    lb.instructions.push(Instruction {
        result: None,
        kind: compiler_ir::InstKind::StoreLocal {
            dst: LocalId(0),
            src: ValueId(7),
        },
    });
    lb.instructions.push(Instruction {
        result: Some((ValueId(8), Type::U64)),
        kind: compiler_ir::InstKind::LoadLocal(LocalId(1)),
    });
    lb.instructions.push(Instruction {
        result: Some((ValueId(9), Type::U64)),
        kind: compiler_ir::InstKind::Const(Const::U64(1)),
    });
    lb.instructions.push(Instruction {
        result: Some((ValueId(10), Type::U64)),
        kind: compiler_ir::InstKind::BinOp {
            op: compiler_ir::BinOp::Sub,
            lhs: ValueId(8),
            rhs: ValueId(9),
        },
    });
    lb.instructions.push(Instruction {
        result: None,
        kind: compiler_ir::InstKind::StoreLocal {
            dst: LocalId(1),
            src: ValueId(10),
        },
    });
    lb.terminator = Some(Terminator::Jump(body));

    // after_loop: load acc; return acc
    let al = func.block_mut(after_loop);
    al.instructions.push(Instruction {
        result: Some((ValueId(11), Type::U64)),
        kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
    });
    al.terminator = Some(Terminator::Return(vec![ValueId(11)]));

    let result = run(&module).unwrap();
    // sum of 5+4+3+2+1 = 15
    assert_eq!(result, 15);
}

#[test]
fn vm_factorial_via_loop() {
    let mut interner = DefaultStringInterner::default();
    let main_sym = interner.get_or_intern("main");

    let mut module = Module::new();
    let main_id = module.declare_function(
        main_sym,
        "main".to_string(),
        Linkage::Export,
        vec![],
        Type::U64,
    );
    let func = module.function_mut(main_id);
    let _result = func.add_local(Type::U64);
    let _i = func.add_local(Type::U64);
    let entry = func.add_block();
    let header = func.add_block();
    let body = func.add_block();
    let exit = func.add_block();
    func.entry = entry;

    // entry: result = 1; i = 5; jump header
    let e = func.block_mut(entry);
    e.instructions.push(Instruction {
        result: Some((ValueId(0), Type::U64)),
        kind: compiler_ir::InstKind::Const(Const::U64(1)),
    });
    e.instructions.push(Instruction {
        result: Some((ValueId(1), Type::U64)),
        kind: compiler_ir::InstKind::Const(Const::U64(5)),
    });
    e.instructions.push(Instruction {
        result: None,
        kind: compiler_ir::InstKind::StoreLocal {
            dst: LocalId(0),
            src: ValueId(0),
        },
    });
    e.instructions.push(Instruction {
        result: None,
        kind: compiler_ir::InstKind::StoreLocal {
            dst: LocalId(1),
            src: ValueId(1),
        },
    });
    e.terminator = Some(Terminator::Jump(header));

    // header: load i; cond = i > 0; br cond, body, exit
    let h = func.block_mut(header);
    h.instructions.push(Instruction {
        result: Some((ValueId(2), Type::U64)),
        kind: compiler_ir::InstKind::LoadLocal(LocalId(1)),
    });
    h.instructions.push(Instruction {
        result: Some((ValueId(3), Type::U64)),
        kind: compiler_ir::InstKind::Const(Const::U64(0)),
    });
    h.instructions.push(Instruction {
        result: Some((ValueId(4), Type::Bool)),
        kind: compiler_ir::InstKind::BinOp {
            op: compiler_ir::BinOp::Gt,
            lhs: ValueId(2),
            rhs: ValueId(3),
        },
    });
    h.terminator = Some(Terminator::Branch {
        cond: ValueId(4),
        then_blk: body,
        else_blk: exit,
    });

    // body: result = result * i; i = i - 1; jump header
    let b = func.block_mut(body);
    b.instructions.push(Instruction {
        result: Some((ValueId(5), Type::U64)),
        kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
    });
    b.instructions.push(Instruction {
        result: Some((ValueId(6), Type::U64)),
        kind: compiler_ir::InstKind::LoadLocal(LocalId(1)),
    });
    b.instructions.push(Instruction {
        result: Some((ValueId(7), Type::U64)),
        kind: compiler_ir::InstKind::BinOp {
            op: compiler_ir::BinOp::Mul,
            lhs: ValueId(5),
            rhs: ValueId(6),
        },
    });
    b.instructions.push(Instruction {
        result: None,
        kind: compiler_ir::InstKind::StoreLocal {
            dst: LocalId(0),
            src: ValueId(7),
        },
    });
    b.instructions.push(Instruction {
        result: Some((ValueId(8), Type::U64)),
        kind: compiler_ir::InstKind::LoadLocal(LocalId(1)),
    });
    b.instructions.push(Instruction {
        result: Some((ValueId(9), Type::U64)),
        kind: compiler_ir::InstKind::Const(Const::U64(1)),
    });
    b.instructions.push(Instruction {
        result: Some((ValueId(10), Type::U64)),
        kind: compiler_ir::InstKind::BinOp {
            op: compiler_ir::BinOp::Sub,
            lhs: ValueId(8),
            rhs: ValueId(9),
        },
    });
    b.instructions.push(Instruction {
        result: None,
        kind: compiler_ir::InstKind::StoreLocal {
            dst: LocalId(1),
            src: ValueId(10),
        },
    });
    b.terminator = Some(Terminator::Jump(header));

    // exit: load result; return result
    let x = func.block_mut(exit);
    x.instructions.push(Instruction {
        result: Some((ValueId(11), Type::U64)),
        kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
    });
    x.terminator = Some(Terminator::Return(vec![ValueId(11)]));

    let result = run(&module).unwrap();
    assert_eq!(result, 120);
}

#[test]
fn vm_store_local_and_reload() {
    let mut interner = DefaultStringInterner::default();
    let main_sym = interner.get_or_intern("main");

    let mut module = Module::new();
    let main_id = module.declare_function(
        main_sym,
        "main".to_string(),
        Linkage::Export,
        vec![],
        Type::U64,
    );
    let func = module.function_mut(main_id);
    let _x = func.add_local(Type::U64);
    let entry = func.add_block();
    func.entry = entry;
    let block = func.block_mut(entry);
    block.instructions.push(Instruction {
        result: Some((ValueId(0), Type::U64)),
        kind: compiler_ir::InstKind::Const(Const::U64(7)),
    });
    block.instructions.push(Instruction {
        result: None,
        kind: compiler_ir::InstKind::StoreLocal {
            dst: LocalId(0),
            src: ValueId(0),
        },
    });
    block.instructions.push(Instruction {
        result: Some((ValueId(1), Type::U64)),
        kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
    });
    block.instructions.push(Instruction {
        result: Some((ValueId(2), Type::U64)),
        kind: compiler_ir::InstKind::Const(Const::U64(3)),
    });
    block.instructions.push(Instruction {
        result: Some((ValueId(3), Type::U64)),
        kind: compiler_ir::InstKind::BinOp {
            op: compiler_ir::BinOp::Add,
            lhs: ValueId(1),
            rhs: ValueId(2),
        },
    });
    block.instructions.push(Instruction {
        result: None,
        kind: compiler_ir::InstKind::StoreLocal {
            dst: LocalId(0),
            src: ValueId(3),
        },
    });
    block.instructions.push(Instruction {
        result: Some((ValueId(4), Type::U64)),
        kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
    });
    block.terminator = Some(Terminator::Return(vec![ValueId(4)]));

    let result = run(&module).unwrap();
    assert_eq!(result, 10);
}

#[test]
fn vm_cast_i64_to_f64_and_back() {
    let mut interner = DefaultStringInterner::default();
    let main_sym = interner.get_or_intern("main");

    let mut module = Module::new();
    let main_id = module.declare_function(
        main_sym,
        "main".to_string(),
        Linkage::Export,
        vec![],
        Type::I64,
    );
    let func = module.function_mut(main_id);
    let entry = func.add_block();
    func.entry = entry;
    let block = func.block_mut(entry);
    block.instructions.push(Instruction {
        result: Some((ValueId(0), Type::I64)),
        kind: compiler_ir::InstKind::Const(Const::I64(42)),
    });
    block.instructions.push(Instruction {
        result: Some((ValueId(1), Type::F64)),
        kind: compiler_ir::InstKind::Cast {
            value: ValueId(0),
            from: Type::I64,
            to: Type::F64,
        },
    });
    block.instructions.push(Instruction {
        result: Some((ValueId(2), Type::I64)),
        kind: compiler_ir::InstKind::Cast {
            value: ValueId(1),
            from: Type::F64,
            to: Type::I64,
        },
    });
    block.terminator = Some(Terminator::Return(vec![ValueId(2)]));

    let result = run(&module).unwrap();
    assert_eq!(result, 42);
}

#[test]
fn vm_recursive_fib() {
    let mut interner = DefaultStringInterner::default();
    let main_sym = interner.get_or_intern("main");
    let fib_sym = interner.get_or_intern("fib");

    let mut module = Module::new();
    let main_id = module.declare_function(
        main_sym,
        "main".to_string(),
        Linkage::Export,
        vec![],
        Type::U64,
    );
    let fib_id = module.declare_function(
        fib_sym,
        "fib".to_string(),
        Linkage::Local,
        vec![Type::U64],
        Type::U64,
    );

    // fib(n):
    {
        let func = module.function_mut(fib_id);
        // No body locals needed — all computation uses values directly.
        let entry = func.add_block();
        let recurse = func.add_block();
        let base = func.add_block();
        func.entry = entry;

        let e = func.block_mut(entry);
        e.instructions.push(Instruction {
            result: Some((ValueId(0), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
        });
        e.instructions.push(Instruction {
            result: Some((ValueId(1), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(1)),
        });
        e.instructions.push(Instruction {
            result: Some((ValueId(2), Type::Bool)),
            kind: compiler_ir::InstKind::BinOp {
                op: compiler_ir::BinOp::Le,
                lhs: ValueId(0),
                rhs: ValueId(1),
            },
        });
        e.terminator = Some(Terminator::Branch {
            cond: ValueId(2),
            then_blk: base,
            else_blk: recurse,
        });

        let b = func.block_mut(base);
        b.instructions.push(Instruction {
            result: Some((ValueId(3), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
        });
        b.terminator = Some(Terminator::Return(vec![ValueId(3)]));

        let r = func.block_mut(recurse);
        r.instructions.push(Instruction {
            result: Some((ValueId(4), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
        });
        r.instructions.push(Instruction {
            result: Some((ValueId(5), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(1)),
        });
        r.instructions.push(Instruction {
            result: Some((ValueId(6), Type::U64)),
            kind: compiler_ir::InstKind::BinOp {
                op: compiler_ir::BinOp::Sub,
                lhs: ValueId(4),
                rhs: ValueId(5),
            },
        });
        r.instructions.push(Instruction {
            result: Some((ValueId(7), Type::U64)),
            kind: compiler_ir::InstKind::Call {
                target: fib_id,
                args: vec![ValueId(6)],
            },
        });
        r.instructions.push(Instruction {
            result: Some((ValueId(8), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
        });
        r.instructions.push(Instruction {
            result: Some((ValueId(9), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(2)),
        });
        r.instructions.push(Instruction {
            result: Some((ValueId(10), Type::U64)),
            kind: compiler_ir::InstKind::BinOp {
                op: compiler_ir::BinOp::Sub,
                lhs: ValueId(8),
                rhs: ValueId(9),
            },
        });
        r.instructions.push(Instruction {
            result: Some((ValueId(11), Type::U64)),
            kind: compiler_ir::InstKind::Call {
                target: fib_id,
                args: vec![ValueId(10)],
            },
        });
        r.instructions.push(Instruction {
            result: Some((ValueId(12), Type::U64)),
            kind: compiler_ir::InstKind::BinOp {
                op: compiler_ir::BinOp::Add,
                lhs: ValueId(7),
                rhs: ValueId(11),
            },
        });
        r.terminator = Some(Terminator::Return(vec![ValueId(12)]));
    }

    // main: return fib(6)
    {
        let func = module.function_mut(main_id);
        let entry = func.add_block();
        func.entry = entry;
        let block = func.block_mut(entry);
        block.instructions.push(Instruction {
            result: Some((ValueId(0), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(6)),
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(1), Type::U64)),
            kind: compiler_ir::InstKind::Call {
                target: fib_id,
                args: vec![ValueId(0)],
            },
        });
        block.terminator = Some(Terminator::Return(vec![ValueId(1)]));
    }

    let result = run(&module).unwrap();
    assert_eq!(result, 8);
}

#[test]
fn vm_call_struct_returns_two_fields() {
    let mut interner = DefaultStringInterner::default();
    let main_sym = interner.get_or_intern("main");
    let make_sym = interner.get_or_intern("make_point");

    let mut module = Module::new();
    let make_id = module.declare_function(
        make_sym,
        "make_point".to_string(),
        Linkage::Local,
        vec![],
        Type::Struct(compiler_ir::StructId(0)),
    );
    let main_id = module.declare_function(
        main_sym,
        "main".to_string(),
        Linkage::Export,
        vec![],
        Type::U64,
    );

    // make_point(): return Point { x: 10, y: 20 }
    {
        let func = module.function_mut(make_id);
        func.add_local(Type::U64); // x
        func.add_local(Type::U64); // y
        let entry = func.add_block();
        func.entry = entry;
        let block = func.block_mut(entry);
        block.instructions.push(Instruction {
            result: Some((ValueId(0), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(10)),
        });
        block.instructions.push(Instruction {
            result: None,
            kind: compiler_ir::InstKind::StoreLocal {
                dst: LocalId(0),
                src: ValueId(0),
            },
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(1), Type::U64)),
            kind: compiler_ir::InstKind::Const(Const::U64(20)),
        });
        block.instructions.push(Instruction {
            result: None,
            kind: compiler_ir::InstKind::StoreLocal {
                dst: LocalId(1),
                src: ValueId(1),
            },
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(2), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(3), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(1)),
        });
        block.terminator = Some(Terminator::Return(vec![ValueId(2), ValueId(3)]));
    }

    // main(): val p = make_point(); return p.x + p.y
    {
        let func = module.function_mut(main_id);
        func.add_local(Type::U64); // p.x
        func.add_local(Type::U64); // p.y
        let entry = func.add_block();
        func.entry = entry;
        let block = func.block_mut(entry);
        block.instructions.push(Instruction {
            result: None,
            kind: compiler_ir::InstKind::CallStruct {
                target: make_id,
                args: vec![],
                dests: vec![LocalId(0), LocalId(1)],
            },
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(0), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(1), Type::U64)),
            kind: compiler_ir::InstKind::LoadLocal(LocalId(1)),
        });
        block.instructions.push(Instruction {
            result: Some((ValueId(2), Type::U64)),
            kind: compiler_ir::InstKind::BinOp {
                op: compiler_ir::BinOp::Add,
                lhs: ValueId(0),
                rhs: ValueId(1),
            },
        });
        block.terminator = Some(Terminator::Return(vec![ValueId(2)]));
    }

    let result = run(&module).unwrap();
    assert_eq!(result, 30);
}

#[test]
fn vm_heap_alloc_ptr_write_read() {
    let mut interner = DefaultStringInterner::default();
    let main_sym = interner.get_or_intern("main");

    let mut module = Module::new();
    let main_id = module.declare_function(
        main_sym,
        "main".to_string(),
        Linkage::Export,
        vec![],
        Type::U64,
    );
    let func = module.function_mut(main_id);
    func.add_local(Type::U64); // ptr
    let entry = func.add_block();
    func.entry = entry;
    let block = func.block_mut(entry);

    // ptr = heap_alloc(8)
    block.instructions.push(Instruction {
        result: Some((ValueId(0), Type::U64)),
        kind: compiler_ir::InstKind::Const(Const::U64(8)),
    });
    block.instructions.push(Instruction {
        result: Some((ValueId(1), Type::U64)),
        kind: compiler_ir::InstKind::HeapAlloc {
            size: ValueId(0),
            site: 0,
            binding: compiler_ir::AllocatorBinding::Ambient,
        },
    });
    block.instructions.push(Instruction {
        result: None,
        kind: compiler_ir::InstKind::StoreLocal {
            dst: LocalId(0),
            src: ValueId(1),
        },
    });

    // ptr_write(ptr, 0, 42u64)
    block.instructions.push(Instruction {
        result: Some((ValueId(2), Type::U64)),
        kind: compiler_ir::InstKind::Const(Const::U64(42)),
    });
    block.instructions.push(Instruction {
        result: Some((ValueId(3), Type::U64)),
        kind: compiler_ir::InstKind::Const(Const::U64(0)),
    });
    block.instructions.push(Instruction {
        result: None,
        kind: compiler_ir::InstKind::PtrWrite {
            ptr: ValueId(1),
            offset: ValueId(3),
            value: ValueId(2),
            value_ty: Type::U64,
        },
    });

    // val = ptr_read(ptr, 0, U64)
    block.instructions.push(Instruction {
        result: Some((ValueId(4), Type::U64)),
        kind: compiler_ir::InstKind::LoadLocal(LocalId(0)),
    });
    block.instructions.push(Instruction {
        result: Some((ValueId(5), Type::U64)),
        kind: compiler_ir::InstKind::Const(Const::U64(0)),
    });
    block.instructions.push(Instruction {
        result: Some((ValueId(6), Type::U64)),
        kind: compiler_ir::InstKind::PtrRead {
            ptr: ValueId(4),
            offset: ValueId(5),
            elem_ty: Type::U64,
        },
    });
    block.terminator = Some(Terminator::Return(vec![ValueId(6)]));

    let result = run(&module).unwrap();
    assert_eq!(result, 42);
}

#[test]
fn vm_capturing_closure_via_make_and_call_indirect() {
    use compiler_ir::{AllocatorBinding, BinOp, FuncId, InstKind};
    let mut interner = DefaultStringInterner::default();
    let main_sym = interner.get_or_intern("main");
    let body_sym = interner.get_or_intern("add_n_closure");

    let mut module = Module::new();
    // Lifted closure body: fn(env: u64, x: i64) -> i64 { x + *(env+8) }
    let body_id = module.declare_function(
        body_sym,
        "add_n_closure".to_string(),
        Linkage::Local,
        vec![Type::U64, Type::I64],
        Type::I64,
    );
    let main_id = module.declare_function(
        main_sym,
        "main".to_string(),
        Linkage::Export,
        vec![],
        Type::I64,
    );

    // body: param[0] = env, param[1] = x; n = ptr_read(env, 8, I64); x + n
    {
        let func = module.function_mut(body_id);
        let entry = func.add_block();
        func.entry = entry;
        let b = func.block_mut(entry);
        b.instructions.push(Instruction {
            result: Some((ValueId(0), Type::U64)),
            kind: InstKind::LoadLocal(LocalId(0)),
        });
        b.instructions.push(Instruction {
            result: Some((ValueId(1), Type::U64)),
            kind: InstKind::Const(Const::U64(8)),
        });
        b.instructions.push(Instruction {
            result: Some((ValueId(2), Type::I64)),
            kind: InstKind::PtrRead {
                ptr: ValueId(0),
                offset: ValueId(1),
                elem_ty: Type::I64,
            },
        });
        b.instructions.push(Instruction {
            result: Some((ValueId(3), Type::I64)),
            kind: InstKind::LoadLocal(LocalId(1)),
        });
        b.instructions.push(Instruction {
            result: Some((ValueId(4), Type::I64)),
            kind: InstKind::BinOp {
                op: BinOp::Add,
                lhs: ValueId(3),
                rhs: ValueId(2),
            },
        });
        b.terminator = Some(Terminator::Return(vec![ValueId(4)]));
    }

    // main: n = 10; cl = MakeClosure(add_n_closure, [n]); cl(5)
    {
        let func = module.function_mut(main_id);
        let entry = func.add_block();
        func.entry = entry;
        let m = func.block_mut(entry);
        m.instructions.push(Instruction {
            result: Some((ValueId(0), Type::I64)),
            kind: InstKind::Const(Const::I64(10)),
        });
        m.instructions.push(Instruction {
            result: Some((ValueId(1), Type::U64)),
            kind: InstKind::MakeClosure {
                target: body_id,
                captures: vec![ValueId(0)],
                capture_tys: vec![Type::I64],
            },
        });
        m.instructions.push(Instruction {
            result: Some((ValueId(2), Type::I64)),
            kind: InstKind::Const(Const::I64(5)),
        });
        m.instructions.push(Instruction {
            result: Some((ValueId(3), Type::I64)),
            kind: InstKind::CallIndirect {
                callee: ValueId(1),
                args: vec![ValueId(2)],
                param_tys: vec![Type::I64],
                ret_ty: Type::I64,
            },
        });
        m.terminator = Some(Terminator::Return(vec![ValueId(3)]));
        let _ = AllocatorBinding::Ambient;
        let _ = FuncId(0);
    }

    let result = run(&module).unwrap();
    assert_eq!(result, 15);
}

#[test]
fn vm_dyn_dispatch_via_vtable() {
    use compiler_ir::{BinOp, FuncId, InstKind};
    let mut interner = DefaultStringInterner::default();
    let main_sym = interner.get_or_intern("main");
    let thunk_sym = interner.get_or_intern("thunk_sound");
    let trait_sym = interner.get_or_intern("Animal");
    let struct_sym = interner.get_or_intern("Dog");

    let mut module = Module::new();
    // Thunk: fn(data_ptr: u64) -> i64 { 7 + (data_ptr & 0) }
    // (uses data_ptr trivially so the unused-arg path is exercised)
    let thunk_id = module.declare_function(
        thunk_sym,
        "thunk_sound".to_string(),
        Linkage::Local,
        vec![Type::U64],
        Type::I64,
    );
    let main_id = module.declare_function(
        main_sym,
        "main".to_string(),
        Linkage::Export,
        vec![],
        Type::I64,
    );
    // Register the vtable + method order, mirroring the AOT module.
    module
        .vtables
        .insert((trait_sym, struct_sym), vec![thunk_id]);
    module
        .trait_method_order
        .insert(trait_sym, vec![interner.get_or_intern("sound")]);

    {
        let func = module.function_mut(thunk_id);
        let entry = func.add_block();
        func.entry = entry;
        let b = func.block_mut(entry);
        b.instructions.push(Instruction {
            result: Some((ValueId(0), Type::I64)),
            kind: InstKind::Const(Const::I64(7)),
        });
        b.terminator = Some(Terminator::Return(vec![ValueId(0)]));
    }

    // main: vtable = VtableAddr; fn_ptr = *(vtable+0); fn_ptr(data_ptr=0)
    {
        let func = module.function_mut(main_id);
        let entry = func.add_block();
        func.entry = entry;
        let m = func.block_mut(entry);
        m.instructions.push(Instruction {
            result: Some((ValueId(0), Type::U64)),
            kind: InstKind::VtableAddr { trait_sym, struct_sym },
        });
        m.instructions.push(Instruction {
            result: Some((ValueId(1), Type::U64)),
            kind: InstKind::Const(Const::U64(0)),
        });
        m.instructions.push(Instruction {
            result: Some((ValueId(2), Type::U64)),
            kind: InstKind::PtrRead {
                ptr: ValueId(0),
                offset: ValueId(1),
                elem_ty: Type::U64,
            },
        });
        // data_ptr = 0 (empty struct sentinel)
        m.instructions.push(Instruction {
            result: Some((ValueId(3), Type::U64)),
            kind: InstKind::Const(Const::U64(0)),
        });
        m.instructions.push(Instruction {
            result: Some((ValueId(4), Type::I64)),
            kind: InstKind::CallIndirectFn {
                callee: ValueId(2),
                args: vec![ValueId(3)],
                param_tys: vec![Type::U64],
                ret_ty: Type::I64,
            },
        });
        m.terminator = Some(Terminator::Return(vec![ValueId(4)]));
        let _ = (BinOp::Add, FuncId(0));
    }

    let result = run(&module).unwrap();
    assert_eq!(result, 7);
}

#[test]
fn vm_mut_ref_propagates_across_call() {
    // fn inc(p: &mut i64) { *p = *p + 1 }   (p is LocalId(0), a U64 ptr)
    // fn main() -> i64 { var v = 41; inc(&mut v); v }
    use compiler_ir::{BinOp, FuncId, InstKind};
    let mut interner = DefaultStringInterner::default();
    let main_sym = interner.get_or_intern("main");
    let inc_sym = interner.get_or_intern("inc");

    let mut module = Module::new();
    let inc_id = module.declare_function(
        inc_sym,
        "inc".to_string(),
        Linkage::Local,
        vec![Type::U64],
        Type::Unit,
    );
    let main_id = module.declare_function(
        main_sym,
        "main".to_string(),
        Linkage::Export,
        vec![],
        Type::I64,
    );

    // inc: *p = *p + 1
    {
        let func = module.function_mut(inc_id);
        let entry = func.add_block();
        func.entry = entry;
        let b = func.block_mut(entry);
        b.instructions.push(Instruction {
            result: Some((ValueId(0), Type::U64)),
            kind: InstKind::LoadLocal(LocalId(0)),
        });
        b.instructions.push(Instruction {
            result: Some((ValueId(1), Type::I64)),
            kind: InstKind::LoadRef { ptr: ValueId(0), ty: Type::I64 },
        });
        b.instructions.push(Instruction {
            result: Some((ValueId(2), Type::I64)),
            kind: InstKind::Const(Const::I64(1)),
        });
        b.instructions.push(Instruction {
            result: Some((ValueId(3), Type::I64)),
            kind: InstKind::BinOp { op: BinOp::Add, lhs: ValueId(1), rhs: ValueId(2) },
        });
        b.instructions.push(Instruction {
            result: Some((ValueId(4), Type::U64)),
            kind: InstKind::LoadLocal(LocalId(0)),
        });
        b.instructions.push(Instruction {
            result: None,
            kind: InstKind::StoreRef { ptr: ValueId(4), value: ValueId(3), ty: Type::I64 },
        });
        b.terminator = Some(Terminator::Return(vec![]));
    }

    // main: var v = 41 (address-taken); inc(&mut v); return v
    {
        let func = module.function_mut(main_id);
        let v = func.add_local(Type::I64); // LocalId(0)
        func.address_taken_locals.insert(v);
        let entry = func.add_block();
        func.entry = entry;
        let m = func.block_mut(entry);
        m.instructions.push(Instruction {
            result: Some((ValueId(0), Type::I64)),
            kind: InstKind::Const(Const::I64(41)),
        });
        m.instructions.push(Instruction {
            result: None,
            kind: InstKind::StoreLocal { dst: v, src: ValueId(0) },
        });
        m.instructions.push(Instruction {
            result: Some((ValueId(1), Type::U64)),
            kind: InstKind::AddressOf { local: v },
        });
        m.instructions.push(Instruction {
            result: None,
            kind: InstKind::Call { target: inc_id, args: vec![ValueId(1)] },
        });
        m.instructions.push(Instruction {
            result: Some((ValueId(2), Type::I64)),
            kind: InstKind::LoadLocal(v),
        });
        m.terminator = Some(Terminator::Return(vec![ValueId(2)]));
        let _ = FuncId(0);
    }

    let result = run(&module).unwrap();
    assert_eq!(result, 42);
}

#[test]
fn vm_panic_resolves_message_via_interner() {
    // A failing `requires`-style guard panics; the VM should surface
    // the interned message text, not a `panic #N` placeholder.
    use compiler_ir::InstKind;
    let mut interner = DefaultStringInterner::default();
    let main_sym = interner.get_or_intern("main");
    let msg_sym = interner.get_or_intern("requires violated: b != 0");

    let mut module = Module::new();
    let main_id = module.declare_function(
        main_sym,
        "main".to_string(),
        Linkage::Export,
        vec![],
        Type::U64,
    );
    let func = module.function_mut(main_id);
    let entry = func.add_block();
    let fail = func.add_block();
    func.entry = entry;
    // entry: cond = false; br cond, <unused>, fail
    let e = func.block_mut(entry);
    e.instructions.push(Instruction {
        result: Some((ValueId(0), Type::Bool)),
        kind: InstKind::Const(Const::Bool(false)),
    });
    e.terminator = Some(Terminator::Branch { cond: ValueId(0), then_blk: entry, else_blk: fail });
    let f = func.block_mut(fail);
    f.terminator = Some(Terminator::Panic { message: msg_sym });

    // Run with interner so the panic message resolves.
    let host = TestHost::new();
    let mut vm = Vm::with_interner(&module, &interner, &host);
    vm.call_function(main_id, Vec::new(), None, Vec::new());
    let res = vm.run_loop();
    match res {
        VmResult::Diverged { message } => {
            assert_eq!(message, "requires violated: b != 0");
        }
        _ => panic!("expected divergence"),
    }
}

#[test]
fn vm_str_raw_byte_layout_round_trip() {
    // Pin the AOT-compatible `[bytes][NUL][u64 len]` layout: a str value
    // points at the len field, byte_start = value - len - 1, and the
    // bytes are readable from the raw buffer (as `__builtin_str_to_ptr`
    // + PtrRead(U8) would do).
    let host = TestHost::new();
    let v = VmHost::alloc_str_bytes(&host, b"hi!");
    assert_eq!(host.string_len(v), 3);
    assert_eq!(VmHost::read_str(&host, v), "hi!");
    let byte_start = v - 3 - 1;
    // Byte-level reads through the same path PtrRead(U8) uses.
    let b0 = host.ptr_read(byte_start, 0, compiler_ir::Type::U8).unwrap();
    let b2 = host.ptr_read(byte_start, 2, compiler_ir::Type::U8).unwrap();
    assert_eq!(unsafe { b0.u64 }, b'h' as u64);
    assert_eq!(unsafe { b2.u64 }, b'!' as u64);
    // Concatenation preserves bytes and length.
    let w = VmHost::alloc_str_bytes(&host, b"yo");
    let cat = VmHost::concat_strings(&host, v, w);
    assert_eq!(VmHost::read_str(&host, cat), "hi!yo");
}

#[test]
fn step_budget_stops_a_hot_loop() {
    // COMPILE-TIME-EVAL C6: the fold runs user code at compile time,
    // so the VM must be able to stop a non-terminating loop. Pin the
    // back-edge counting and the message wording.
    use compiler_ir::InstKind;
    let mut interner = DefaultStringInterner::default();
    let main_sym = interner.get_or_intern("main");

    let mut module = Module::new();
    let main_id = module.declare_function(
        main_sym,
        "main".to_string(),
        Linkage::Export,
        vec![],
        Type::U64,
    );
    let func = module.function_mut(main_id);
    let _i = func.add_local(Type::U64);
    let entry = func.add_block();
    let header = func.add_block();
    let exit = func.add_block();
    func.entry = entry;

    // entry: i = 0; jump header
    let e = func.block_mut(entry);
    e.instructions.push(Instruction {
        result: Some((ValueId(0), Type::U64)),
        kind: InstKind::Const(Const::U64(0)),
    });
    e.instructions.push(Instruction {
        result: None,
        kind: InstKind::StoreLocal { dst: LocalId(0), src: ValueId(0) },
    });
    e.terminator = Some(Terminator::Jump(header));

    // header: br true, header, exit  (the always-taken back-edge)
    let h = func.block_mut(header);
    h.instructions.push(Instruction {
        result: Some((ValueId(1), Type::Bool)),
        kind: InstKind::Const(Const::Bool(true)),
    });
    h.terminator = Some(Terminator::Branch { cond: ValueId(1), then_blk: header, else_blk: exit });

    let x = func.block_mut(exit);
    x.terminator = Some(Terminator::Return(vec![ValueId(1)]));

    let host = TestHost::new();
    let mut vm = Vm::with_interner(&module, &interner, &host);
    vm.set_step_budget(Some(10));
    vm.call_function(main_id, Vec::new(), None, Vec::new());
    match vm.run_loop() {
        VmResult::Diverged { message } => {
            assert!(message.contains("step budget exceeded"), "{message}");
            assert!(message.contains("10 loop iterations"), "{message}");
        }
        _ => panic!("expected the budget to stop the loop"),
    }
}

#[test]
fn step_budget_does_not_count_forward_jumps() {
    // A program whose jumps are all forward (an if/else) is not a
    // loop; a budget must not trip on it.
    use compiler_ir::InstKind;
    let mut interner = DefaultStringInterner::default();
    let main_sym = interner.get_or_intern("main");

    let mut module = Module::new();
    let main_id = module.declare_function(
        main_sym,
        "main".to_string(),
        Linkage::Export,
        vec![],
        Type::U64,
    );
    let func = module.function_mut(main_id);
    let entry = func.add_block();
    let then_blk = func.add_block();
    let join = func.add_block();
    func.entry = entry;

    let e = func.block_mut(entry);
    e.instructions.push(Instruction {
        result: Some((ValueId(0), Type::Bool)),
        kind: InstKind::Const(Const::Bool(true)),
    });
    e.terminator = Some(Terminator::Branch { cond: ValueId(0), then_blk, else_blk: join });

    let t = func.block_mut(then_blk);
    t.instructions.push(Instruction {
        result: Some((ValueId(1), Type::U64)),
        kind: InstKind::Const(Const::U64(42)),
    });
    t.terminator = Some(Terminator::Jump(join));

    let j = func.block_mut(join);
    j.instructions.push(Instruction {
        result: Some((ValueId(2), Type::U64)),
        kind: InstKind::Const(Const::U64(7)),
    });
    j.terminator = Some(Terminator::Return(vec![ValueId(2)]));

    let host = TestHost::new();
    let mut vm = Vm::with_interner(&module, &interner, &host);
    vm.set_step_budget(Some(10));
    vm.call_function(main_id, Vec::new(), None, Vec::new());
    match vm.run_loop() {
        VmResult::ExitCode(code) => assert_eq!(code, 7),
        VmResult::Diverged { message } => panic!("unexpected divergence: {message}"),
    }
}