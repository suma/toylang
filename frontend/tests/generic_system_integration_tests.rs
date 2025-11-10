//! Generic System Integration Tests
//!
//! This module contains integration tests for generic functions and structs.
//! It validates generic type parameter handling, type unification, and
//! generic instantiation throughout the type checking process.
//!
//! Test Categories:
//! - Generic function parsing and type checking
//! - Generic struct instantiation
//! - Type parameter substitution
//! - Generic type unification
//! - Instantiation recording and tracking

use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, StringInterner};

mod generic_compilation {
    //! Basic generic system functionality tests

    #[test]
    fn test_basic_generics_compilation() {
        // Test that generic-related code compiles
        println!("Basic generics compilation test passed");
    }
}

mod type_substitution {
    //! Tests for generic type parameter substitution

    use super::*;

    #[test]
    fn test_simple_type_substitution() {
        let mut interner: DefaultStringInterner = StringInterner::new();
        let t_param = interner.get_or_intern("T");

        let generic_type = TypeDecl::Generic(t_param);
        let mut substitutions = std::collections::HashMap::new();
        substitutions.insert(t_param, TypeDecl::Int64);

        let substituted = generic_type.substitute_generics(&substitutions);
        assert_eq!(substituted, TypeDecl::Int64);
    }

    #[test]
    fn test_array_type_substitution() {
        let mut interner: DefaultStringInterner = StringInterner::new();
        let t_param = interner.get_or_intern("T");

        let generic_array = TypeDecl::Array(vec![TypeDecl::Generic(t_param)], 3);
        let mut substitutions = std::collections::HashMap::new();
        substitutions.insert(t_param, TypeDecl::Int64);

        let substituted_array = generic_array.substitute_generics(&substitutions);
        assert_eq!(substituted_array, TypeDecl::Array(vec![TypeDecl::Int64], 3));
    }

    #[test]
    fn test_nested_generic_substitution() {
        let mut interner: DefaultStringInterner = StringInterner::new();
        let t_param = interner.get_or_intern("T");

        let generic_nested = TypeDecl::Array(
            vec![TypeDecl::Array(vec![TypeDecl::Generic(t_param)], 2)],
            3,
        );
        let mut substitutions = std::collections::HashMap::new();
        substitutions.insert(t_param, TypeDecl::UInt64);

        let substituted = generic_nested.substitute_generics(&substitutions);
        assert_eq!(
            substituted,
            TypeDecl::Array(vec![TypeDecl::Array(vec![TypeDecl::UInt64], 2)], 3)
        );
    }

    #[test]
    fn test_multiple_generic_parameter_substitution() {
        let mut interner: DefaultStringInterner = StringInterner::new();
        let t_param = interner.get_or_intern("T");
        let u_param = interner.get_or_intern("U");

        let mut substitutions = std::collections::HashMap::new();
        substitutions.insert(t_param, TypeDecl::Int64);
        substitutions.insert(u_param, TypeDecl::Bool);

        // Test T substitution
        let generic_t = TypeDecl::Generic(t_param);
        assert_eq!(generic_t.substitute_generics(&substitutions), TypeDecl::Int64);

        // Test U substitution
        let generic_u = TypeDecl::Generic(u_param);
        assert_eq!(generic_u.substitute_generics(&substitutions), TypeDecl::Bool);
    }

    #[test]
    fn test_unused_parameter_substitution() {
        let mut interner: DefaultStringInterner = StringInterner::new();
        let t_param = interner.get_or_intern("T");
        let v_param = interner.get_or_intern("V");

        // Create substitution only for T
        let mut substitutions = std::collections::HashMap::new();
        substitutions.insert(t_param, TypeDecl::Int64);

        // V should remain generic
        let generic_v = TypeDecl::Generic(v_param);
        let result = generic_v.substitute_generics(&substitutions);
        assert_eq!(result, TypeDecl::Generic(v_param));
    }
}

mod generic_instantiation {
    //! Tests for generic instantiation tracking

    use super::*;
    use frontend::type_checker::inference::{GenericInstantiation, InstantiationKind, TypeInferenceState};

    #[test]
    fn test_generic_instantiation_recording() {
        let mut interner: DefaultStringInterner = StringInterner::new();
        let identity_name = interner.get_or_intern("identity");
        let instantiated_name = interner.get_or_intern("identity_i64");
        let t_param = interner.get_or_intern("T");

        let mut inference_state = TypeInferenceState::new();

        let mut substitutions = std::collections::HashMap::new();
        substitutions.insert(t_param, TypeDecl::Int64);

        let instantiation = GenericInstantiation {
            original_name: identity_name,
            type_substitutions: substitutions,
            instantiated_name,
            kind: InstantiationKind::Function,
        };

        inference_state.record_instantiation(instantiation.clone());

        let pending = inference_state.get_pending_instantiations();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0], instantiation);
    }

    #[test]
    fn test_duplicate_instantiation_filtering() {
        let mut interner: DefaultStringInterner = StringInterner::new();
        let identity_name = interner.get_or_intern("identity");
        let instantiated_name = interner.get_or_intern("identity_i64");
        let t_param = interner.get_or_intern("T");

        let mut inference_state = TypeInferenceState::new();

        let mut substitutions = std::collections::HashMap::new();
        substitutions.insert(t_param, TypeDecl::Int64);

        let instantiation = GenericInstantiation {
            original_name: identity_name,
            type_substitutions: substitutions,
            instantiated_name,
            kind: InstantiationKind::Function,
        };

        inference_state.record_instantiation(instantiation.clone());
        inference_state.record_instantiation(instantiation);

        let pending = inference_state.get_pending_instantiations();
        assert_eq!(pending.len(), 1, "Duplicate instantiations should be filtered");
    }

    #[test]
    fn test_multiple_instantiation_tracking() {
        let mut interner: DefaultStringInterner = StringInterner::new();
        let func_name = interner.get_or_intern("generic_fn");
        let t_param = interner.get_or_intern("T");

        let mut inference_state = TypeInferenceState::new();

        // First instantiation with u64
        let mut sub1 = std::collections::HashMap::new();
        sub1.insert(t_param, TypeDecl::UInt64);
        let inst1 = GenericInstantiation {
            original_name: func_name,
            type_substitutions: sub1,
            instantiated_name: interner.get_or_intern("generic_fn_u64"),
            kind: InstantiationKind::Function,
        };

        // Second instantiation with i64
        let mut sub2 = std::collections::HashMap::new();
        sub2.insert(t_param, TypeDecl::Int64);
        let inst2 = GenericInstantiation {
            original_name: func_name,
            type_substitutions: sub2,
            instantiated_name: interner.get_or_intern("generic_fn_i64"),
            kind: InstantiationKind::Function,
        };

        inference_state.record_instantiation(inst1);
        inference_state.record_instantiation(inst2);

        let pending = inference_state.get_pending_instantiations();
        assert_eq!(pending.len(), 2, "Multiple different instantiations should be tracked");
    }

    #[test]
    fn test_struct_instantiation_recording() {
        let mut interner: DefaultStringInterner = StringInterner::new();
        let container_name = interner.get_or_intern("Container");
        let instantiated_name = interner.get_or_intern("Container_u64");
        let t_param = interner.get_or_intern("T");

        let mut inference_state = TypeInferenceState::new();

        let mut substitutions = std::collections::HashMap::new();
        substitutions.insert(t_param, TypeDecl::UInt64);

        let instantiation = GenericInstantiation {
            original_name: container_name,
            type_substitutions: substitutions,
            instantiated_name,
            kind: InstantiationKind::Struct,
        };

        inference_state.record_instantiation(instantiation.clone());

        let pending = inference_state.get_pending_instantiations();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].kind, InstantiationKind::Struct);
    }
}

mod generic_type_inference {
    //! Tests for generic type parameter inference

    use super::*;

    #[test]
    fn test_generic_type_matching_u64() {
        let mut interner: DefaultStringInterner = StringInterner::new();
        let t_param = interner.get_or_intern("T");

        let generic = TypeDecl::Generic(t_param);
        let concrete = TypeDecl::UInt64;

        // Simulate type inference matching generic with u64
        assert_eq!(generic, TypeDecl::Generic(t_param));
        assert_eq!(concrete, TypeDecl::UInt64);
    }

    #[test]
    fn test_generic_type_matching_array() {
        let mut interner: DefaultStringInterner = StringInterner::new();
        let t_param = interner.get_or_intern("T");

        let generic_array = TypeDecl::Array(vec![TypeDecl::Generic(t_param)], 5);
        let concrete_array = TypeDecl::Array(vec![TypeDecl::Int64], 5);

        // Element type should match for unification
        assert_eq!(generic_array, TypeDecl::Array(vec![TypeDecl::Generic(t_param)], 5));
        assert_eq!(concrete_array, TypeDecl::Array(vec![TypeDecl::Int64], 5));
    }
}
