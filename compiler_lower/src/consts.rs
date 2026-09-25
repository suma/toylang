//! Compile-time evaluation of top-level `const` initialisers, for the
//! backends that consume the IR.
//!
//! **Mostly historical now.** Since COMPILE-TIME-EVAL C3 the driver
//! evaluates every `const` initialiser on the tree-walker before
//! lowering and rewrites the scalar ones to literals, so what arrives
//! here is normally already a literal. This pass stays because
//! `lower_program` can be called on an AST that did not go through
//! that driver, and because non-scalar initialisers (a `str`) are
//! deliberately left alone by it.
//!
//! What it must **not** do is have arithmetic of its own. Having a
//! second, weaker evaluator here is what made `const D: u64 =
//! double(21u64)` mean 42 on the tree-walker and "cannot evaluate the
//! initialiser" everywhere else, and what made `const X: u64 = 3u64 -
//! 5u64` a run-time trap on one engine and a silently wrapped number
//! on the other three. The operators below therefore delegate to
//! [`crate::fold`], the same table body lowering folds with.

use std::collections::HashMap;

use frontend::ast::{Expr, ExprRef, File};
use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ir::Const;

pub type ConstValues = HashMap<DefaultSymbol, Const>;

/// CONST-ARRAY: one `const K: [T; N] = [..]`, already flat.
///
/// The elements are laid out as bytes here, once, rather than carried
/// as a list the lowering re-reads: an index is then a load from a
/// read-only symbol, and the table costs nothing at run time. That is
/// the whole point of writing it `const` — the heap `Vec` it replaces
/// is what stopped `Sha256` from claiming `never_allocates`.
pub struct ConstArray {
    pub elem_ty: crate::ir::Type,
    /// Element width, and therefore the index stride.
    pub stride: u64,
    pub length: u64,
    pub bytes: Vec<u8>,
    /// CONST-ARRAY: a table of structs or tuples. Its element type needs
    /// the IR's struct instances to lay out, which exist only once
    /// lowering runs, so the elements are kept as their scalar leaves
    /// (declaration order) and laid out at the first read. The scalar
    /// fields above are unused for such a table.
    pub table: Option<ConstTable>,
}

/// The leaves of a compound-element `const` table (CONST-ARRAY).
pub struct ConstTable {
    pub elem_decl: frontend::type_decl::TypeDecl,
    pub elements: Vec<Vec<Const>>,
}

pub type ConstArrays = HashMap<DefaultSymbol, ConstArray>;

pub(super) fn evaluate_consts(
    program: &File,
    interner: &DefaultStringInterner,
    struct_defs: &crate::templates::StructDefs,
) -> Result<(ConstValues, ConstArrays), String> {
    let mut values: ConstValues = HashMap::new();
    let mut arrays: ConstArrays = HashMap::new();
    for c in &program.consts {
        if let Some(array) = eval_const_array(c, program, &values, interner, struct_defs)? {
            arrays.insert(c.name, array);
            continue;
        }
        let v = eval_const_expr(&c.value, program, &values, interner).ok_or_else(|| {
            format!(
                "compiler MVP cannot evaluate the initialiser for `const {}`: only literal values and references to earlier consts are supported",
                interner.resolve(c.name).unwrap_or("?")
            )
        })?;
        // The type-checker has already validated the declared type
        // against the initialiser; we don't re-check here.
        values.insert(c.name, v);
    }
    Ok((values, arrays))
}

/// A `const` whose initialiser is an array literal, laid out.
///
/// `Ok(None)` means "not an array literal" — the scalar path takes
/// it from there. An array of anything but a scalar is an error
/// rather than a fallthrough: the scalar reader would only report
/// "cannot evaluate the initialiser", which says nothing about the
/// part that is actually unsupported.
fn eval_const_array(
    decl: &frontend::ast::ConstDecl,
    program: &File,
    values: &ConstValues,
    interner: &DefaultStringInterner,
    struct_defs: &crate::templates::StructDefs,
) -> Result<Option<ConstArray>, String> {
    let Some(Expr::ArrayLiteral(items)) = program.expression.get(&decl.value) else {
        return Ok(None);
    };
    let name = interner.resolve(decl.name).unwrap_or("?");
    // A table of structs or tuples: its leaves, per element.
    if let frontend::type_decl::TypeDecl::Array(inner, _, _) = &decl.type_decl
        && let Some(elem_decl) = inner.first()
        && crate::types::lower_scalar(elem_decl).is_none()
    {
        let mut elements = Vec::with_capacity(items.len());
        for item in &items {
            let mut leaves = Vec::new();
            if !flatten_const(item, elem_decl, program, values, interner, struct_defs, &mut leaves) {
                return Err(format!(
                    "compiler MVP cannot lay out an element of `const {name}`: a table's \
                     elements must be scalars, or structs / tuples of them written as \
                     literals"
                ));
            }
            elements.push(leaves);
        }
        return Ok(Some(ConstArray {
            elem_ty: crate::ir::Type::Unit,
            stride: 0,
            length: items.len() as u64,
            bytes: Vec::new(),
            table: Some(ConstTable { elem_decl: elem_decl.clone(), elements }),
        }));
    }
    let elem_ty = match &decl.type_decl {
        frontend::type_decl::TypeDecl::Array(inner, _, _) => inner
            .first()
            .and_then(crate::types::lower_scalar)
            .ok_or_else(|| {
                format!(
                    "compiler MVP: `const {name}` is an array of a type its elements cannot                      be laid out in; only scalars are supported"
                )
            })?,
        other => {
            return Err(format!(
                "compiler MVP: `const {name}` has an array initialiser but is declared `{}`",
                crate::spelling::spell_type_decl(interner, other)
            ));
        }
    };
    let stride = scalar_byte_width(elem_ty).ok_or_else(|| {
        format!("compiler MVP: `const {name}` has elements with no fixed width")
    })?;
    let mut bytes = Vec::with_capacity(items.len() * stride as usize);
    for item in &items {
        let value = eval_const_expr(item, program, values, interner).ok_or_else(|| {
            format!(
                "compiler MVP cannot evaluate an element of `const {name}`: only literal                  values and references to earlier consts are supported"
            )
        })?;
        append_const_bytes(&mut bytes, value, stride);
    }
    Ok(Some(ConstArray {
        elem_ty,
        stride,
        length: items.len() as u64,
        bytes,
        table: None,
    }))
}

/// The scalar leaves of one compound `const` element, in the order the
/// lowering lays a value of `decl` out: struct fields in declaration
/// order (whatever order the literal names them in), tuple elements in
/// order, recursively. `false` for anything that is not a literal of
/// that shape.
fn flatten_const(
    expr: &ExprRef,
    decl: &frontend::type_decl::TypeDecl,
    program: &File,
    values: &ConstValues,
    interner: &DefaultStringInterner,
    struct_defs: &crate::templates::StructDefs,
    out: &mut Vec<Const>,
) -> bool {
    use frontend::type_decl::TypeDecl;
    if crate::types::lower_scalar(decl).is_some() {
        return match eval_const_expr(expr, program, values, interner) {
            Some(c) => {
                out.push(c);
                true
            }
            None => false,
        };
    }
    match (decl, program.expression.get(expr)) {
        (TypeDecl::Tuple(types), Some(Expr::TupleLiteral(elems))) if types.len() == elems.len() => {
            types.iter().zip(elems.iter()).all(|(t, e)| {
                flatten_const(e, t, program, values, interner, struct_defs, out)
            })
        }
        (
            TypeDecl::Struct(name, _) | TypeDecl::Identifier(name),
            Some(Expr::StructLiteral(_, fields)),
        ) => {
            let Some(template) = struct_defs.get(name) else {
                return false;
            };
            if !template.generic_params.is_empty() {
                return false;
            }
            template.fields.iter().all(|(field_name, field_ty)| {
                let written = fields
                    .iter()
                    .find(|(sym, _)| interner.resolve(*sym) == Some(field_name.as_str()));
                match written {
                    Some((_, value)) => {
                        flatten_const(value, field_ty, program, values, interner, struct_defs, out)
                    }
                    None => false,
                }
            })
        }
        _ => false,
    }
}

/// A leaf's bytes at its own width, little-endian -- the table layout
/// the lowering builds for a compound `const` (CONST-ARRAY).
pub(super) fn const_leaf_bytes(value: Const, width: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(width as usize);
    append_const_bytes(&mut out, value, width);
    out
}

/// The byte width of a scalar. `None` for anything that is not one,
/// which an array of it cannot be laid out in.
///
/// Deliberately not [`crate::array_layout::elem_stride_bytes`]: that
/// answers for a *stack array's slot*, where a compound element still
/// takes a uniform 8 bytes per leaf. A `const` table is bytes in
/// `.rodata` and only holds scalars.
fn scalar_byte_width(ty: crate::ir::Type) -> Option<u64> {
    // A `str` is a pointer, not bytes a table can be laid out in.
    ty.scalar_byte_size().filter(|_| ty != crate::ir::Type::Str)
}

/// One element's bytes, little-endian, in its own width.
///
/// Little-endian because that is what both supported targets read
/// (x86-64 and aarch64) and what `PtrRead` does on the byte the
/// address names; a big-endian port would flip this one function.
fn append_const_bytes(out: &mut Vec<u8>, value: Const, stride: u64) {
    let raw: u64 = match value {
        Const::I64(v) => v as u64,
        Const::U64(v) => v,
        Const::I32(v) => v as u32 as u64,
        Const::U32(v) => v as u64,
        Const::I16(v) => v as u16 as u64,
        Const::U16(v) => v as u64,
        Const::I8(v) => v as u8 as u64,
        Const::U8(v) => v as u64,
        Const::Bool(v) => v as u64,
        Const::F64(v) => v.to_bits(),
        Const::F32(v) => v.to_bits() as u64,
    };
    out.extend_from_slice(&raw.to_le_bytes()[..stride as usize]);
}

/// Evaluate an expression to a scalar constant, or `None` when it is
/// not one the compiler can fold: a call, a string, a struct, an
/// identifier that is not an earlier const, or an operation that
/// traps. This is the literal reader COMPILE-TIME-EVAL C3/C6 leaves
/// in place for lowering calls that did not go through the driver —
/// the driver's fold runs the calls on the IR VM and rewrites them to
/// literals first, so what arrives here is normally already flat.
///
/// Also the evaluator behind the driver's array-length resolution
/// (C5): after the fold, a computed length like `double(2u64) + 1u64`
/// is a tree of literals and folded consts, and this turns it into
/// the count.
pub fn eval_const_expr(
    expr_ref: &ExprRef,
    program: &File,
    values: &ConstValues,
    interner: &DefaultStringInterner,
) -> Option<Const> {
    eval_const_expr_in_pool(expr_ref, &program.expression, values, interner)
}

/// Pool-scoped variant of [`eval_const_expr`], for callers that hold
/// the program's pools apart from its other fields.
pub fn eval_const_expr_in_pool(
    expr_ref: &ExprRef,
    pool: &frontend::ast::ExprPool,
    values: &ConstValues,
    interner: &DefaultStringInterner,
) -> Option<Const> {
    let _ = interner;
    match pool.get(expr_ref)? {
        Expr::Int64(v) => Some(Const::I64(v)),
        Expr::UInt64(v) => Some(Const::U64(v)),
        // NUM-W: a suffixed narrow literal is its own node, and this
        // reader only knew the two wide ones — so `const S: u32 =
        // 99u32` was "only literal values ... are supported" on the
        // compiled lanes while the tree-walker read it fine.
        Expr::Int8(v) => Some(Const::I8(v)),
        Expr::Int16(v) => Some(Const::I16(v)),
        Expr::Int32(v) => Some(Const::I32(v)),
        Expr::UInt8(v) => Some(Const::U8(v)),
        Expr::UInt16(v) => Some(Const::U16(v)),
        Expr::UInt32(v) => Some(Const::U32(v)),
        // CHAR-LITERAL-NUM: a `char` is a u32 that was written as a
        // character.
        Expr::CharLiteral(v) => Some(Const::U32(v)),
        Expr::Float64(v) => Some(Const::F64(v)),
        // SIMD-F32: single-precision const initialisers.
        Expr::Float32(v) => Some(Const::F32(v)),
        Expr::True => Some(Const::Bool(true)),
        Expr::False => Some(Const::Bool(false)),
        Expr::Identifier(sym) => values.get(&sym).copied(),
        // Fold simple arithmetic / comparison so initialisers like
        // `const TWO_PI: f64 = PI + PI` work. Unsupported operators
        // bubble `None` up, which the caller turns into a compile
        // error.
        Expr::Binary(op, lhs, rhs) => {
            let l = eval_const_expr_in_pool(&lhs, pool, values, interner)?;
            let r = eval_const_expr_in_pool(&rhs, pool, values, interner)?;
            const_fold_binop(op, l, r)
        }
        Expr::Unary(op, operand) => {
            let v = eval_const_expr_in_pool(&operand, pool, values, interner)?;
            crate::fold::fold_unary(crate::fold::unaryop_for(&op)?, v)
        }
        _ => None,
    }
}

fn const_fold_binop(op: frontend::ast::Operator, l: Const, r: Const) -> Option<Const> {
    crate::fold::fold_binop(crate::fold::binop_for(&op)?, l, r)
}
