#[test]
fn test_basic_generics_compilation() {
    // Just test that the new generic-related code compiles
    println!("Basic generics compilation test passed");
}

use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, StringInterner};

#[test]
fn test_type_substitution() {
    let mut interner: DefaultStringInterner = StringInterner::new();
    let t_param = interner.get_or_intern("T");
    
    // Test TypeDecl::substitute_generics
    let generic_type = TypeDecl::Generic(t_param);
    let mut substitutions = std::collections::HashMap::new();
    substitutions.insert(t_param, TypeDecl::Int64);
    
    let substituted = generic_type.substitute_generics(&substitutions);
    assert_eq!(substituted, TypeDecl::Int64);
    
    // Test array substitution
    let generic_array = TypeDecl::Array(vec![TypeDecl::Generic(t_param)], 3);
    let substituted_array = generic_array.substitute_generics(&substitutions);
    assert_eq!(substituted_array, TypeDecl::Array(vec![TypeDecl::Int64], 3));
    
    println!("Type substitution test passed");
}

#[test]
fn test_generic_instantiation_recording() {
    use frontend::type_checker::inference::{GenericInstantiation, InstantiationKind, TypeInferenceState};
    
    let mut interner: DefaultStringInterner = StringInterner::new();
    let identity_name = interner.get_or_intern("identity");
    let instantiated_name = interner.get_or_intern("identity_i64");
    let t_param = interner.get_or_intern("T");
    
    let mut inference_state = TypeInferenceState::new();
    
    // Create an instantiation
    let mut substitutions = std::collections::HashMap::new();
    substitutions.insert(t_param, TypeDecl::Int64);
    
    let instantiation = GenericInstantiation {
        original_name: identity_name,
        type_substitutions: substitutions,
        instantiated_name,
        kind: InstantiationKind::Function,
    };
    
    // Record the instantiation
    inference_state.record_instantiation(instantiation.clone());
    
    // Check that it was recorded
    let pending = inference_state.get_pending_instantiations();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0], instantiation);
    
    // Try to record the same instantiation again
    inference_state.record_instantiation(instantiation);
    
    // Should still be only one (no duplicates)
    let pending = inference_state.get_pending_instantiations();
    assert_eq!(pending.len(), 1);
    
    println!("Generic instantiation recording test passed");
}