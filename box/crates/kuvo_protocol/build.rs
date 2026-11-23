use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Deserialize;

mod generate;
#[allow(dead_code)] // The generated table uses the borrowed forms in the library.
#[path = "src/shape.rs"]
mod shape;

use shape::Shape;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Answers {
    error: Shape,
    passed_on: Vec<String>,
    routes: Vec<Route>,
}

#[derive(Deserialize)]
struct Route {
    route: String,
    feature: String,
    answers: std::collections::BTreeMap<u16, Shape>,
    body: Option<Shape>,
}

#[derive(Deserialize)]
struct FeatureVector {
    scenarios: [FeatureScenario; 1],
}

#[derive(Deserialize)]
struct FeatureScenario {
    steps: [FeatureStep; 1],
}

#[derive(Deserialize)]
struct FeatureStep {
    expect: FeatureExpect,
}

#[derive(Deserialize)]
struct FeatureExpect {
    machine: FeatureAnswer,
}

#[derive(Deserialize)]
struct FeatureAnswer {
    json: FeatureNames,
}

#[derive(Deserialize)]
struct FeatureNames {
    features: Vec<String>,
}

fn read(path: &Path, export: bool) -> io::Result<String> {
    println!("cargo:rerun-if-changed={}", path.display());
    fs::read_to_string(path).map_err(|error| {
        let hint = if export && error.kind() == io::ErrorKind::NotFound {
            " Run scripts/route-shapes.sh or scripts/build-built.sh from the repository root."
        } else {
            ""
        };
        io::Error::new(error.kind(), format!("{}: {error}.{hint}", path.display()))
    })
}

fn json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, Box<dyn Error>> {
    let text = read(path, true)?;
    serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()).into())
}

fn build() -> Result<(), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let protocol = root.join("protocol");
    let answers = json(&protocol.join("vectors/routes/shapes/answers.json"))?;
    let features: FeatureVector = json(&protocol.join("vectors/routes/features.json"))?;
    let [scenario] = features.scenarios;
    let [step] = scenario.steps;
    let features = step.expect.machine.json.features;
    if features.is_empty() || features.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err("features.json must name every feature once, in order".into());
    }

    let required = read(&protocol.join("gate-required.txt"), false)?;
    let baseline: Vec<_> = required
        .lines()
        .find_map(|line| line.strip_prefix("# Baseline: "))
        .ok_or("gate-required.txt has no # Baseline: entry")?
        .split_whitespace()
        .collect();
    let mut required: Vec<_> = required
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.starts_with('#'))
        .collect();
    required.sort_unstable();
    required.dedup();
    if baseline.is_empty()
        || baseline
            .iter()
            .any(|name| required.binary_search(name).is_err())
    {
        return Err("gate-required.txt must name baseline features in the required list".into());
    }

    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or("Cargo did not set OUT_DIR")?);
    fs::write(output.join("routes.rs"), generate::routes(answers)?)?;
    fs::write(
        output.join("features.rs"),
        format!(
            "pub const FEATURES: &[&str] = &{features:?};\npub const REQUIRED: &[&str] = &{required:?};\npub(crate) const BASELINE: &[&str] = &{baseline:?};\n"
        ),
    )?;
    Ok(())
}

fn main() -> ExitCode {
    match build() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("kuvo_protocol: {error}");
            ExitCode::FAILURE
        }
    }
}
