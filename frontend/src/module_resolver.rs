use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs;
use string_interner::{DefaultSymbol, DefaultStringInterner};
use crate::ast::{Program, ImportDecl};
use crate::type_checker::TypeCheckError;
use crate::Parser;

/// Represents a resolved module with its metadata
#[derive(Debug, Clone)]
pub struct ResolvedModule {
    pub package_name: Vec<DefaultSymbol>,
    pub file_path: PathBuf,
    pub program: Program,
}

/// Module resolver for handling import statements and file discovery
#[derive(Debug)]
pub struct ModuleResolver {
    /// Cache of already loaded modules (path -> module)
    loaded_modules: HashMap<Vec<DefaultSymbol>, ResolvedModule>,
    
    /// Search paths for module resolution
    search_paths: Vec<PathBuf>,
    
    /// Dependency graph for cycle detection
    dependency_graph: HashMap<Vec<DefaultSymbol>, Vec<Vec<DefaultSymbol>>>,
    
    /// Currently resolving modules (for cycle detection)
    resolving_stack: Vec<Vec<DefaultSymbol>>,
}

impl ModuleResolver {
    /// Create a new module resolver with default search paths
    pub fn new() -> Self {
        let mut search_paths = Vec::new();
        
        // Add current directory as default search path
        search_paths.push(PathBuf::from("."));
        
        Self {
            loaded_modules: HashMap::new(),
            search_paths,
            dependency_graph: HashMap::new(),
            resolving_stack: Vec::new(),
        }
    }
    
    /// Create a module resolver with custom search paths
    pub fn with_search_paths(paths: Vec<PathBuf>) -> Self {
        Self {
            loaded_modules: HashMap::new(),
            search_paths: paths,
            dependency_graph: HashMap::new(),
            resolving_stack: Vec::new(),
        }
    }
    
    /// Add a search path for module resolution
    pub fn add_search_path<P: AsRef<Path>>(&mut self, path: P) {
        self.search_paths.push(path.as_ref().to_path_buf());
    }
    
    /// Resolve an import declaration to a module
    pub fn resolve_import(&mut self, import: &ImportDecl, current_dir: Option<&Path>, string_interner: &mut DefaultStringInterner) -> Result<ResolvedModule, TypeCheckError> {
        let module_path = &import.module_path;
        
        // Check if module is already loaded
        if let Some(module) = self.loaded_modules.get(module_path) {
            return Ok(module.clone());
        }
        
        // Check for circular dependencies
        if self.resolving_stack.contains(module_path) {
            return Err(TypeCheckError::generic_error(&format!(
                "Circular dependency detected: {} -> {}",
                self.resolving_stack.iter()
                    .map(|path| self.module_path_to_string(path, string_interner))
                    .collect::<Vec<_>>()
                    .join(" -> "),
                self.module_path_to_string(module_path, string_interner)
            )));
        }
        
        // Add to resolving stack
        self.resolving_stack.push(module_path.clone());
        
        // Find module file
        let file_path = self.find_module_file(module_path, current_dir, string_interner)?;
        
        // Load and parse module
        let module = self.load_module_from_file(&file_path, module_path, string_interner)?;
        
        // Remove from resolving stack
        self.resolving_stack.pop();
        
        // Add to dependency graph
        if let Some(current_module) = self.resolving_stack.last() {
            self.dependency_graph
                .entry(current_module.clone())
                .or_insert_with(Vec::new)
                .push(module_path.clone());
        }
        
        // Cache the module
        self.loaded_modules.insert(module_path.clone(), module.clone());
        
        Ok(module)
    }
    
    /// Find the file path for a module
    fn find_module_file(&self, module_path: &[DefaultSymbol], current_dir: Option<&Path>, string_interner: &mut DefaultStringInterner) -> Result<PathBuf, TypeCheckError> {
        // Convert symbols to string path components
        let path_components: Vec<String> = module_path.iter()
            .filter_map(|symbol| string_interner.resolve(*symbol))
            .map(|s| s.to_string())
            .collect();
        
        if path_components.len() != module_path.len() {
            return Err(TypeCheckError::generic_error("Failed to resolve module path symbols"));
        }
        
        // Try different search strategies
        let search_paths = if let Some(current) = current_dir {
            let mut paths = vec![current.to_path_buf()];
            paths.extend(self.search_paths.iter().cloned());
            paths
        } else {
            self.search_paths.clone()
        };
        
        for search_path in &search_paths {
            // Strategy 1: module_path as file (math/basic.t)
            if let Some(file_path) = self.try_file_path(search_path, &path_components) {
                return Ok(file_path);
            }
            
            // Strategy 2: module_path as directory with mod.t (math/basic/mod.t)
            if let Some(dir_path) = self.try_directory_path(search_path, &path_components) {
                return Ok(dir_path);
            }
        }
        
        Err(TypeCheckError::not_found(
            "Module",
            &path_components.join(".")
        ))
    }
    
    /// Try to find module as a direct file
    fn try_file_path(&self, search_path: &Path, components: &[String]) -> Option<PathBuf> {
        let mut file_path = search_path.to_path_buf();
        
        // Add path components as directories except the last one
        for component in &components[..components.len().saturating_sub(1)] {
            file_path.push(component);
        }
        
        // Add the last component with .t extension
        if let Some(last) = components.last() {
            file_path.push(format!("{}.t", last));
        }
        
        if file_path.exists() && file_path.is_file() {
            Some(file_path)
        } else {
            None
        }
    }
    
    /// Try to find module as a directory with mod.t
    fn try_directory_path(&self, search_path: &Path, components: &[String]) -> Option<PathBuf> {
        let mut dir_path = search_path.to_path_buf();
        
        // Add all components as directories
        for component in components {
            dir_path.push(component);
        }
        
        // Look for mod.t in the directory
        dir_path.push("mod.t");
        
        if dir_path.exists() && dir_path.is_file() {
            Some(dir_path)
        } else {
            None
        }
    }
    
    /// Load and parse a module from file
    fn load_module_from_file(&mut self, file_path: &Path, expected_package: &[DefaultSymbol], string_interner: &mut DefaultStringInterner) -> Result<ResolvedModule, TypeCheckError> {
        // Read file content
        let content = fs::read_to_string(file_path)
            .map_err(|e| TypeCheckError::generic_error(&format!("Failed to read module file {:?}: {}", file_path, e)))?;
        
        // Parse the module using the shared string_interner
        let mut module_parser = Parser::new(&content, string_interner);
        let program = module_parser.parse_program()
            .map_err(|e| TypeCheckError::generic_error(&format!("Failed to parse module {:?}: {:?}", file_path, e)))?;
        
        // Validate package declaration matches expected path
        if let Some(package_decl) = &program.package_decl {
            // Since we're using shared string_interner, we can compare symbols directly
            if package_decl.name != expected_package {
                return Err(TypeCheckError::generic_error(&format!(
                    "Package declaration mismatch: expected {:?}, found {:?}",
                    self.module_path_to_string(expected_package, string_interner),
                    self.module_path_to_string(&package_decl.name, string_interner)
                )));
            }
        }
        
        Ok(ResolvedModule {
            package_name: expected_package.to_vec(),
            file_path: file_path.to_path_buf(),
            program,
        })
    }
    
    /// Convert module path to string for display
    fn module_path_to_string(&self, path: &[DefaultSymbol], string_interner: &mut DefaultStringInterner) -> String {
        path.iter()
            .map(|symbol| string_interner.resolve(*symbol).unwrap_or("<unknown>"))
            .collect::<Vec<_>>()
            .join(".")
    }
    
    /// Check for circular dependencies in the dependency graph
    pub fn detect_cycles(&self, _string_interner: &mut DefaultStringInterner) -> Option<Vec<Vec<Vec<DefaultSymbol>>>> {
        let mut visited = HashMap::new();
        let mut rec_stack = HashMap::new();
        let mut cycles = Vec::new();
        
        for module in self.dependency_graph.keys() {
            if !visited.get(module).unwrap_or(&false) {
                self.dfs_detect_cycle(module, &mut visited, &mut rec_stack, &mut cycles, &mut Vec::new());
            }
        }
        
        if cycles.is_empty() {
            None
        } else {
            Some(cycles)
        }
    }
    
    /// DFS helper for cycle detection
    fn dfs_detect_cycle(
        &self,
        module: &Vec<DefaultSymbol>,
        visited: &mut HashMap<Vec<DefaultSymbol>, bool>,
        rec_stack: &mut HashMap<Vec<DefaultSymbol>, bool>,
        cycles: &mut Vec<Vec<Vec<DefaultSymbol>>>,
        current_path: &mut Vec<Vec<DefaultSymbol>>,
    ) {
        visited.insert(module.clone(), true);
        rec_stack.insert(module.clone(), true);
        current_path.push(module.clone());
        
        if let Some(dependencies) = self.dependency_graph.get(module) {
            for dep in dependencies {
                if !visited.get(dep).unwrap_or(&false) {
                    self.dfs_detect_cycle(dep, visited, rec_stack, cycles, current_path);
                } else if *rec_stack.get(dep).unwrap_or(&false) {
                    // Found a cycle - store the cycle starting from the dependency
                    if let Some(cycle_start) = current_path.iter().position(|m| m == dep) {
                        let mut cycle = current_path[cycle_start..].to_vec();
                        cycle.push(dep.clone()); // Complete the cycle
                        cycles.push(cycle);
                    }
                }
            }
        }
        
        current_path.pop();
        rec_stack.insert(module.clone(), false);
    }
    
    /// Get all loaded modules
    pub fn get_loaded_modules(&self) -> &HashMap<Vec<DefaultSymbol>, ResolvedModule> {
        &self.loaded_modules
    }
    
    /// Clear all cached modules (useful for testing)
    pub fn clear_cache(&mut self) {
        self.loaded_modules.clear();
        self.dependency_graph.clear();
        self.resolving_stack.clear();
    }
}

impl Default for ModuleResolver {
    fn default() -> Self {
        Self::new()
    }
}