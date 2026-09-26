//! SPAN-RANGE-INTRINSIC — the stdlib `Span<T>`'s range operations
//! lowered to the one range instruction their bodies perform, instead
//! of a call to them.
//!
//! `copy_from` / `move_from` / `bytes_eq` / `fill` are each a length
//! check and a single `MemCopy` / `MemMove` / `MemEq` / `MemSet`. The
//! compiled lanes have no inliner, so a caller paid a call (and, on the
//! IR VM, a Rust frame) for one instruction: `String::eq` written over
//! `Span::bytes_eq` was slower on `poc/logsearch`'s archive run, where
//! it is the key comparison of every `Dict<String, _>` probe, so it
//! stayed a raw `__builtin_mem_eq`. The same reasoning as
//! `Ptr<T>`'s `get` / `set` (MEMORY-ACCESS M5), and the same rule for
//! what qualifies: only methods written in `std.span`.
//!
//! The arithmetic is the body's -- `count * sizeof::<T>()` bytes from
//! each window's `addr` -- and so is the length-mismatch panic's text.
//! What differs is where that panic is reported: at the call, not
//! inside `Span::copy_from`, since there is no frame for it any more.
//!
//! `Span::from_parts(p, n)` bound by a `val` is the same kind of case
//! from the other side: the body is the struct literal
//! `Span { data: p, count: n }`, but a call returning a compound pays
//! for the call *and* the multi-value return. It is stored straight
//! into the binding's two leaves instead (`lower_let_span_from_parts`).
//! With both, `String::eq` written over two windows and `bytes_eq`
//! runs as fast as the raw builtin did; with `from_parts` still a
//! call it was 5-9% slower on the same run.
//!
//! `find` / `find_seq` stay calls: they answer an `Option<u64>`, and a
//! compound value has nowhere to go from here (`Ok(Some(value))` is one
//! scalar).

use frontend::ast::{Expr, ExprRef};
use string_interner::DefaultSymbol;

use super::bindings::{flatten_struct_locals, Binding};
use super::FunctionLower;
use crate::ir::{BinOp, Const, InstKind, LocalId, StructId, Terminator, Type, ValueId};

/// Which range operation.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SpanOp {
    CopyFrom,
    MoveFrom,
    BytesEq,
    Fill,
}

/// The locals holding a `Span<T>` binding's two leaves.
#[derive(Clone, Copy)]
struct Window {
    addr: LocalId,
    count: LocalId,
}

impl FunctionLower<'_> {
    /// Whether every spec registered for `method` on `target_sym` was
    /// written in the stdlib module `std.<module>`. A user type that
    /// is merely named `Ptr` or `Span` keeps its own methods.
    ///
    /// The generic impl's methods are templates in `generic_methods`;
    /// a concrete one (`impl Ptr<u8> { fn load16 .. }`) is in
    /// `method_registry`.
    pub(super) fn method_is_from_std_module(
        &self,
        target_sym: DefaultSymbol,
        method: DefaultSymbol,
        module: &str,
    ) -> bool {
        let all_from = |specs: &Vec<super::method_registry::MethodTemplateSpec>| {
            !specs.is_empty()
                && specs.iter().all(|spec| {
                    spec.method.module_path.as_deref().is_some_and(|path| {
                        let names: Vec<&str> =
                            path.iter().filter_map(|s| self.interner.resolve(*s)).collect();
                        names == ["std", module]
                    })
                })
        };
        self.generic_methods.get(&(target_sym, method)).is_some_and(all_from)
            || (!self.generic_methods.contains_key(&(target_sym, method))
                && self.method_registry.get(&(target_sym, method)).is_some_and(all_from))
    }

    /// The two leaves of a `Span<T>` binding, in declaration order
    /// (`data.addr`, then `count`).
    fn span_window(binding: &Binding) -> Option<Window> {
        let Binding::Struct { fields, .. } = binding else {
            return None;
        };
        match flatten_struct_locals(fields).as_slice() {
            [(addr, Type::U64), (count, Type::U64)] => Some(Window { addr: *addr, count: *count }),
            _ => None,
        }
    }

    /// The window an argument names, when it is a plain name bound to
    /// a `Span`. Anything else (a temporary, a field) keeps the call.
    fn span_arg_window(&self, arg: &ExprRef) -> Option<Window> {
        let Some(Expr::Identifier(sym)) = self.program.expression.get(arg) else {
            return None;
        };
        Self::span_window(self.bindings.get(&sym)?)
    }

    fn load_u64(&mut self, local: LocalId) -> ValueId {
        self.emit(InstKind::LoadLocal(local), Some(Type::U64))
            .expect("LoadLocal returns a value")
    }

    /// `count * elem_size`, without the multiply for a byte window.
    fn span_byte_len(&mut self, count: ValueId, elem_size: u64) -> ValueId {
        if elem_size == 1 {
            return count;
        }
        let size = self
            .emit(InstKind::Const(Const::U64(elem_size)), Some(Type::U64))
            .expect("Const returns a value");
        self.emit(InstKind::BinOp { op: BinOp::Mul, lhs: count, rhs: size }, Some(Type::U64))
            .expect("imul returns a value")
    }

    /// `s.copy_from(t)` / `s.move_from(t)` / `s.bytes_eq(t)` /
    /// `s.fill(b)` on the stdlib `Span<T>`, lowered to the range
    /// instruction its body performs. `Ok(None)` means "not this
    /// intrinsic here" -- nothing has been emitted, so the caller can
    /// still make the call.
    pub(super) fn lower_span_range_intrinsic(
        &mut self,
        binding: &Binding,
        target_sym: DefaultSymbol,
        method: DefaultSymbol,
        recv_type_args: &[Type],
        args: &[ExprRef],
    ) -> Result<Option<Option<ValueId>>, String> {
        if self.interner.resolve(target_sym) != Some("Span") {
            return Ok(None);
        }
        let op = match self.interner.resolve(method) {
            Some("copy_from") => SpanOp::CopyFrom,
            Some("move_from") => SpanOp::MoveFrom,
            Some("bytes_eq") => SpanOp::BytesEq,
            Some("fill") => SpanOp::Fill,
            _ => return Ok(None),
        };
        if args.len() != 1 || !self.method_is_from_std_module(target_sym, method, "span") {
            return Ok(None);
        }
        let Some(me) = Self::span_window(binding) else {
            return Ok(None);
        };
        let [elem_ty] = recv_type_args else {
            return Ok(None);
        };
        let Some(elem_size) = self.compute_byte_size(*elem_ty) else {
            return Ok(None);
        };

        if op == SpanOp::Fill {
            // `impl Span<u8>` only: the byte is the element.
            if *elem_ty != Type::U8 {
                return Ok(None);
            }
            let byte = self
                .lower_expr(&args[0])?
                .ok_or_else(|| "Span::fill value produced no value".to_string())?;
            let dest = self.load_u64(me.addr);
            let size = self.load_u64(me.count);
            self.emit(InstKind::MemSet { dest, byte, size }, None);
            return Ok(Some(None));
        }

        let Some(other) = self.span_arg_window(&args[0]) else {
            return Ok(None);
        };
        // The body's own panic text, interned by parsing `span.t`. Its
        // absence would mean the body is not the one this mirrors.
        let mismatch = match op {
            SpanOp::CopyFrom => Some("Span::copy_from length mismatch"),
            SpanOp::MoveFrom => Some("Span::move_from length mismatch"),
            _ => None,
        };
        let mismatch = match mismatch {
            Some(text) => match self.interner.get(text) {
                Some(sym) => Some(sym),
                None => return Ok(None),
            },
            None => None,
        };

        let mine = self.load_u64(me.count);
        let theirs = self.load_u64(other.count);
        let same = self
            .emit(InstKind::BinOp { op: BinOp::Eq, lhs: mine, rhs: theirs }, Some(Type::Bool))
            .expect("icmp returns a value");

        if let Some(message) = mismatch {
            self.emit_trap_unless(same, message);
            // `other` is the source, `me` the destination.
            let src = self.load_u64(other.addr);
            let dest = self.load_u64(me.addr);
            let size = self.span_byte_len(mine, elem_size);
            let kind = if op == SpanOp::CopyFrom {
                InstKind::MemCopy { src, dest, size }
            } else {
                InstKind::MemMove { src, dest, size }
            };
            self.emit(kind, None);
            return Ok(Some(None));
        }

        // `bytes_eq`: unequal lengths answer `false` without reading
        // either window -- the shorter one may end before `mine`
        // bytes -- so the comparison sits behind a branch, stored to a
        // local the way `&&` is (`lower_short_circuit`).
        let result = self.module.function_mut(self.func_id).add_local(Type::Bool);
        let compare = self.fresh_block();
        let differ = self.fresh_block();
        let merge = self.fresh_block();
        self.terminate(Terminator::Branch { cond: same, then_blk: compare, else_blk: differ });

        self.switch_to(compare);
        let a = self.load_u64(me.addr);
        let b = self.load_u64(other.addr);
        let size = self.span_byte_len(mine, elem_size);
        let eq = self
            .emit(InstKind::MemEq { a, b, size }, Some(Type::Bool))
            .expect("mem_eq returns a value");
        self.emit(InstKind::StoreLocal { dst: result, src: eq }, None);
        self.terminate(Terminator::Jump(merge));

        self.switch_to(differ);
        let no = self
            .emit(InstKind::Const(Const::Bool(false)), Some(Type::Bool))
            .expect("Const returns a value");
        self.emit(InstKind::StoreLocal { dst: result, src: no }, None);
        self.terminate(Terminator::Jump(merge));

        self.switch_to(merge);
        Ok(Some(self.emit(InstKind::LoadLocal(result), Some(Type::Bool))))
    }

    /// `val s: Span<T> = Span::from_parts(p, n)` on the stdlib `Span`:
    /// store `p`'s address and `n` into the new binding's two leaves,
    /// which is all the body does. `Ok(false)` means "not this
    /// intrinsic here" -- nothing has been emitted or bound.
    pub(super) fn lower_let_span_from_parts(
        &mut self,
        name: DefaultSymbol,
        struct_id: StructId,
        struct_name: DefaultSymbol,
        fn_name: DefaultSymbol,
        args: &[ExprRef],
    ) -> Result<bool, String> {
        if self.interner.resolve(struct_name) != Some("Span")
            || self.interner.resolve(fn_name) != Some("from_parts")
            || args.len() != 2
            || !self.method_is_from_std_module(struct_name, fn_name, "span")
        {
            return Ok(false);
        }
        // The window argument has to be a name bound to a one-leaf
        // `Ptr`; anything else keeps the call.
        let Some(Expr::Identifier(p)) = self.program.expression.get(&args[0]) else {
            return Ok(false);
        };
        let Some(Binding::Struct { fields: ptr_fields, .. }) = self.bindings.get(&p) else {
            return Ok(false);
        };
        let ptr_leaves = flatten_struct_locals(ptr_fields);
        let [(ptr_addr, Type::U64)] = ptr_leaves.as_slice() else {
            return Ok(false);
        };
        let ptr_addr = *ptr_addr;
        let fields = self.allocate_struct_fields(struct_id);
        let leaves = flatten_struct_locals(&fields);
        let [(addr, Type::U64), (count, Type::U64)] = leaves.as_slice() else {
            return Ok(false);
        };
        let (addr, count) = (*addr, *count);
        // Arguments in the order written, as the call evaluated them.
        let a = self.load_u64(ptr_addr);
        let n = self
            .lower_expr(&args[1])?
            .ok_or_else(|| "Span::from_parts length produced no value".to_string())?;
        self.emit(InstKind::StoreLocal { dst: addr, src: a }, None);
        self.emit(InstKind::StoreLocal { dst: count, src: n }, None);
        self.bindings.insert(name, Binding::Struct { struct_id, fields });
        Ok(true)
    }
}
