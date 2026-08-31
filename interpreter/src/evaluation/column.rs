//! DATA-ORIENTED Phase 1: the tree-walking interpreter's column
//! window (`ps.mass`).
//!
//! The compiled lanes hand back a `Column<T>` (`core/std/column.t`)
//! holding an address, a length and a stride. This engine has no
//! addresses for an array — it holds one as a `Vec` of element values
//! — and no bytes for a `SoaVec`'s columns, so a window here is the
//! *source* plus the field to select, sharing the source's `Rc` so a
//! write through either is seen by both.
//!
//! It is spelled as an ordinary `Column` struct so it prints,
//! compares and passes like the type the checker says it is, and
//! `evaluate_method_call` answers its four methods before they can
//! reach the stdlib bodies that read `self.addr`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::error::InterpreterError;
use crate::object::{Object, RcObject};
use super::{EvaluationContext, EvaluationResult};

/// The two fields the window carries in place of an address — the
/// array (or vec) it views and the field it selects. Named with a
/// `__` prefix because a user struct cannot declare a field starting
/// that way.
pub(super) const COLUMN_SOURCE_FIELD: &str = "__column_source";
pub(super) const COLUMN_FIELD_FIELD: &str = "__column_field";

impl EvaluationContext<'_> {
    /// The tree-walker's column window: the *source* it views (an
    /// array, or a `SoaVec`) plus the field to select, sharing the
    /// source's `Rc` so a later write through either is seen by both.
    ///
    /// The compiled lanes hand back a `Column<T>` holding an address,
    /// a length and a stride; this engine has no addresses for an
    /// array and no bytes for a vec's columns, so it carries what it
    /// does have. Both are spelled as a `Column` struct, and
    /// `evaluate_method_call` answers the window's methods before they
    /// can reach the stdlib bodies that read `self.addr`.
    pub(super) fn build_column_view(&mut self, source: RcObject, field: DefaultSymbol) -> Object {
        let column = self.string_interner.get_or_intern("Column");
        let field_name = self
            .string_interner
            .resolve(field)
            .unwrap_or("<unknown>")
            .to_string();
        let source_key = self.string_interner.get_or_intern(COLUMN_SOURCE_FIELD);
        let field_key = self.string_interner.get_or_intern(COLUMN_FIELD_FIELD);
        let mut fields = HashMap::new();
        fields.insert(source_key, source);
        fields.insert(field_key, Rc::new(RefCell::new(Object::String(field_name))));
        Object::Struct {
            type_name: column,
            fields: Box::new(fields),
            type_args: Vec::new(),
        }
    }

}

/// The `(source, field)` a column window views, if this struct is one
/// this engine built. A `Column` value from anywhere else (there is
/// no other way to make one today) has no source fields and falls
/// through to ordinary method dispatch.
pub(super) fn column_source(
    interner: &DefaultStringInterner,
    fields: &HashMap<DefaultSymbol, RcObject>,
) -> Option<(RcObject, DefaultSymbol)> {
    let source_key = interner.get(COLUMN_SOURCE_FIELD)?;
    let field_key = interner.get(COLUMN_FIELD_FIELD)?;
    let array = fields.get(&source_key)?.clone();
    let name = match &*fields.get(&field_key)?.borrow() {
        Object::String(name) => name.clone(),
        _ => return None,
    };
    let field = interner.get(&name)?;
    Some((array, field))
}


impl EvaluationContext<'_> {
    /// How many elements a window's source holds — an array's length,
    /// or a `SoaVec`'s live `len` (its capacity is not readable).
    fn column_len(&self, source: &RcObject) -> Result<u64, InterpreterError> {
        let borrowed = source.borrow();
        match &*borrowed {
            Object::Array(elements) => Ok(elements.len() as u64),
            Object::Struct { fields, .. } => {
                let len_key = self.string_interner.get("len").ok_or_else(|| {
                    InterpreterError::InternalError("column window: no `len` field".to_string())
                })?;
                fields
                    .get(&len_key)
                    .and_then(|v| v.borrow().try_unwrap_uint64().ok())
                    .ok_or_else(|| {
                        InterpreterError::InternalError(
                            "column window: source has no readable `len`".to_string(),
                        )
                    })
            }
            other => Err(InterpreterError::InternalError(format!(
                "column window over neither an array nor a vec: {other:?}"
            ))),
        }
    }

    /// Element `index` of a window's source.
    ///
    /// A `SoaVec`'s elements live in the heap's typed slots, one per
    /// index — the shape `__builtin_soa_write` gives them on this
    /// engine (see `builtin.rs`). Returning the stored `Rc` rather
    /// than a copy is what makes a window a view of the vec.
    fn column_element(
        &self,
        source: &RcObject,
        index: u64,
    ) -> Result<RcObject, InterpreterError> {
        let borrowed = source.borrow();
        match &*borrowed {
            Object::Array(elements) => Ok(elements[index as usize].clone()),
            Object::Struct { fields, .. } => {
                let data_key = self.string_interner.get("data").ok_or_else(|| {
                    InterpreterError::InternalError("column window: no `data` field".to_string())
                })?;
                let addr = fields
                    .get(&data_key)
                    .and_then(|v| v.borrow().try_unwrap_pointer().ok())
                    .ok_or_else(|| {
                        InterpreterError::InternalError(
                            "column window: vec has no buffer".to_string(),
                        )
                    })?;
                self.heap_manager
                    .borrow()
                    .typed_read(addr, index as usize)
                    .ok_or_else(|| {
                        InterpreterError::InternalError(
                            "column window: element was never written".to_string(),
                        )
                    })
            }
            other => Err(InterpreterError::InternalError(format!(
                "column window over neither an array nor a vec: {other:?}"
            ))),
        }
    }

    /// `Column<T>`'s methods against an array- or vec-backed window.
    ///
    /// Bounds messages match `core/std/column.t` exactly — the same
    /// program must fail with the same text whichever engine runs it.
    pub(super) fn column_method(
        &mut self,
        method_name: &str,
        array: RcObject,
        field: DefaultSymbol,
        args: &[RcObject],
    ) -> Result<EvaluationResult, InterpreterError> {
        let len = self.column_len(&array)?;
        let index_arg = |which: &str| -> Result<u64, InterpreterError> {
            args.first()
                .ok_or_else(|| {
                    InterpreterError::InternalError(format!("Column::{which} takes an index"))
                })?
                .borrow()
                .try_unwrap_uint64()
                .map_err(|_| {
                    InterpreterError::InternalError(format!(
                        "Column::{which} expects a u64 index"
                    ))
                })
        };
        match method_name {
            "len" => Ok(EvaluationResult::Value((Object::UInt64(len)).into())),
            "is_empty" => Ok(EvaluationResult::Value((Object::Bool(len == 0)).into())),
            "get" => {
                let index = index_arg("get")?;
                if index >= len {
                    return Err(self.panic_error("Column::get index out of bounds".to_string(), None));
                }
                let element = self.column_element(&array, index)?;
                let value = {
                    let borrowed = element.borrow();
                    match &*borrowed {
                        Object::Struct { fields, .. } => fields.get(&field).cloned(),
                        _ => None,
                    }
                };
                value.map(|v| EvaluationResult::Value(v.into())).ok_or_else(|| {
                    let name = self.string_interner.resolve(field).unwrap_or("<unknown>");
                    InterpreterError::InternalError(format!(
                        "column window: element has no field `{name}`"
                    ))
                })
            }
            "set" => {
                let index = index_arg("set")?;
                if index >= len {
                    return Err(self.panic_error("Column::set index out of bounds".to_string(), None));
                }
                let value = args.get(1).cloned().ok_or_else(|| {
                    InterpreterError::InternalError("Column::set takes a value".to_string())
                })?;
                let element = self.column_element(&array, index)?;
                let mut borrowed = element.borrow_mut();
                match &mut *borrowed {
                    Object::Struct { fields, .. } => {
                        fields.insert(field, value);
                        Ok(EvaluationResult::Value((Object::Unit).into()))
                    }
                    other => Err(InterpreterError::InternalError(format!(
                        "column window: element is not a struct: {other:?}"
                    ))),
                }
            }
            other => Err(InterpreterError::InternalError(format!(
                "`Column` has no method `{other}`"
            ))),
        }
    }
}
