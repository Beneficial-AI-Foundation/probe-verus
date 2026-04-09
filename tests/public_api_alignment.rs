//! Cross-tool alignment test for `is-public-api`.
//!
//! Compares `is-public-api` values between probe-verus and probe-rust for all
//! functions that share the same `rust-qualified-name` (RQN). Uses
//! `cargo public-api` as ground truth when available.
//!
//! ## Requirements
//!
//! - `probe-rust` installed and on PATH
//! - `probe-verus` built (this crate)
//! - `cargo-public-api` installed (`cargo install cargo-public-api`)
//! - A fixture project (dalek-verus) accessible at `../dalek-verus/curve25519-dalek`
//!
//! Run with:
//! ```text
//! cargo test --test public_api_alignment -- --nocapture --ignored
//! ```

use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

const DALEK_VERUS_PATH: &str = "../dalek-verus/curve25519-dalek";

/// Load atoms from a probe JSON file (envelope-aware).
fn load_atoms(path: &Path) -> HashMap<String, Value> {
    let content = std::fs::read_to_string(path).expect("failed to read JSON file");
    let json: Value = serde_json::from_str(&content).expect("failed to parse JSON");

    let data = if let Some(d) = json.get("data") {
        d.clone()
    } else {
        json
    };

    let obj = data.as_object().expect("data should be an object");
    obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// Extract RQN-keyed map: rqn → (is-public-api, code-name, display-name)
fn rqn_map(atoms: &HashMap<String, Value>) -> HashMap<String, (Option<bool>, String, String)> {
    let mut map = HashMap::new();
    for (code_name, atom) in atoms {
        if let Some(rqn) = atom.get("rust-qualified-name").and_then(|v| v.as_str()) {
            let is_pub_api = atom.get("is-public-api").and_then(|v| v.as_bool());
            let display = atom
                .get("display-name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            map.insert(rqn.to_string(), (is_pub_api, code_name.clone(), display));
        }
    }
    map
}

#[test]
#[ignore]
fn alignment_probe_verus_vs_cargo_public_api() {
    let project_path = Path::new(DALEK_VERUS_PATH);
    if !project_path.exists() {
        eprintln!(
            "SKIP: dalek-verus fixture not found at {}",
            project_path.display()
        );
        return;
    }

    // Run cargo public-api to get ground truth
    let public_api_output = Command::new("cargo")
        .arg("public-api")
        .current_dir(project_path)
        .output();

    let public_rqns = match public_api_output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            probe_verus::public_api::parse_public_api_output(&stdout)
        }
        _ => {
            eprintln!("SKIP: cargo public-api not available or failed");
            return;
        }
    };

    eprintln!(
        "cargo public-api returned {} public RQNs",
        public_rqns.len()
    );

    // Run probe-verus extract with --with-public-api
    let verus_output = Command::new("cargo")
        .args([
            "run",
            "--",
            "extract",
            "--skip-verify",
            "--with-public-api",
            "--allow-duplicates",
        ])
        .arg(project_path)
        .output()
        .expect("failed to run probe-verus extract");

    if !verus_output.status.success() {
        let stderr = String::from_utf8_lossy(&verus_output.stderr);
        eprintln!("probe-verus extract failed:\n{stderr}");
        return;
    }

    // Find the atoms.json output (sort entries for determinism)
    let probes_dir = project_path.join(".verilib").join("probes");
    let mut entries: Vec<_> = std::fs::read_dir(&probes_dir)
        .expect("probes dir should exist")
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    let atoms_file = entries
        .iter()
        .find(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with("_atoms.json"))
        })
        .map(|e| e.path());

    let atoms_path = match atoms_file {
        Some(p) => p,
        None => {
            eprintln!("SKIP: no atoms.json found in {}", probes_dir.display());
            return;
        }
    };

    let atoms = load_atoms(&atoms_path);
    let verus_rqn_map = rqn_map(&atoms);

    eprintln!(
        "probe-verus: {} atoms total, {} with RQN",
        atoms.len(),
        verus_rqn_map.len()
    );

    // Compare: for every function in probe-verus with an RQN,
    // check if cargo public-api agrees on is-public-api
    let mut agrees = 0usize;
    let mut disagrees = Vec::new();
    let mut no_opinion = 0usize;

    for (rqn, (is_pub_api, code_name, display_name)) in &verus_rqn_map {
        let cargo_says_public = public_rqns.contains(rqn);
        match is_pub_api {
            Some(v) => {
                if *v == cargo_says_public {
                    agrees += 1;
                } else {
                    disagrees.push((
                        rqn.clone(),
                        display_name.clone(),
                        code_name.clone(),
                        *v,
                        cargo_says_public,
                    ));
                }
            }
            None => {
                no_opinion += 1;
            }
        }
    }

    eprintln!();
    eprintln!("=== Alignment Results ===");
    eprintln!("  Agree:    {agrees}");
    eprintln!("  Disagree: {}", disagrees.len());
    eprintln!("  No opinion (is-public-api=null): {no_opinion}");

    if !disagrees.is_empty() {
        eprintln!();
        eprintln!("Disagreements (probe-verus vs cargo-public-api):");
        for (rqn, display, _code_name, verus_val, cargo_val) in &disagrees {
            eprintln!("  {display} ({rqn}): verus={verus_val}, cargo={cargo_val}");
        }
    }

    // With --with-public-api, there should be zero disagreements for
    // functions that have an RQN (the override should have aligned them).
    assert_eq!(
        disagrees.len(),
        0,
        "expected zero disagreements when using --with-public-api"
    );
}

#[test]
#[ignore]
fn alignment_probe_verus_vs_probe_rust() {
    let project_path = Path::new(DALEK_VERUS_PATH);
    if !project_path.exists() {
        eprintln!(
            "SKIP: dalek-verus fixture not found at {}",
            project_path.display()
        );
        return;
    }

    // Check if probe-rust is available
    let probe_rust_check = Command::new("probe-rust").arg("--version").output();
    if probe_rust_check.is_err() || !probe_rust_check.unwrap().status.success() {
        eprintln!("SKIP: probe-rust not found on PATH");
        return;
    }

    // Run probe-rust extract
    let rust_output = Command::new("probe-rust")
        .args(["extract", "--skip-verify"])
        .arg(project_path)
        .output()
        .expect("failed to run probe-rust");

    if !rust_output.status.success() {
        let stderr = String::from_utf8_lossy(&rust_output.stderr);
        eprintln!("probe-rust extract failed:\n{stderr}");
        return;
    }

    // Find probe-rust atoms
    let probes_dir = project_path.join(".verilib").join("probes");
    let mut entries: Vec<_> = std::fs::read_dir(&probes_dir)
        .expect("probes dir should exist")
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    let rust_atoms_file = entries
        .iter()
        .find(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rust_") && n.ends_with("_atoms.json"))
        })
        .map(|e| e.path());

    let verus_atoms_file = entries
        .iter()
        .find(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("verus_") && n.ends_with("_atoms.json"))
        })
        .map(|e| e.path());

    let (rust_path, verus_path) = match (rust_atoms_file, verus_atoms_file) {
        (Some(r), Some(v)) => (r, v),
        _ => {
            eprintln!("SKIP: could not find both rust and verus atoms.json");
            return;
        }
    };

    let rust_atoms = load_atoms(&rust_path);
    let verus_atoms = load_atoms(&verus_path);
    let rust_map = rqn_map(&rust_atoms);
    let verus_map = rqn_map(&verus_atoms);

    eprintln!(
        "probe-rust:  {} atoms, {} with RQN",
        rust_atoms.len(),
        rust_map.len()
    );
    eprintln!(
        "probe-verus: {} atoms, {} with RQN",
        verus_atoms.len(),
        verus_map.len()
    );

    // Find shared RQNs and compare is-public-api
    let mut shared = 0usize;
    let mut agrees = 0usize;
    let mut disagrees = Vec::new();

    for (rqn, (rust_pub, _rust_cn, rust_dn)) in &rust_map {
        if let Some((verus_pub, _verus_cn, verus_dn)) = verus_map.get(rqn) {
            shared += 1;
            match (rust_pub, verus_pub) {
                (Some(r), Some(v)) if r == v => agrees += 1,
                (Some(r), Some(v)) => {
                    disagrees.push((rqn.clone(), rust_dn.clone(), verus_dn.clone(), *r, *v));
                }
                _ => {}
            }
        }
    }

    let rust_only = rust_map.len() - shared;
    let verus_only: usize = verus_map
        .keys()
        .filter(|k| !rust_map.contains_key(*k))
        .count();

    eprintln!();
    eprintln!("=== Cross-tool RQN Alignment ===");
    eprintln!("  Shared RQNs: {shared}");
    eprintln!("  Agree on is-public-api: {agrees}");
    eprintln!("  Disagree: {}", disagrees.len());
    eprintln!("  probe-rust only: {rust_only}");
    eprintln!("  probe-verus only: {verus_only}");

    if !disagrees.is_empty() {
        eprintln!();
        eprintln!("Disagreements:");
        for (rqn, rust_dn, verus_dn, r, v) in &disagrees {
            eprintln!("  {rqn}: rust({rust_dn})={r}, verus({verus_dn})={v}");
        }
    }
}
