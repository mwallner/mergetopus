use std::{env, fs, path::PathBuf};

fn main() {
    let json_path = PathBuf::from("THIRDPARTY.json");
    let json_content = fs::read_to_string(&json_path).expect("Failed to read THIRDPARTY.json");

    let license_path = PathBuf::from("LICENSE");
    let license_content = fs::read_to_string(&license_path).expect("Failed to read LICENSE");

    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = PathBuf::from(out_dir).join("licenses_json.rs");

    // Serialize to Rust string literals (JSON escapes) so arbitrary content
    // (including occurrences of '"#') won't prematurely end raw string literals.
    let json_literal = serde_json::to_string(&json_content).expect("Failed to escape JSON content");
    let license_literal =
        serde_json::to_string(&license_content).expect("Failed to escape LICENSE content");

    fs::write(
        &dest_path,
        format!(
            "pub static JSON_LICENSE_DATA: &str = {};\npub static LICENSE_TEXT: &str = {};",
            json_literal, license_literal
        ),
    )
    .expect("Failed to write licenses_json.rs");
}
