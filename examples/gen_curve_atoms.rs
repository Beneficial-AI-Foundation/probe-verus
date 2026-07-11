use probe_verus::{
    build_call_graph, convert_to_atoms_with_lines, find_duplicate_code_names, parse_scip_json,
};

fn main() {
    let scip_data = parse_scip_json("data/curve_top.json").expect("Failed to parse");
    let (call_graph, symbol_to_display_name) = build_call_graph(&scip_data);

    println!("Call graph built with {} functions", call_graph.len());

    let atoms = convert_to_atoms_with_lines(&call_graph, &symbol_to_display_name);
    println!("Converted {} atoms", atoms.len());

    // Check for duplicates
    let duplicates = find_duplicate_code_names(&atoms);

    if duplicates.is_empty() {
        println!("\n✓ No duplicate code_names found!");
    } else {
        println!("\n⚠ Found {} duplicate code_name(s):", duplicates.len());
        for dup in &duplicates {
            println!("  - '{}'", dup.code_name);
        }
    }

    // Show sample From implementations
    println!("\n=== Sample From implementations ===");
    for atom in atoms
        .iter()
        .filter(|a| a.code_name.contains("From") && a.code_name.contains("window/"))
    {
        println!("{}", atom.code_name);
    }

    // Show sample Mul implementations
    println!("\n=== Sample Mul implementations ===");
    for atom in atoms
        .iter()
        .filter(|a| a.code_name.contains("Mul") && a.code_name.contains("montgomery/"))
        .take(5)
    {
        println!("{}", atom.code_name);
    }

    // Write output
    let json = serde_json::to_string_pretty(&atoms).expect("Failed to serialize");
    std::fs::write("atoms_curve_fixed.json", &json).expect("Failed to write");
    println!("\nOutput written to atoms_curve_fixed.json");
}
