use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct LicenseInfo {
    license: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct Library {
    package_name: String,
    license: String,
    licenses: Vec<LicenseInfo>,
}

#[derive(Debug, Deserialize)]
struct Root {
    third_party_libraries: Vec<Library>,
}

include!(concat!(env!("OUT_DIR"), "/licenses_json.rs"));

fn parse_json(data: &str) -> Result<Root, serde_json::Error> {
    serde_json::from_str(data)
}

fn normalize_license(license: &str) -> String {
    let separators = [" OR ", "/"];
    let mut parts: Vec<&str> = Vec::new();

    for sep in &separators {
        if license.contains(sep) {
            parts = license.split(sep).collect();
            break;
        }
    }

    if parts.is_empty() {
        parts.push(license);
    }

    parts.sort();
    parts.join(" OR ")
}

pub fn print_license(full: bool, json_output: bool) {
    if json_output {
        println!("{}", JSON_LICENSE_DATA);
        return;
    }

    println!("Mergetopus is licensed under the {}", LICENSE_TEXT);
    println!("------------------------------------------------");
    println!(" Mergetopus is built using the following crates: ");
    println!("------------------------------------------------");

    let root: Root = parse_json(JSON_LICENSE_DATA).expect("Failed to parse JSON");

    if full {
        for library in root.third_party_libraries {
            println!("Package: {}", library.package_name);
            for license_info in library.licenses {
                println!("License: {}", license_info.license);
                println!("{}", license_info.text);
            }
            println!("------------------------------------------------");
        }
    } else {
        let mut license_map: HashMap<String, Vec<String>> = HashMap::new();

        for library in root.third_party_libraries {
            let normalized_license = normalize_license(&library.license);
            license_map
                .entry(normalized_license)
                .or_default()
                .push(library.package_name.clone());
        }

        for (license, packages) in license_map {
            println!("License: {}", license);
            println!("Packages: {}", packages.join(", "));
            println!("------------------------------------------------");
        }
    }
}
