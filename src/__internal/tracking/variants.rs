//! Variant manifest: per-project list of approved parameter values
//! and module-version swaps a perf plan is allowed to reference.
//!
//! Lives at `<project>/variants.toml` alongside the design. The plan
//! executor checks every sweep against the manifest -- a plan that
//! references `clock_ghz = 1.7` when the manifest declares
//! `values = [1.0, 1.5, 2.0]` fails validation before any simulation
//! runs. This closes the "no approved-variants registry" gap called
//! out in `docs/brainstorming/perf-plan-formalization.md`: every
//! perf plan becomes reproducible across teams and re-runnable on a
//! different commit because the approved sweep surface is checked-in
//! and machine-readable.
//!
//! Schema (TOML):
//!
//! ```toml
//! schema_version = 1
//!
//! [parameters.clock_ghz]
//! values = [1.0, 1.5, 2.0]
//! default = 1.5
//!
//! [parameters.fifo_depth]
//! values = [8, 16, 32, 64]
//! default = 16
//!
//! [modules.arbiter]
//! variants = ["round_robin", "priority", "weighted"]
//! default = "round_robin"
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const SUPPORTED_SCHEMA_VERSION: u32 = 1;

/// Standard filename. The plan executor and sim-flow CLI look here
/// by default; callers can pass an explicit path to `load` to opt
/// out of the convention.
pub const MANIFEST_FILENAME: &str = "variants.toml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VariantManifest {
    pub schema_version: u32,
    #[serde(default)]
    pub parameters: BTreeMap<String, ParameterVariant>,
    #[serde(default)]
    pub modules: BTreeMap<String, ModuleVariant>,
}

/// Approved scalar values for one parameter. `default` MUST appear
/// in `values`; validation rejects manifests where it doesn't so a
/// stale default is caught at load time, not mid-sweep.
///
/// `rendering` is an optional per-parameter axis-rendering hint.
/// Numeric parameters don't usually need it -- their natural order
/// drives the axis. Enum-valued parameters (`["write_through",
/// "write_back", "write_around"]`) carry no numeric order, so chart
/// libraries can't pick a display sequence or color scheme without
/// a convention. `rendering.labels` and `rendering.colors` align
/// with `values` index-by-index.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParameterVariant {
    pub values: Vec<toml::Value>,
    pub default: toml::Value,
    #[serde(default)]
    pub rendering: Option<AxisRendering>,
}

/// Per-parameter axis-rendering hints for chart libraries. Both
/// fields are optional; provide whichever ones the chart layer
/// needs. Each non-empty vector must have the same length as the
/// parent `ParameterVariant::values`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AxisRendering {
    /// Display label per value (e.g. "Write-through" for the TOML
    /// value "write_through"). Defaults to the value's string form
    /// when omitted.
    #[serde(default)]
    pub labels: Option<Vec<String>>,
    /// Color per value, in any format the renderer accepts
    /// (typically hex "#rrggbb" or a named CSS color).
    #[serde(default)]
    pub colors: Option<Vec<String>>,
}

/// Approved module implementations for one topology slot. The plan
/// can reference any name in `variants`; selecting a name not in the
/// list is a validation error.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModuleVariant {
    pub variants: Vec<String>,
    pub default: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("variants.toml: unsupported schema_version {found} (this build supports {supported})")]
    UnsupportedSchemaVersion { found: u32, supported: u32 },
    #[error("variants.toml parameter `{name}`: default {default:?} is not in values {values:?}")]
    ParameterDefaultNotInValues {
        name: String,
        default: toml::Value,
        values: Vec<toml::Value>,
    },
    #[error("variants.toml parameter `{name}`: values list is empty")]
    ParameterValuesEmpty { name: String },
    #[error(
        "variants.toml parameter `{name}` rendering.{field}: length {found} doesn't match \
         values length {expected}"
    )]
    RenderingLengthMismatch {
        name: String,
        field: &'static str,
        expected: usize,
        found: usize,
    },
    #[error("variants.toml module `{slot}`: default `{default}` is not in variants {variants:?}")]
    ModuleDefaultNotInVariants {
        slot: String,
        default: String,
        variants: Vec<String>,
    },
    #[error("variants.toml module `{slot}`: variants list is empty")]
    ModuleVariantsEmpty { slot: String },
}

impl VariantManifest {
    /// Validate internal invariants: schema version is supported,
    /// each parameter has a non-empty `values` list with `default`
    /// in it, each module slot has a non-empty `variants` list with
    /// `default` in it.
    pub fn validate(&self) -> std::result::Result<(), ValidationError> {
        if self.schema_version != SUPPORTED_SCHEMA_VERSION {
            return Err(ValidationError::UnsupportedSchemaVersion {
                found: self.schema_version,
                supported: SUPPORTED_SCHEMA_VERSION,
            });
        }
        for (name, param) in &self.parameters {
            if param.values.is_empty() {
                return Err(ValidationError::ParameterValuesEmpty { name: name.clone() });
            }
            if !param.values.iter().any(|v| v == &param.default) {
                return Err(ValidationError::ParameterDefaultNotInValues {
                    name: name.clone(),
                    default: param.default.clone(),
                    values: param.values.clone(),
                });
            }
            if let Some(rendering) = &param.rendering {
                let n = param.values.len();
                if let Some(labels) = &rendering.labels
                    && labels.len() != n
                {
                    return Err(ValidationError::RenderingLengthMismatch {
                        name: name.clone(),
                        field: "labels",
                        expected: n,
                        found: labels.len(),
                    });
                }
                if let Some(colors) = &rendering.colors
                    && colors.len() != n
                {
                    return Err(ValidationError::RenderingLengthMismatch {
                        name: name.clone(),
                        field: "colors",
                        expected: n,
                        found: colors.len(),
                    });
                }
            }
        }
        for (slot, module) in &self.modules {
            if module.variants.is_empty() {
                return Err(ValidationError::ModuleVariantsEmpty { slot: slot.clone() });
            }
            if !module.variants.iter().any(|v| v == &module.default) {
                return Err(ValidationError::ModuleDefaultNotInVariants {
                    slot: slot.clone(),
                    default: module.default.clone(),
                    variants: module.variants.clone(),
                });
            }
        }
        Ok(())
    }

    /// True if `value` is in `parameters[name].values`. Returns
    /// false if the parameter is not declared at all -- callers
    /// usually want to detect undeclared parameters as a separate
    /// error.
    pub fn is_parameter_value_approved(&self, name: &str, value: &toml::Value) -> bool {
        self.parameters
            .get(name)
            .is_some_and(|p| p.values.iter().any(|v| v == value))
    }

    /// True if `variant` is in `modules[slot].variants`.
    pub fn is_module_variant_approved(&self, slot: &str, variant: &str) -> bool {
        self.modules
            .get(slot)
            .is_some_and(|m| m.variants.iter().any(|v| v == variant))
    }

    pub fn parameter(&self, name: &str) -> Option<&ParameterVariant> {
        self.parameters.get(name)
    }

    pub fn module(&self, slot: &str) -> Option<&ModuleVariant> {
        self.modules.get(slot)
    }
}

/// Load and validate a manifest from `path`. Validation errors
/// surface as `Error::Config` so the CLI prints the underlying
/// message; the loader is otherwise a thin wrapper around
/// `toml::from_str`.
pub fn load(path: &Path) -> Result<VariantManifest> {
    let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let manifest: VariantManifest = toml::from_str(&text).map_err(|source| Error::TomlParse {
        path: path.to_path_buf(),
        source,
    })?;
    manifest
        .validate()
        .map_err(|err| Error::Config(format!("{}: {err}", path.display())))?;
    Ok(manifest)
}

/// Convenience: load the project's default `variants.toml` if it
/// exists. Returns `Ok(None)` when the file is absent (the project
/// hasn't adopted variant manifests yet); returns `Err` on
/// load/parse/validation failure.
pub fn load_project(project_dir: &Path) -> Result<Option<VariantManifest>> {
    let path: PathBuf = project_dir.join(MANIFEST_FILENAME);
    if !path.exists() {
        return Ok(None);
    }
    load(&path).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(dir: &Path, text: &str) -> PathBuf {
        let path = dir.join(MANIFEST_FILENAME);
        std::fs::write(&path, text).expect("write manifest");
        path
    }

    #[test]
    fn loads_valid_manifest_with_parameters_and_modules() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_manifest(
            tmp.path(),
            r#"
schema_version = 1

[parameters.clock_ghz]
values = [1.0, 1.5, 2.0]
default = 1.5

[parameters.fifo_depth]
values = [8, 16, 32, 64]
default = 16

[modules.arbiter]
variants = ["round_robin", "priority", "weighted"]
default = "round_robin"
"#,
        );
        let manifest = load(&path).expect("manifest loads");
        assert_eq!(manifest.schema_version, 1);
        assert_eq!(manifest.parameters.len(), 2);
        assert_eq!(manifest.modules.len(), 1);
        assert!(
            manifest.is_parameter_value_approved("clock_ghz", &toml::Value::Float(1.5)),
            "1.5 GHz should be approved"
        );
        assert!(
            !manifest.is_parameter_value_approved("clock_ghz", &toml::Value::Float(1.7)),
            "1.7 GHz is NOT approved"
        );
        assert!(manifest.is_module_variant_approved("arbiter", "priority"));
        assert!(!manifest.is_module_variant_approved("arbiter", "fifo"));
    }

    #[test]
    fn rejects_parameter_default_not_in_values() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_manifest(
            tmp.path(),
            r#"
schema_version = 1

[parameters.clock_ghz]
values = [1.0, 1.5, 2.0]
default = 99.0
"#,
        );
        let err = load(&path).expect_err("should reject mismatched default");
        let msg = format!("{err}");
        assert!(
            msg.contains("default") && msg.contains("clock_ghz"),
            "error should name the offending parameter: {msg}"
        );
    }

    #[test]
    fn rejects_module_default_not_in_variants() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_manifest(
            tmp.path(),
            r#"
schema_version = 1

[modules.arbiter]
variants = ["round_robin", "priority"]
default = "weighted"
"#,
        );
        let err = load(&path).expect_err("should reject mismatched default");
        let msg = format!("{err}");
        assert!(
            msg.contains("default") && msg.contains("arbiter"),
            "error should name the offending slot: {msg}"
        );
    }

    #[test]
    fn rejects_empty_values_list() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_manifest(
            tmp.path(),
            r#"
schema_version = 1

[parameters.clock_ghz]
values = []
default = 1.0
"#,
        );
        let err = load(&path).expect_err("should reject empty values");
        let msg = format!("{err}");
        assert!(msg.contains("empty"), "error should mention empty: {msg}");
    }

    #[test]
    fn rejects_unsupported_schema_version() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_manifest(
            tmp.path(),
            r#"
schema_version = 99
"#,
        );
        let err = load(&path).expect_err("should reject future version");
        let msg = format!("{err}");
        assert!(
            msg.contains("schema_version"),
            "expected schema error: {msg}"
        );
    }

    #[test]
    fn load_project_returns_none_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = load_project(tmp.path()).expect("absent file is not an error");
        assert!(manifest.is_none());
    }

    #[test]
    fn load_project_loads_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(
            tmp.path(),
            r#"
schema_version = 1

[parameters.fifo_depth]
values = [8, 16]
default = 8
"#,
        );
        let manifest = load_project(tmp.path()).expect("loads");
        let manifest = manifest.expect("present");
        assert_eq!(manifest.parameters.len(), 1);
    }

    #[test]
    fn empty_manifest_is_valid() {
        // A project that declares the manifest but hasn't filled in
        // parameters or modules yet is still well-formed.
        let manifest = VariantManifest {
            schema_version: 1,
            parameters: BTreeMap::new(),
            modules: BTreeMap::new(),
        };
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn loads_enum_parameter_with_axis_rendering() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_manifest(
            tmp.path(),
            r##"
schema_version = 1

[parameters.write_policy]
values = ["write_through", "write_back", "write_around"]
default = "write_through"

[parameters.write_policy.rendering]
labels = ["Write-through", "Write-back", "Write-around"]
colors = ["#1f77b4", "#ff7f0e", "#2ca02c"]
"##,
        );
        let manifest = load(&path).expect("loads");
        let param = manifest.parameter("write_policy").expect("present");
        let rendering = param.rendering.as_ref().expect("rendering attached");
        let labels = rendering.labels.as_ref().expect("labels");
        assert_eq!(labels[1], "Write-back");
        let colors = rendering.colors.as_ref().expect("colors");
        assert_eq!(colors[0], "#1f77b4");
    }

    #[test]
    fn rejects_rendering_labels_length_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_manifest(
            tmp.path(),
            r#"
schema_version = 1

[parameters.write_policy]
values = ["a", "b", "c"]
default = "a"

[parameters.write_policy.rendering]
labels = ["A", "B"]
"#,
        );
        let err = load(&path).expect_err("labels too short");
        assert!(format!("{err}").contains("labels"));
    }

    #[test]
    fn rejects_rendering_colors_length_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_manifest(
            tmp.path(),
            r#"
schema_version = 1

[parameters.write_policy]
values = ["a", "b"]
default = "a"

[parameters.write_policy.rendering]
colors = ["red", "green", "blue"]
"#,
        );
        let err = load(&path).expect_err("colors too long");
        assert!(format!("{err}").contains("colors"));
    }

    #[test]
    fn rendering_with_only_labels_is_valid() {
        // labels OR colors -- both are independently optional.
        let tmp = tempfile::tempdir().unwrap();
        let path = write_manifest(
            tmp.path(),
            r#"
schema_version = 1

[parameters.x]
values = [1, 2, 3]
default = 1

[parameters.x.rendering]
labels = ["one", "two", "three"]
"#,
        );
        let manifest = load(&path).expect("loads");
        let param = manifest.parameter("x").expect("present");
        let rendering = param.rendering.as_ref().expect("rendering");
        assert!(rendering.labels.is_some());
        assert!(rendering.colors.is_none());
    }
}
